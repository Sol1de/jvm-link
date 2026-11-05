//! Callback-free JVMTI ownership, independent of JNI reference management.
use crate::{Error, Result, connection::Admission, scope};
use jni::sys;
use jvmti_bindings::sys::jvmti as raw;
use std::ptr::NonNull;

/// Explicit capability bits. Availability is determined by the Java 21 host.
pub use raw::jvmtiCapabilities as JvmtiCapabilities;

/// An optional, current-thread session borrowing a connection.
///
/// Create with `Connection::jvmti_session`, then call `open`. Opening is separate
/// so even failed initialization leaves an owner available for cleanup retries.
/// No callbacks or event notifications are installed and no raw handle is exposed.
/// Explicitly retry `close` before dropping a session. If Drop cleanup fails,
/// its environment and error remain owned by the VM cleanup registry. Retry
/// `Connection::close` on this thread, even through a later connection to that VM.
/// The unsafe connection's host-liveness obligation includes pending environments.
///
/// ```
/// # use jvm_link_contract::{Connection, JvmtiCapabilities, Result};
/// fn inspect(connection: &Connection) -> Result<()> {
///     let mut session = connection.jvmti_session()?;
///     // Keep `session` and close it explicitly even if open returns an error.
///     let opened = session.open(&JvmtiCapabilities::default());
///     session.close()?;
///     opened
/// }
/// ```
///
/// A session cannot be moved to another thread.
///
/// ```compile_fail
/// # use jvm_link_contract::Connection;
/// fn transfer(connection: &Connection) {
///     let session = connection.jvmti_session().unwrap();
///     std::thread::scope(|threads| { threads.spawn(move || drop(session)); });
/// }
/// ```
pub struct JvmtiSession<'connection> {
    vm: NonNull<sys::JavaVM>,
    admission: Option<Admission<'connection>>,
    environment: Option<Environment>,
    closing: bool,
}

struct Environment(NonNull<raw::jvmtiEnv>);

impl<'connection> JvmtiSession<'connection> {
    pub(crate) fn new(vm: NonNull<sys::JavaVM>, admission: Admission<'connection>) -> Self {
        Self {
            vm,
            admission: Some(admission),
            environment: None,
            closing: false,
        }
    }

    /// Opens JVMTI 21 and requests exactly these capabilities. Use `Default` for
    /// none. Further calls add capabilities to the same environment, never replace
    /// it. On any error, ownership remains here; retry or explicitly close.
    pub fn open(&mut self, capabilities: &JvmtiCapabilities) -> Result<()> {
        if self.admission.is_none() {
            return Err(Error::Closed);
        }
        if self.closing {
            return Err(Error::Closing);
        }
        let vm = self.vm;
        unsafe {
            scope::with_env(vm, |_| {
                if self.environment.is_none() {
                    self.environment = Some(Environment::open(vm)?);
                }
                self.environment
                    .as_ref()
                    .unwrap()
                    .add_capabilities(capabilities)
            })
        }
    }

    /// Disposes without consuming the session. Failed disposal retains exactly
    /// the same environment; retries also work while the connection is closing.
    pub fn close(&mut self) -> Result<()> {
        if self.admission.is_none() {
            return Ok(());
        }
        self.closing = true;
        if self.environment.is_some() {
            unsafe { scope::with_env(self.vm, |_| dispose(&mut self.environment)) }?;
        }
        // A successful disposal can still have left a failed thread detach.
        unsafe { scope::retry_cleanup(self.vm) }?;
        self.admission = None;
        Ok(())
    }
}

impl Drop for JvmtiSession<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            scope::retain_jvmti(
                self.vm,
                Pending {
                    environment: self.environment.take(),
                    error: Some(error),
                },
            );
        }
    }
}

pub(crate) struct Pending {
    environment: Option<Environment>,
    error: Option<Error>,
}

// The registry transports ownership only; take_jvmti permits access on its owner thread.
unsafe impl Send for Pending {}

pub(crate) unsafe fn retry_pending(vm: NonNull<sys::JavaVM>, report: bool) -> Result<()> {
    let Some(mut cleanup) = scope::take_jvmti(vm)? else {
        return Ok(());
    };
    for pending in &mut cleanup.pending {
        if pending.environment.is_some() {
            let result = unsafe { scope::with_env(vm, |_| dispose(&mut pending.environment)) };
            if let Err(error) = result {
                if report {
                    return Err(error);
                }
                cleanup.pending.push(Pending {
                    environment: None,
                    error: Some(error),
                });
                return Ok(());
            }
        }
        if report && let Some(error) = pending.error.take() {
            return Err(error);
        }
    }
    cleanup
        .pending
        .retain(|p| p.environment.is_some() || p.error.is_some());
    Ok(())
}

impl Environment {
    unsafe fn open(vm: NonNull<sys::JavaVM>) -> Result<Self> {
        let mut pointer = std::ptr::null_mut();
        let code = unsafe {
            ((**vm.as_ptr()).v1_2.GetEnv)(vm.as_ptr(), &mut pointer, raw::JVMTI_VERSION_21)
        };
        jni::errors::jni_error_code_to_result(code).map_err(Error::Operation)?;
        let pointer = pointer.cast::<raw::jvmtiEnv>();
        if pointer.is_null() || !pointer.is_aligned() {
            return Err(Error::InvalidJvmtiEnvironment);
        }
        let table = unsafe { (*pointer).functions };
        if table.is_null() || !table.is_aligned() {
            return Err(Error::InvalidJvmtiEnvironment);
        }
        Ok(Self(NonNull::new(pointer).unwrap()))
    }

    fn add_capabilities(&self, capabilities: &JvmtiCapabilities) -> Result<()> {
        let function = unsafe { (*self.0.as_ref().functions).AddCapabilities }
            .ok_or(Error::InvalidJvmtiEnvironment)?;
        check("AddCapabilities", unsafe {
            function(self.0.as_ptr(), capabilities)
        })
    }
}

fn dispose(environment: &mut Option<Environment>) -> Result<()> {
    if let Some(owned) = environment {
        let function = unsafe { (*owned.0.as_ref().functions).DisposeEnvironment }
            .ok_or(Error::InvalidJvmtiEnvironment)?;
        check("DisposeEnvironment", unsafe { function(owned.0.as_ptr()) })?;
        *environment = None;
    }
    Ok(())
}

fn check(operation: &'static str, code: raw::jvmtiError) -> Result<()> {
    if code == raw::JVMTI_ERROR_NONE {
        Ok(())
    } else {
        Err(Error::Jvmti {
            operation,
            code: code.raw(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static GET_RESULT: Cell<(i32, *mut std::ffi::c_void)> = const { Cell::new((0, std::ptr::null_mut())) };
        static DISPOSE_CALLS: Cell<usize> = const { Cell::new(0) };
        static LAST_ENV: Cell<usize> = const { Cell::new(0) };
        static JNI_ENV: Cell<*mut std::ffi::c_void> = const { Cell::new(std::ptr::null_mut()) };
        static ATTACHED: Cell<bool> = const { Cell::new(false) };
        static DETACH_FAIL: Cell<bool> = const { Cell::new(false) };
        static PENDING_EXCEPTION: Cell<bool> = const { Cell::new(false) };
    }

    unsafe extern "system" fn scope_get(
        vm: *mut sys::JavaVM,
        out: *mut *mut std::ffi::c_void,
        version: i32,
    ) -> i32 {
        if version == raw::JVMTI_VERSION_21 {
            return unsafe { get(vm, out, version) };
        }
        if !ATTACHED.get() {
            return sys::JNI_EDETACHED;
        }
        unsafe {
            *out = JNI_ENV.get();
        }
        sys::JNI_OK
    }

    unsafe extern "system" fn attach(
        _: *mut sys::JavaVM,
        out: *mut *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
    ) -> i32 {
        ATTACHED.set(true);
        unsafe {
            *out = JNI_ENV.get();
        }
        sys::JNI_OK
    }

    unsafe extern "system" fn detach(_: *mut sys::JavaVM) -> i32 {
        if DETACH_FAIL.get() {
            return sys::JNI_ERR;
        }
        ATTACHED.set(false);
        sys::JNI_OK
    }

    unsafe extern "system" fn get_java_vm(_: *mut sys::JNIEnv, _: *mut *mut sys::JavaVM) -> i32 {
        // Do not register a stack-allocated mock VM in the binding's global singleton.
        sys::JNI_ERR
    }
    unsafe extern "system" fn exception_check(_: *mut sys::JNIEnv) -> sys::jboolean {
        PENDING_EXCEPTION.get()
    }
    unsafe extern "system" fn exception_occurred(_: *mut sys::JNIEnv) -> sys::jthrowable {
        std::ptr::null_mut()
    }
    unsafe extern "system" fn push_frame(_: *mut sys::JNIEnv, _: i32) -> i32 {
        sys::JNI_OK
    }
    unsafe extern "system" fn pop_frame(_: *mut sys::JNIEnv, value: sys::jobject) -> sys::jobject {
        value
    }
    unsafe extern "system" fn add(
        _: *mut raw::jvmtiEnv,
        _: *const JvmtiCapabilities,
    ) -> raw::jvmtiError {
        raw::JVMTI_ERROR_NONE
    }

    fn session_fixture(test: impl FnOnce(&crate::Connection, NonNull<raw::jvmtiEnv>)) {
        use std::ffi::c_void;
        // Stable C JNI table slots; only these entries are exercised by the binding.
        let mut jni_table = [std::ptr::null::<c_void>(); 234];
        jni_table[15] = exception_occurred as *const c_void;
        jni_table[19] = push_frame as *const c_void;
        jni_table[20] = pop_frame as *const c_void;
        jni_table[219] = get_java_vm as *const c_void;
        jni_table[228] = exception_check as *const c_void;
        let mut jni_env = jni_table.as_ptr().cast::<sys::JNINativeInterface_>();
        let mut functions: raw::jvmtiInterface_1_ = unsafe { std::mem::zeroed() };
        functions.DisposeEnvironment = Some(failing_dispose);
        functions.AddCapabilities = Some(add);
        let mut env = raw::jvmtiEnv {
            functions: &functions,
        };
        GET_RESULT.set((0, (&mut env as *mut raw::jvmtiEnv).cast()));
        JNI_ENV.set((&mut jni_env as *mut sys::JNIEnv).cast());
        ATTACHED.set(false);
        DETACH_FAIL.set(false);
        PENDING_EXCEPTION.set(false);
        DISPOSE_CALLS.set(0);
        let mut vm_table = [std::ptr::null::<c_void>(); 8];
        vm_table[5] = detach as *const c_void;
        vm_table[6] = scope_get as *const c_void;
        vm_table[7] = attach as *const c_void;
        let mut vm = vm_table.as_ptr().cast::<sys::JNIInvokeInterface_>();
        let vm = NonNull::from(&mut vm);
        let connection = unsafe { crate::Connection::from_vm(vm) };
        test(&connection, NonNull::from(&mut env));
        connection.close().unwrap();
        assert!(scope::cleanup_owners(vm).is_empty());
        assert!(!ATTACHED.get());
    }

    #[test]
    fn public_session_retries_disposal_and_keeps_connection_busy() {
        session_fixture(|connection, pointer| {
            let mut session = connection.jvmti_session().unwrap();
            session.open(&JvmtiCapabilities::default()).unwrap();
            assert!(matches!(
                session.close(),
                Err(Error::Jvmti {
                    operation: "DisposeEnvironment",
                    ..
                })
            ));
            assert_eq!(session.environment.as_ref().unwrap().0, pointer);
            assert!(matches!(
                session.open(&JvmtiCapabilities::default()),
                Err(Error::Closing)
            ));
            assert!(matches!(
                connection.close(),
                Err(Error::Busy { active_scopes: 1 })
            ));
            session.close().unwrap();
            session.close().unwrap();
            assert_eq!(DISPOSE_CALLS.get(), 2);
            assert_eq!(LAST_ENV.get(), pointer.as_ptr() as usize);
        });
    }

    #[test]
    fn public_session_does_not_dispose_twice_after_failed_detach() {
        session_fixture(|connection, _| {
            let mut session = connection.jvmti_session().unwrap();
            session.open(&JvmtiCapabilities::default()).unwrap();
            DISPOSE_CALLS.set(1);
            DETACH_FAIL.set(true);
            assert!(matches!(session.close(), Err(Error::Cleanup { .. })));
            assert!(session.environment.is_none());
            assert!(matches!(
                connection.close(),
                Err(Error::Busy { active_scopes: 1 })
            ));
            DETACH_FAIL.set(false);
            session.close().unwrap();
            assert_eq!(DISPOSE_CALLS.get(), 2);
        });
    }

    #[test]
    fn dropped_session_retains_disposal_and_error_for_owner_thread() {
        session_fixture(|connection, pointer| {
            let mut session = connection.jvmti_session().unwrap();
            session.open(&JvmtiCapabilities::default()).unwrap();
            drop(session);
            std::thread::scope(|threads| {
                threads
                    .spawn(|| {
                        assert!(matches!(
                            connection.close(),
                            Err(Error::CleanupPending { .. })
                        ))
                    })
                    .join()
                    .unwrap();
            });
            assert!(matches!(
                connection.close(),
                Err(Error::Jvmti {
                    operation: "DisposeEnvironment",
                    ..
                })
            ));
            assert_eq!(LAST_ENV.get(), pointer.as_ptr() as usize);
            assert_eq!(DISPOSE_CALLS.get(), 2);
            connection.close().unwrap();
        });
    }

    #[test]
    fn pending_exception_on_drop_preserves_environment_and_host_exception() {
        session_fixture(|connection, pointer| {
            let mut session = connection.jvmti_session().unwrap();
            session.open(&JvmtiCapabilities::default()).unwrap();
            PENDING_EXCEPTION.set(true);
            drop(session);
            assert!(PENDING_EXCEPTION.get());
            assert_eq!(DISPOSE_CALLS.get(), 0);
            PENDING_EXCEPTION.set(false);
            DISPOSE_CALLS.set(1);
            assert!(matches!(
                connection.close(),
                Err(Error::PendingJavaException)
            ));
            assert_eq!(DISPOSE_CALLS.get(), 2);
            assert_eq!(LAST_ENV.get(), pointer.as_ptr() as usize);
            connection.close().unwrap();
        });
    }

    #[test]
    fn connection_drop_preserves_pending_environment_and_unreported_errors() {
        session_fixture(|connection, pointer| {
            let vm = connection.jvmti_session().unwrap().vm;
            let first = unsafe { crate::Connection::from_vm(vm) };
            let mut session = first.jvmti_session().unwrap();
            session.open(&JvmtiCapabilities::default()).unwrap();
            drop(session);
            DISPOSE_CALLS.set(0);
            drop(first);
            assert_eq!(DISPOSE_CALLS.get(), 1);
            // A later connection retries the pointer and observes both failed cleanups.
            for _ in 0..2 {
                assert!(matches!(
                    connection.close(),
                    Err(Error::Jvmti {
                        operation: "DisposeEnvironment",
                        ..
                    })
                ));
            }
            assert_eq!(DISPOSE_CALLS.get(), 2);
            assert_eq!(LAST_ENV.get(), pointer.as_ptr() as usize);
            connection.close().unwrap();
        });
    }

    #[test]
    fn dropped_session_keeps_simultaneous_disposal_and_detach_errors() {
        session_fixture(|connection, _| {
            let mut session = connection.jvmti_session().unwrap();
            session.open(&JvmtiCapabilities::default()).unwrap();
            DETACH_FAIL.set(true);
            drop(session);
            DETACH_FAIL.set(false);
            assert!(
                matches!(connection.close(), Err(Error::Cleanup { operation: Some(error), .. })
                if matches!(*error, Error::Jvmti { operation: "DisposeEnvironment", .. }))
            );
            assert_eq!(DISPOSE_CALLS.get(), 2);
            connection.close().unwrap();
        });
    }

    #[test]
    fn in_progress_retry_remains_visible_to_reentrant_and_remote_shutdown() {
        session_fixture(|connection, _| {
            let mut session = connection.jvmti_session().unwrap();
            let vm = session.vm;
            session.open(&JvmtiCapabilities::default()).unwrap();
            drop(session);
            let cleanup = scope::take_jvmti(vm).unwrap().unwrap();
            assert!(scope::take_cleanup_error(vm).is_none());
            assert!(matches!(
                connection.close(),
                Err(Error::CleanupPending { .. })
            ));
            std::thread::scope(|threads| {
                threads
                    .spawn(|| {
                        assert!(matches!(
                            connection.close(),
                            Err(Error::CleanupPending { .. })
                        ))
                    })
                    .join()
                    .unwrap();
            });
            drop(cleanup);
            assert!(matches!(
                connection.close(),
                Err(Error::Jvmti {
                    operation: "DisposeEnvironment",
                    ..
                })
            ));
            connection.close().unwrap();
        });
    }

    unsafe extern "system" fn get(
        _: *mut sys::JavaVM,
        output: *mut *mut std::ffi::c_void,
        version: i32,
    ) -> i32 {
        assert_eq!(version, raw::JVMTI_VERSION_21);
        let (code, pointer) = GET_RESULT.get();
        unsafe {
            *output = pointer;
        }
        code
    }

    #[test]
    fn opening_checks_status_before_outputs_and_validates_pointers() {
        let mut table = [std::ptr::null::<std::ffi::c_void>(); 8];
        table[6] = get as *const std::ffi::c_void;
        let mut vm = table.as_ptr().cast::<sys::JNIInvokeInterface_>();
        let vm = NonNull::from(&mut vm);
        GET_RESULT.set((sys::JNI_EVERSION, std::ptr::dangling_mut::<u8>().cast()));
        assert!(matches!(
            unsafe { Environment::open(vm) },
            Err(Error::Operation(_))
        ));
        let mut env = raw::jvmtiEnv {
            functions: std::ptr::null(),
        };
        for pointer in [
            std::ptr::null_mut(),
            std::ptr::dangling_mut::<u8>().cast(),
            (&mut env as *mut raw::jvmtiEnv).cast(),
        ] {
            GET_RESULT.set((sys::JNI_OK, pointer));
            assert!(matches!(
                unsafe { Environment::open(vm) },
                Err(Error::InvalidJvmtiEnvironment)
            ));
        }
        let mut functions: raw::jvmtiInterface_1_ = unsafe { std::mem::zeroed() };
        functions.DisposeEnvironment = Some(failing_dispose);
        env.functions = &functions;
        GET_RESULT.set((sys::JNI_OK, (&mut env as *mut raw::jvmtiEnv).cast()));
        let mut owned = Some(unsafe { Environment::open(vm).unwrap() });
        assert!(matches!(
            owned
                .as_ref()
                .unwrap()
                .add_capabilities(&JvmtiCapabilities::default()),
            Err(Error::InvalidJvmtiEnvironment)
        ));
        DISPOSE_CALLS.set(1);
        dispose(&mut owned).unwrap();
    }

    unsafe extern "system" fn failing_dispose(env: *mut raw::jvmtiEnv) -> raw::jvmtiError {
        LAST_ENV.set(env as usize);
        let calls = DISPOSE_CALLS.replace(DISPOSE_CALLS.get() + 1);
        if calls == 0 {
            raw::jvmtiError::INTERNAL
        } else {
            raw::JVMTI_ERROR_NONE
        }
    }

    unsafe extern "system" fn unavailable(
        _: *mut raw::jvmtiEnv,
        caps: *const JvmtiCapabilities,
    ) -> raw::jvmtiError {
        assert!(unsafe { (*caps).can_tag_objects() });
        raw::jvmtiError::NOT_AVAILABLE
    }

    #[test]
    fn capability_error_and_dispose_failure_preserve_the_same_environment() {
        // All unused FFI slots are nullable function pointers or reserved pointers.
        let mut table: raw::jvmtiInterface_1_ = unsafe { std::mem::zeroed() };
        table.DisposeEnvironment = Some(failing_dispose);
        table.AddCapabilities = Some(unavailable);
        let mut env = raw::jvmtiEnv { functions: &table };
        let pointer = NonNull::from(&mut env);
        let mut owned = Some(Environment(pointer));
        let mut caps = JvmtiCapabilities::default();
        caps.set_can_tag_objects(true);
        assert!(matches!(
            owned.as_ref().unwrap().add_capabilities(&caps),
            Err(Error::Jvmti {
                operation: "AddCapabilities",
                code: 98
            })
        ));
        assert_eq!(owned.as_ref().unwrap().0, pointer);
        DISPOSE_CALLS.set(0);
        assert!(matches!(
            dispose(&mut owned),
            Err(Error::Jvmti {
                operation: "DisposeEnvironment",
                code: 113
            })
        ));
        assert_eq!(owned.as_ref().unwrap().0, pointer);
        assert_eq!(LAST_ENV.get(), pointer.as_ptr() as usize);
        dispose(&mut owned).unwrap();
        dispose(&mut owned).unwrap();
        assert!(owned.is_none());
        assert_eq!(DISPOSE_CALLS.get(), 2);
        assert_eq!(LAST_ENV.get(), pointer.as_ptr() as usize);
    }

    #[test]
    fn session_reservation_blocks_shutdown_and_close_is_idempotent() {
        let connection = unsafe { crate::Connection::from_vm(NonNull::dangling()) };
        let mut session = connection.jvmti_session().unwrap();
        assert!(matches!(
            connection.close(),
            Err(Error::Busy { active_scopes: 1 })
        ));
        session.close().unwrap();
        session.close().unwrap();
        assert!(matches!(
            session.open(&JvmtiCapabilities::default()),
            Err(Error::Closed)
        ));
        connection.close().unwrap();
        assert!(matches!(connection.jvmti_session(), Err(Error::Closed)));
    }
}

//! Current-thread attachment ownership; no connection or Java-version admission yet.
use crate::{Error, Result, config};
use jni::{AttachGuard, Env, errors::jni_error_code_to_result, sys};
use std::{
    cell::RefCell,
    collections::HashMap,
    marker::PhantomData,
    ptr::NonNull,
    rc::Rc,
    sync::{Mutex, OnceLock},
    thread::ThreadId,
};

#[derive(Default)]
struct State {
    depth: usize,
    owned: bool,
}

#[derive(Default)]
struct Owner {
    attached: bool,
    error: Option<Error>,
}

thread_local! {
    static ATTACHMENTS: RefCell<HashMap<usize, State>> = RefCell::new(HashMap::new());
}

struct Attachment {
    vm: NonNull<sys::JavaVM>,
    env: NonNull<sys::JNIEnv>,
    active: bool,
    _thread: PhantomData<Rc<()>>,
}

impl Attachment {
    unsafe fn enter(vm: NonNull<sys::JavaVM>) -> Result<Self> {
        let jni_version = config::get().jni_version();
        let mut raw = std::ptr::null_mut();
        let table = unsafe { &**vm.as_ptr() };
        let code = unsafe { (table.v1_2.GetEnv)(vm.as_ptr(), &mut raw, jni_version) };
        let owned = if code == sys::JNI_EDETACHED {
            let mut args = sys::JavaVMAttachArgs {
                version: jni_version,
                name: std::ptr::null_mut(),
                group: std::ptr::null_mut(),
            };
            let code = unsafe {
                (table.v1_4.AttachCurrentThreadAsDaemon)(
                    vm.as_ptr(),
                    &mut raw,
                    (&mut args as *mut sys::JavaVMAttachArgs).cast(),
                )
            };
            jni_error_code_to_result(code).map_err(Error::Attach)?;
            owners()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(vm.as_ptr() as usize)
                .or_default()
                .entry(std::thread::current().id())
                .or_default()
                .attached = true;
            true
        } else {
            jni_error_code_to_result(code).map_err(Error::Attach)?;
            false
        };
        ATTACHMENTS.with(|states| {
            let mut states = states.borrow_mut();
            let state = states.entry(vm.as_ptr() as usize).or_default();
            state.depth += 1;
            state.owned |= owned;
        });
        let mut attachment = Self {
            vm,
            env: NonNull::dangling(),
            active: true,
            _thread: PhantomData,
        };
        let env = raw.cast::<sys::JNIEnv>();
        let valid = !env.is_null()
            && env.is_aligned()
            && unsafe {
                let table = *env;
                !table.is_null() && table.is_aligned()
            };
        if !valid {
            return attachment.finish(Err(Error::Attach(jni::errors::Error::NullPtr("JNIEnv"))));
        }
        attachment.env = NonNull::new(env).unwrap();
        Ok(attachment)
    }

    fn release(&mut self) -> Result<()> {
        self.active = false;
        ATTACHMENTS.with(|states| {
            let mut states = states.borrow_mut();
            let state = states.get_mut(&(self.vm.as_ptr() as usize)).unwrap();
            state.depth -= 1;
        });
        unsafe { retry_cleanup(self.vm) }
    }

    fn finish<T>(&mut self, result: Result<T>) -> Result<T> {
        match self.release() {
            Ok(()) => result,
            Err(Error::Cleanup { owner, source, .. }) => Err(Error::Cleanup {
                owner,
                source,
                operation: result.err().map(Box::new),
            }),
            Err(error) => Err(error),
        }
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        if self.active
            && let Err(error) = self.release()
        {
            retain_cleanup_error(self.vm, error);
        }
    }
}

/// Retry only this thread's idle attachment. Never detaches a borrowed thread.
/// # Safety
/// The host VM and its invocation table must still be alive.
pub(crate) unsafe fn retry_cleanup(vm: NonNull<sys::JavaVM>) -> Result<()> {
    let key = vm.as_ptr() as usize;
    let owned = ATTACHMENTS
        .try_with(|states| {
            let mut states = states.borrow_mut();

            match states.get(&key) {
                Some(state) if state.depth != 0 => false,
                Some(state) if state.owned => true,
                _ => {
                    states.remove(&key);
                    false
                }
            }
        })
        .unwrap_or(false);
    if owned {
        let code = unsafe { ((**vm.as_ptr()).v1_1.DetachCurrentThread)(vm.as_ptr()) };
        jni_error_code_to_result(code).map_err(|source| Error::Cleanup {
            owner: std::thread::current().id(),
            source,
            operation: None,
        })?;
        ATTACHMENTS.with(|states| {
            states.borrow_mut().remove(&key);
        });
        let mut owners = owners().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(threads) = owners.get_mut(&key) {
            let owner = std::thread::current().id();

            if let Some(state) = threads.get_mut(&owner) {
                state.attached = false;
                if state.error.is_none() {
                    threads.remove(&owner);
                }
            }

            if threads.is_empty() {
                owners.remove(&key);
            }
        }
    }
    Ok(())
}

/// Run in a fresh binding frame on the current thread.
/// # Safety
/// The VM must be genuine and live until scopes, pending cleanup and exported
/// globals (including those in errors) have been released. This internal entry
/// does not perform Java runtime admission or connection lifecycle checks.
pub(crate) unsafe fn with_env<T>(
    vm: NonNull<sys::JavaVM>,
    operation: impl for<'local> FnOnce(&mut Env<'local>) -> Result<T>,
) -> Result<T> {
    let mut attachment = unsafe { Attachment::enter(vm)? };
    let result = {
        let mut borrowed = unsafe { AttachGuard::from_unowned(attachment.env.as_ptr()) };
        let env = borrowed.borrow_env_mut();
        if env.exception_check() {
            Err(Error::PendingJavaException)
        } else {
            let result = env
                .with_local_frame(32, |env| {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(env)));
                    let exception = capture_exception(env);
                    match result {
                        Ok(result) => {
                            Ok::<_, jni::errors::Error>(normalize_exception(result, exception))
                        }
                        Err(payload) => std::panic::resume_unwind(payload),
                    }
                })
                .map_err(Error::Frame)
                .and_then(|result| result);

            let exception = capture_exception(env);
            normalize_exception(result, exception)
        }
    };
    attachment.finish(result)
}

fn capture_exception(
    env: &mut Env<'_>,
) -> jni::errors::Result<Option<jni::refs::Global<jni::objects::JThrowable<'static>>>> {
    let Some(throwable) = env.exception_occurred() else {
        return Ok(None);
    };
    env.exception_clear();
    let result = env.new_global_ref(&throwable).map(Some);
    env.delete_local_ref(throwable);
    env.exception_clear();
    result
}

fn normalize_exception<T>(
    result: Result<T>,
    exception: jni::errors::Result<Option<jni::refs::Global<jni::objects::JThrowable<'static>>>>,
) -> Result<T> {
    match exception {
        Ok(None) => result,
        Ok(Some(throwable)) => Err(Error::JavaException {
            throwable,
            operation: result.err().map(Box::new),
        }),
        Err(source) => Err(Error::ExceptionCapture {
            source,
            operation: result.err().map(Box::new),
        }),
    }
}

fn owners() -> &'static Mutex<HashMap<usize, HashMap<ThreadId, Owner>>> {
    static OWNERS: OnceLock<Mutex<HashMap<usize, HashMap<ThreadId, Owner>>>> = OnceLock::new();
    OWNERS.get_or_init(Mutex::default)
}

pub(crate) fn cleanup_owners(vm: NonNull<sys::JavaVM>) -> Vec<ThreadId> {
    owners()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(vm.as_ptr() as usize))
        .map(|owners| owners.keys().copied().collect())
        .unwrap_or_default()
}

pub(crate) fn retain_cleanup_error(vm: NonNull<sys::JavaVM>, mut error: Error) {
    let mut owners = owners().lock().unwrap_or_else(|e| e.into_inner());
    let state = owners
        .entry(vm.as_ptr() as usize)
        .or_default()
        .entry(std::thread::current().id())
        .or_default();

    if let Error::Cleanup { operation, .. } = &mut error {
        debug_assert!(operation.is_none());
        *operation = state.error.take().map(Box::new);
    }
    state.error = Some(error);
}

pub(crate) fn take_cleanup_error(vm: NonNull<sys::JavaVM>) -> Option<Error> {
    let mut owners = owners().lock().unwrap_or_else(|e| e.into_inner());
    let key = vm.as_ptr() as usize;
    let threads = owners.get_mut(&key)?;
    let owner = std::thread::current().id();
    let state = threads.get_mut(&owner)?;
    let error = state.error.take();

    if !state.attached {
        threads.remove(&owner);
    }

    if threads.is_empty() {
        owners.remove(&key);
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::c_void, ptr};

    #[test]
    fn exception_capture_failure_retains_typed_cause_and_operation() {
        let result = normalize_exception::<()>(
            Err(Error::Operation(jni::errors::Error::JavaException)),
            Err(jni::errors::Error::JniCall(jni::errors::JniError::NoMemory)),
        );
        assert!(matches!(result, Err(Error::ExceptionCapture {
            source: jni::errors::Error::JniCall(jni::errors::JniError::NoMemory),
            operation: Some(error),
        }) if matches!(*error, Error::Operation(jni::errors::Error::JavaException))));
        assert_eq!(normalize_exception(Ok(42), Ok(None)).unwrap(), 42);
    }

    #[test]
    fn detach_failure_retains_exception_capture_error() {
        fixture(false, |vm| unsafe {
            let mut attachment = Attachment::enter(vm).unwrap();
            MOCK.with(|m| m.borrow_mut().detach_code = sys::JNI_ERR);
            let operation = normalize_exception::<()>(
                Err(Error::Closing),
                Err(jni::errors::Error::JniCall(jni::errors::JniError::NoMemory)),
            );
            assert!(matches!(attachment.finish(operation),
                Err(Error::Cleanup { operation: Some(error), .. })
                if matches!(*error, Error::ExceptionCapture { operation: Some(_), .. })));
            MOCK.with(|m| m.borrow_mut().detach_code = 0);
            retry_cleanup(vm).unwrap();
        });
    }

    struct Mock {
        attached: bool,
        get_code: Option<i32>,
        attach_code: i32,
        detach_code: i32,
        output: *mut c_void,
        gets: usize,
        attaches: usize,
        detaches: usize,
        version: i32,
    }
    thread_local! { static MOCK: RefCell<Mock> = const { RefCell::new(Mock {
        attached: false, get_code: None, attach_code: 0, detach_code: 0,
        output: ptr::null_mut(), gets: 0, attaches: 0, detaches: 0, version: 0,
    }) }; }

    unsafe extern "system" fn get(_: *mut sys::JavaVM, out: *mut *mut c_void, version: i32) -> i32 {
        MOCK.with(|mock| {
            let mut m = mock.borrow_mut();
            m.gets += 1;
            m.version = version;
            let code = m
                .get_code
                .unwrap_or(if m.attached { 0 } else { sys::JNI_EDETACHED });
            if code == 0 {
                unsafe { *out = m.output };
            }
            code
        })
    }
    unsafe extern "system" fn attach(
        _: *mut sys::JavaVM,
        out: *mut *mut c_void,
        args: *mut c_void,
    ) -> i32 {
        MOCK.with(|mock| {
            let mut m = mock.borrow_mut();
            m.attaches += 1;
            m.version = unsafe { (*(args.cast::<sys::JavaVMAttachArgs>())).version };
            if m.attach_code == 0 {
                m.attached = true;
                unsafe { *out = m.output };
            }
            m.attach_code
        })
    }
    unsafe extern "system" fn detach(_: *mut sys::JavaVM) -> i32 {
        MOCK.with(|mock| {
            let mut m = mock.borrow_mut();
            m.detaches += 1;
            if m.detach_code == 0 {
                m.attached = false;
            }
            m.detach_code
        })
    }

    fn fixture(borrowed: bool, test: impl FnOnce(NonNull<sys::JavaVM>)) {
        // C invocation table layout: only the three exercised entries are called.
        let table = [
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            detach as *const c_void,
            get as *const c_void,
            attach as *const c_void,
        ];
        let mut vm = table.as_ptr().cast::<sys::JNIInvokeInterface_>();
        // Attachment tests never invoke JNI environment functions.
        let mut env = ptr::dangling::<sys::JNINativeInterface_>();
        MOCK.with(|m| {
            *m.borrow_mut() = Mock {
                attached: borrowed,
                get_code: None,
                attach_code: 0,
                detach_code: 0,
                output: (&mut env as *mut sys::JNIEnv).cast(),
                gets: 0,
                attaches: 0,
                detaches: 0,
                version: 0,
            }
        });
        test(NonNull::from(&mut vm));
        ATTACHMENTS.with(|states| assert!(states.borrow().is_empty()));
        assert!(cleanup_owners(NonNull::from(&mut vm)).is_empty());
    }

    #[test]
    fn nested_scopes_borrow_or_own_until_outer_exit() {
        for borrowed in [true, false] {
            fixture(borrowed, |vm| unsafe {
                let mut outer = Attachment::enter(vm).unwrap();
                let mut inner = Attachment::enter(vm).unwrap();
                assert!(matches!(
                    inner.finish::<()>(Err(Error::Closing)),
                    Err(Error::Closing)
                ));
                MOCK.with(|m| assert_eq!(m.borrow().detaches, 0));
                outer.finish(Ok(())).unwrap();
                MOCK.with(|m| {
                    let m = m.borrow();
                    assert_eq!(
                        (m.gets, m.attaches, m.detaches),
                        (2, usize::from(!borrowed), usize::from(!borrowed))
                    );
                    assert_eq!(m.version, config::get().jni_version());
                    assert_eq!(m.attached, borrowed);
                });
            });
        }
    }

    #[test]
    fn unwind_keeps_borrowed_attachment_and_releases_owned_attachment() {
        for borrowed in [true, false] {
            fixture(borrowed, |vm| {
                let panic = std::panic::catch_unwind(|| unsafe {
                    let _outer = Attachment::enter(vm).unwrap();
                    let _inner = Attachment::enter(vm).unwrap();
                    panic!("operation panic");
                });
                assert!(panic.is_err());
                MOCK.with(|m| assert_eq!(m.borrow().detaches, usize::from(!borrowed)));
            });
        }
    }

    #[test]
    fn rejects_get_and_attach_errors_without_detach() {
        for code in [sys::JNI_ERR, sys::JNI_EVERSION, sys::JNI_ENOMEM, 42] {
            fixture(false, |vm| unsafe {
                MOCK.with(|m| m.borrow_mut().get_code = Some(code));
                assert!(matches!(Attachment::enter(vm), Err(Error::Attach(_))));
                MOCK.with(|m| assert_eq!((m.borrow().attaches, m.borrow().detaches), (0, 0)));
            });
            fixture(false, |vm| unsafe {
                MOCK.with(|m| m.borrow_mut().attach_code = code);
                assert!(matches!(Attachment::enter(vm), Err(Error::Attach(_))));
                MOCK.with(|m| assert_eq!((m.borrow().attaches, m.borrow().detaches), (1, 0)));
            });
        }
    }

    #[test]
    fn validates_environment_outputs_and_cleans_successful_attachment() {
        for output in [ptr::null_mut(), ptr::without_provenance_mut(1)] {
            for borrowed in [true, false] {
                fixture(borrowed, |vm| unsafe {
                    MOCK.with(|m| m.borrow_mut().output = output);
                    assert!(matches!(Attachment::enter(vm), Err(Error::Attach(_))));
                    MOCK.with(|m| assert_eq!(m.borrow().detaches, usize::from(!borrowed)));
                });
            }
        }
        for table in [ptr::null(), ptr::without_provenance(1)] {
            fixture(false, |vm| unsafe {
                let mut table: sys::JNIEnv = table;
                MOCK.with(|m| m.borrow_mut().output = (&mut table as *mut sys::JNIEnv).cast());
                assert!(matches!(Attachment::enter(vm), Err(Error::Attach(_))));
            });
        }
    }

    #[test]
    fn detach_failure_preserves_operation_and_ownership_for_retry() {
        fixture(false, |vm| unsafe {
            let mut scope = Attachment::enter(vm).unwrap();
            MOCK.with(|m| m.borrow_mut().detach_code = sys::JNI_ERR);
            assert!(matches!(scope.finish::<()>(Err(Error::Closing)),
                Err(Error::Cleanup { operation: Some(error), owner, .. })
                if matches!(*error, Error::Closing) && owner == std::thread::current().id()));
            // A different thread has no authority over the owner's attachment.
            let address = vm.as_ptr() as usize;
            std::thread::spawn(move || {
                retry_cleanup(NonNull::new(address as *mut sys::JavaVM).unwrap()).unwrap();
                MOCK.with(|m| assert_eq!(m.borrow().detaches, 0));
            })
            .join()
            .unwrap();
            MOCK.with(|m| assert_eq!(m.borrow().detaches, 1));
            // A subsequent scope borrows the pending attachment and retries on exit.
            let mut next = Attachment::enter(vm).unwrap();
            MOCK.with(|m| m.borrow_mut().detach_code = 0);
            next.finish(Ok(())).unwrap();
            MOCK.with(|m| assert_eq!((m.borrow().attaches, m.borrow().detaches), (1, 2)));
        });
    }

    #[test]
    fn unwind_retains_failed_detach_for_explicit_retry() {
        fixture(false, |vm| unsafe {
            MOCK.with(|m| m.borrow_mut().detach_code = sys::JNI_ERR);
            assert!(
                std::panic::catch_unwind(|| {
                    let _scope = Attachment::enter(vm).unwrap();
                    panic!("unwind");
                })
                .is_err()
            );
            assert!(matches!(retry_cleanup(vm), Err(Error::Cleanup { .. })));
            MOCK.with(|m| m.borrow_mut().detach_code = 0);
            retry_cleanup(vm).unwrap();
            retry_cleanup(vm).unwrap();
            assert!(matches!(
                take_cleanup_error(vm),
                Some(Error::Cleanup { .. })
            ));
            MOCK.with(|m| assert_eq!(m.borrow().detaches, 3));
        });
    }

    #[test]
    fn connection_close_preserves_failed_detach_and_reports_remote_owner() {
        fixture(false, |vm| unsafe {
            let connection = crate::connection::Connection::from_vm(vm);
            let mut attachment = Attachment::enter(vm).unwrap();
            MOCK.with(|m| m.borrow_mut().detach_code = sys::JNI_ERR);
            assert!(matches!(
                attachment.finish::<()>(Err(Error::Closing)),
                Err(Error::Cleanup {
                    operation: Some(_),
                    ..
                })
            ));
            std::thread::scope(|threads| {
                let connection = &connection;
                let owner = std::thread::current().id();
                threads
                    .spawn(move || {
                        assert!(
                            matches!(connection.close(), Err(Error::CleanupPending { owners })
                        if owners == vec![owner])
                        );
                    })
                    .join()
                    .unwrap();
            });
            assert!(matches!(connection.close(), Err(Error::Cleanup { .. })));
            assert!(matches!(
                connection.with_env(|_| Ok(())),
                Err(Error::Closing)
            ));
            MOCK.with(|m| m.borrow_mut().detach_code = 0);
            connection.close().unwrap();
            connection.close().unwrap();
            assert!(cleanup_owners(vm).is_empty());
            MOCK.with(|m| assert_eq!(m.borrow().detaches, 3));
        });
    }

    #[test]
    fn connection_drop_leaves_unwind_cleanup_owned_for_retry() {
        fixture(false, |vm| unsafe {
            let connection = crate::connection::Connection::from_vm(vm);
            MOCK.with(|m| m.borrow_mut().detach_code = sys::JNI_ERR);
            assert!(
                std::panic::catch_unwind(|| {
                    let _attachment = Attachment::enter(vm).unwrap();
                    panic!("operation");
                })
                .is_err()
            );
            MOCK.with(|m| m.borrow_mut().detach_code = 0);
            drop(connection);
            MOCK.with(|m| assert!(!m.borrow().attached));
            assert_eq!(cleanup_owners(vm), vec![std::thread::current().id()]);
            MOCK.with(|m| m.borrow_mut().detach_code = 0);
            let retry = crate::connection::Connection::from_vm(vm);
            assert!(matches!(retry.close(), Err(Error::Cleanup { .. })));
            retry.close().unwrap();
            assert!(cleanup_owners(vm).is_empty());
        });
    }

    #[test]
    fn closing_another_connection_cannot_detach_an_active_attachment() {
        fixture(false, |vm| unsafe {
            let connection = crate::connection::Connection::from_vm(vm);
            let mut attachment = Attachment::enter(vm).unwrap();
            assert!(
                matches!(connection.close(), Err(Error::CleanupPending { owners })
                if owners == vec![std::thread::current().id()])
            );
            MOCK.with(|m| assert_eq!(m.borrow().detaches, 0));
            attachment.finish(Ok(())).unwrap();
            connection.close().unwrap();
        });
    }

    #[test]
    fn close_retries_cleanup_retained_during_unwind() {
        fixture(false, |vm| unsafe {
            let connection = crate::connection::Connection::from_vm(vm);
            MOCK.with(|m| m.borrow_mut().detach_code = sys::JNI_ERR);
            assert!(
                std::panic::catch_unwind(|| {
                    let _attachment = Attachment::enter(vm).unwrap();
                    panic!("operation");
                })
                .is_err()
            );
            MOCK.with(|m| m.borrow_mut().detach_code = 0);
            assert!(matches!(
                connection.close(),
                Err(Error::Cleanup {
                    source: jni::errors::Error::JniCall(jni::errors::JniError::Unknown),
                    ..
                })
            ));
            MOCK.with(|m| assert!(!m.borrow().attached));
            connection.close().unwrap();
        });
    }

    #[test]
    fn drop_never_waits_for_stderr() {
        let stderr = std::io::stderr();
        let lock = stderr.lock();
        let (sent, received) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            fixture(false, |vm| unsafe {
                let connection = crate::connection::Connection::from_vm(vm);
                let mut attachment = Attachment::enter(vm).unwrap();
                MOCK.with(|m| m.borrow_mut().detach_code = sys::JNI_ERR);
                assert!(attachment.finish(Ok(())).is_err());
                drop(connection);
                sent.send(()).unwrap();
                MOCK.with(|m| m.borrow_mut().detach_code = 0);
                retry_cleanup(vm).unwrap();
                assert!(matches!(
                    take_cleanup_error(vm),
                    Some(Error::Cleanup { .. })
                ));
            });
        });
        let result = received.recv_timeout(std::time::Duration::from_secs(2));
        drop(lock);
        worker.join().unwrap();
        result.expect("Drop blocked on stderr");
    }

    #[test]
    fn failed_constructor_keeps_a_closing_connection_for_retry() {
        fixture(false, |vm| unsafe {
            let connection = crate::Connection::from_vm(vm);
            let mut attachment = Attachment::enter(vm).unwrap();
            MOCK.with(|m| m.borrow_mut().detach_code = sys::JNI_ERR);
            let validation =
                attachment.finish::<(u16, jni::JNIVersion)>(Err(Error::UnsupportedJavaVersion {
                    detected: 17,
                }));
            let Error::ConnectionFailed { source, connection } =
                connection.complete_validation(validation).unwrap_err()
            else {
                panic!("lost retry path");
            };
            assert!(
                matches!(*source, Error::Cleanup { operation: Some(error), .. }
                if matches!(*error, Error::UnsupportedJavaVersion { detected: 17 }))
            );
            assert!(matches!(
                connection.with_env(|_| Ok(())),
                Err(Error::Closing)
            ));
            MOCK.with(|m| m.borrow_mut().detach_code = 0);
            connection.close().unwrap();
        });
    }

    #[test]
    fn retry_error_keeps_the_original_unwind_cause() {
        fixture(false, |vm| unsafe {
            let connection = crate::Connection::from_vm(vm);
            let attachment = Attachment::enter(vm).unwrap();
            MOCK.with(|m| m.borrow_mut().detach_code = sys::JNI_ERR);
            drop(attachment);
            MOCK.with(|m| m.borrow_mut().detach_code = sys::JNI_ENOMEM);
            assert!(matches!(connection.close(), Err(Error::Cleanup {
                source: jni::errors::Error::JniCall(jni::errors::JniError::NoMemory),
                operation: Some(error), ..
            }) if matches!(*error, Error::Cleanup { source: jni::errors::Error::JniCall(jni::errors::JniError::Unknown), .. })));
            MOCK.with(|m| m.borrow_mut().detach_code = 0);
            connection.close().unwrap();
        });
    }
}

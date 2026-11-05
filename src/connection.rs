//! Borrowed host connection, compatibility admission and retryable shutdown.
use crate::{Error, Result, scope, vm_lookup};
use jni::{Env, sys};
use std::{
    ptr::NonNull,
    sync::{Mutex, MutexGuard},
};

#[derive(Debug, Default)]
enum Phase {
    #[default]
    Open,
    Closing,
    Closed,
}

#[derive(Debug, Default)]
struct State {
    phase: Phase,
    active: usize,
}

/// A borrowed Java 21 JVM. Call `close` explicitly before owner threads exit.
#[derive(Debug)]
pub struct Connection {
    vm: NonNull<sys::JavaVM>,
    state: Mutex<State>,
    _compatibility: Option<(u16, jni::JNIVersion)>,
}

// The host VM is shared; thread-local environments never enter this object.
unsafe impl Send for Connection {}
unsafe impl Sync for Connection {}

impl Connection {
    /// Discovers the existing host VM and admits only Java 21 with JNI 1.8 or newer.
    /// No JVM is created or destroyed, and no permanent attachment is retained.
    ///
    /// # Safety
    /// The host must keep its JVM and jvm.dll alive until all scopes, attachments
    /// and exported globals, including those held by errors, are released.
    /// This also applies after failure, including a `ConnectionFailed` error.
    pub unsafe fn connect_existing() -> Result<Self> {
        let vm = unsafe { vm_lookup::discover() }.map_err(Error::Discovery)?;
        unsafe { Self::connect_vm(vm) }
    }

    unsafe fn connect_vm(vm: NonNull<sys::JavaVM>) -> Result<Self> {
        Self::new(vm).complete_validation(unsafe { scope::with_env(vm, validate_java) })
    }

    pub(crate) fn complete_validation(
        mut self,
        validation: Result<(u16, jni::JNIVersion)>,
    ) -> Result<Self> {
        match validation {
            Ok(compatibility) => {
                self._compatibility = Some(compatibility);
                Ok(self)
            }
            Err(source) => {
                self.state().phase = Phase::Closing;
                if scope::cleanup_owners(self.vm).is_empty() {
                    Err(source)
                } else {
                    Err(Error::ConnectionFailed {
                        source: Box::new(source),
                        connection: Box::new(self),
                    })
                }
            }
        }
    }

    fn new(vm: NonNull<sys::JavaVM>) -> Self {
        Self {
            vm,
            state: Mutex::default(),
            _compatibility: None,
        }
    }

    /// # Safety
    /// The VM must be genuine and compatible. The host must keep it and its DLL
    /// alive through all scopes, pending cleanup and exported globals/errors.
    #[cfg(test)]
    pub(crate) unsafe fn from_vm(vm: NonNull<sys::JavaVM>) -> Self {
        Self::new(vm)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn admit(&self) -> Result<Admission<'_>> {
        let mut state = self.state();
        match state.phase {
            Phase::Open => state.active += 1,
            Phase::Closing => return Err(Error::Closing),
            Phase::Closed => return Err(Error::Closed),
        }
        Ok(Admission(self))
    }

    /// Runs a synchronous callback in a fresh local frame on the current thread.
    /// Only owned Rust values and binding globals may escape the frame.
    pub fn with_env<T>(
        &self,
        operation: impl for<'local> FnOnce(&mut Env<'local>) -> Result<T>,
    ) -> Result<T> {
        let _admission = self.admit()?;
        unsafe { scope::with_env(self.vm, operation) }
    }

    /// Closes without waiting for active scopes. Retry on `Busy` or cleanup errors.
    /// Only the owning thread can retry a failed detach. Repeated success is a no-op.
    /// A retained unwind/Drop failure is reported even if this retry succeeds;
    /// call again to finish closing after observing that error.
    pub fn close(&self) -> Result<()> {
        {
            let mut state = self.state();
            if matches!(state.phase, Phase::Closed) {
                return Ok(());
            }
            state.phase = Phase::Closing;
            if state.active != 0 {
                return Err(Error::Busy {
                    active_scopes: state.active,
                });
            }
        }
        // No connection lock spans JNI, even when close is reentered.
        let retained = scope::take_cleanup_error(self.vm);
        let cleanup = unsafe { scope::retry_cleanup(self.vm) };
        match (cleanup, retained) {
            (Err(Error::Cleanup { owner, source, .. }), operation) => {
                return Err(Error::Cleanup {
                    owner,
                    source,
                    operation: operation.map(Box::new),
                });
            }
            (Err(error), _) | (Ok(()), Some(error)) => return Err(error),
            (Ok(()), None) => {}
        }
        let owners = scope::cleanup_owners(self.vm);
        if !owners.is_empty() {
            return Err(Error::CleanupPending { owners });
        }
        self.state().phase = Phase::Closed;
        Ok(())
    }
}

struct Admission<'a>(&'a Connection);

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        self.0.state().active -= 1;
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        if !matches!(state.phase, Phase::Closed)
            && let Err(error) = unsafe { scope::retry_cleanup(self.vm) }
        {
            scope::retain_cleanup_error(self.vm, error);
        }
    }
}

fn validate_java(env: &mut Env<'_>) -> Result<(u16, jni::JNIVersion)> {
    let interface = env.version().map_err(Error::Operation)?;
    if interface < jni::JNIVersion::V1_8 {
        return Err(Error::Operation(jni::errors::Error::JniCall(
            jni::errors::JniError::WrongVersion,
        )));
    }
    let key = env.new_string("java.specification.version")?;
    let value = env
        .call_static_method(
            jni::jni_str!("java/lang/System"),
            jni::jni_str!("getProperty"),
            jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
            &[(&key).into()],
        )?
        .l()?;
    if value.is_null() {
        return Err(Error::JavaVersionUndetermined);
    }
    let value = env
        .cast_local::<jni::objects::JString>(value)?
        .try_to_string(env)?;
    let version = admit_version(&value)?;
    Ok((version, interface))
}

fn admit_version(value: &str) -> Result<u16> {
    let value = if value == "1.8" { "8" } else { value };
    let version = if !value.starts_with('0')
        && !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
    {
        value.parse::<u16>().ok().filter(|v| *v != 0)
    } else {
        None
    }
    .ok_or(Error::JavaVersionUndetermined)?;
    if version != 21 {
        return Err(Error::UnsupportedJavaVersion { detected: version });
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn admits_only_known_java_21_and_distinguishes_unknown_values() {
        assert_eq!(admit_version("21").unwrap(), 21);
        for (value, detected) in [("1.8", 8), ("17", 17), ("22", 22)] {
            assert!(
                matches!(admit_version(value), Err(Error::UnsupportedJavaVersion { detected: actual }) if actual == detected)
            );
        }
        for value in ["", "unknown", "21.0.1", "+21", "0", "65536", "1.21", "021"] {
            assert!(matches!(
                admit_version(value),
                Err(Error::JavaVersionUndetermined)
            ));
        }
    }

    #[test]
    fn connection_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Connection>();
    }

    #[test]
    fn public_constructor_reports_absent_host_without_creating_a_vm() {
        assert!(matches!(
            unsafe { Connection::connect_existing() },
            Err(Error::Discovery(crate::DiscoveryError::ModuleAbsent))
        ));
    }

    #[test]
    fn failed_validation_without_cleanup_returns_original_error() {
        let connection = unsafe { Connection::from_vm(NonNull::dangling()) };
        assert!(matches!(
            connection.complete_validation(Err(Error::JavaVersionUndetermined)),
            Err(Error::JavaVersionUndetermined)
        ));
    }

    #[test]
    fn close_inside_admitted_operation_is_busy_and_rejects_nested_work() {
        let connection = unsafe { Connection::from_vm(NonNull::dangling()) };
        let admission = connection.admit().unwrap();
        assert!(matches!(
            connection.close(),
            Err(Error::Busy { active_scopes: 1 })
        ));
        assert!(matches!(
            connection.with_env(|_| Ok(())),
            Err(Error::Closing)
        ));
        drop(admission);
        connection.close().unwrap();
        connection.close().unwrap();
        assert!(matches!(
            connection.with_env(|_| Ok(())),
            Err(Error::Closed)
        ));
    }

    #[test]
    fn concurrent_close_preserves_admitted_scopes_and_is_idempotent() {
        let connection = unsafe { Connection::from_vm(NonNull::dangling()) };
        let barrier = Barrier::new(2);
        std::thread::scope(|threads| {
            let barrier = &barrier;
            let connection = &connection;
            threads.spawn(move || {
                let _scope = connection.admit().unwrap();
                barrier.wait();
                barrier.wait();
                assert!(matches!(connection.admit(), Err(Error::Closing)));
            });
            barrier.wait();
            assert!(matches!(
                connection.close(),
                Err(Error::Busy { active_scopes: 1 })
            ));
            barrier.wait();
        });
        std::thread::scope(|threads| {
            for _ in 0..8 {
                threads.spawn(|| connection.close().unwrap());
            }
        });
    }

    #[test]
    fn admission_is_released_on_unwind() {
        let connection = unsafe { Connection::from_vm(NonNull::dangling()) };
        assert!(
            std::panic::catch_unwind(|| {
                let _scope = connection.admit().unwrap();
                panic!("operation");
            })
            .is_err()
        );
        connection.close().unwrap();
    }
}

//! Technical Java 21 adapter: compiles private discovery and scope modules unchanged.
use jvm_link_contract::{DiscoveryError, Error, Result};

#[path = "../../../src/config.rs"]
mod config;
#[path = "../../../src/scope.rs"]
mod scope;

#[path = "../../../src/vm_lookup.rs"]
mod vm_lookup;

/// # Safety
/// Called only by DiscoveryHost on its live Java 21 VM.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_DiscoveryHost_check(
    env: *mut jni::sys::JNIEnv,
    _: jni::sys::jclass,
) -> jni::sys::jboolean {
    // Keep assertions/panics inside Rust; never unwind through the JNI boundary.
    let passed = std::panic::catch_unwind(|| unsafe {
        let mut expected = std::ptr::null_mut();
        let code = ((**env).v1_1.GetJavaVM)(env, &mut expected);
        assert_eq!(code, jni::sys::JNI_OK);
        assert!(!expected.is_null());
        for _ in 0..10 {
            assert_eq!(vm_lookup::discover().unwrap().as_ptr(), expected);
        }
        let vm = std::ptr::NonNull::new(expected).unwrap();
        exercise_scopes(vm, false);
        let address = expected as usize;
        std::thread::spawn(move || {
            let vm = std::ptr::NonNull::new(address as *mut jni::sys::JavaVM).unwrap();
            exercise_scopes(vm, true);
        })
        .join()
        .unwrap();
    });
    passed.is_ok()
}

unsafe fn status(vm: std::ptr::NonNull<jni::sys::JavaVM>) -> i32 {
    let mut env = std::ptr::null_mut();
    unsafe { ((**vm.as_ptr()).v1_2.GetEnv)(vm.as_ptr(), &mut env, config::get().jni_version()) }
}

unsafe fn exercise_scopes(vm: std::ptr::NonNull<jni::sys::JavaVM>, native: bool) {
    let expected = if native {
        jni::sys::JNI_EDETACHED
    } else {
        jni::sys::JNI_OK
    };
    assert_eq!(unsafe { status(vm) }, expected);
    unsafe { exercise_exceptions_and_references(vm) };
    assert_eq!(unsafe { status(vm) }, expected);
    for _ in 0..10 {
        unsafe {
            scope::with_env(vm, |env| {
                let local = env.new_string("outer frame")?;
                if native {
                    let thread = env
                        .call_static_method(
                            jni::jni_str!("java/lang/Thread"),
                            jni::jni_str!("currentThread"),
                            jni::jni_sig!("()Ljava/lang/Thread;"),
                            &[],
                        )?
                        .l()?;
                    assert!(
                        env.call_method(
                            &thread,
                            jni::jni_str!("isDaemon"),
                            jni::jni_sig!("()Z"),
                            &[]
                        )?
                        .z()?
                    );
                }
                scope::with_env(vm, |inner| {
                    let _local = inner.new_string("inner frame")?;
                    Ok(())
                })?;
                assert_eq!(status(vm), jni::sys::JNI_OK);
                assert!(matches!(
                    scope::with_env::<()>(vm, |_| Err(Error::Closing)),
                    Err(Error::Closing)
                ));
                let panic = std::panic::catch_unwind(|| {
                    scope::with_env::<()>(vm, |inner| {
                        let _local = inner.new_string("panic frame")?;
                        panic!("expected nested scope panic");
                    })
                });
                assert!(panic.is_err());
                assert_eq!(status(vm), jni::sys::JNI_OK);
                assert_eq!(local.try_to_string(env)?, "outer frame");
                assert!(matches!(
                    env.throw_new(
                        jni::jni_str!("java/lang/IllegalStateException"),
                        jni::jni_str!("host-owned"),
                    ),
                    Err(jni::errors::Error::JavaException)
                ));
                let before = env.exception_occurred().unwrap();
                assert!(matches!(
                    scope::with_env::<()>(vm, |_| panic!("pending exception must block entry")),
                    Err(Error::PendingJavaException)
                ));
                assert!(env.exception_check());
                let after = env.exception_occurred().unwrap();
                env.exception_clear();
                assert!(env.is_same_object(&before, &after)?);
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(unsafe { status(vm) }, expected);
    }
    assert!(matches!(
        unsafe { scope::with_env::<()>(vm, |_| Err(Error::Closing)) },
        Err(Error::Closing)
    ));
    assert_eq!(unsafe { status(vm) }, expected);
    assert!(
        std::panic::catch_unwind(|| unsafe {
            scope::with_env::<()>(vm, |_| panic!("expected outer scope panic"))
        })
        .is_err()
    );
    assert_eq!(unsafe { status(vm) }, expected);
    unsafe {
        scope::retry_cleanup(vm).unwrap();
    }
    assert_eq!(unsafe { status(vm) }, expected);
}

unsafe fn exercise_exceptions_and_references(vm: std::ptr::NonNull<jni::sys::JavaVM>) {
    unsafe {
        exercise_allocation_failures(vm);
        for outcome in 0..3 {
            let mut weak = None;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scope::with_env(vm, |env| {
                    let local = env.new_object(
                        jni::jni_str!("java/lang/Object"),
                        jni::jni_sig!("()V"),
                        &[],
                    )?;
                    weak = Some(env.new_weak_ref(&local)?);
                    match outcome {
                        0 => Ok(()),
                        1 => Err(Error::Closing),
                        _ => panic!("expected reference scope panic"),
                    }
                })
            }));
            match outcome {
                0 => result.unwrap().unwrap(),
                1 => assert!(matches!(result.unwrap(), Err(Error::Closing))),
                _ => assert!(result.is_err()),
            }
            await_collection(vm, &weak.unwrap());
        }

        let (global, weak) = scope::with_env(vm, |env| {
            let local = env.new_string("exported global")?;
            Ok((env.new_global_ref(&local)?, env.new_weak_ref(&local)?))
        })
        .unwrap();
        scope::with_env(vm, |env| {
            assert_eq!(global.try_to_string(env)?, "exported global");
            assert!(!weak.is_garbage_collected(env)?);
            drop(global);
            Ok(())
        })
        .unwrap();
        await_collection(vm, &weak);

        for outcome in 0..3 {
            let error = scope::with_env::<()>(vm, |env| {
                let failure = env.call_static_method(
                    jni::jni_str!("DiscoveryHost"),
                    jni::jni_str!("fail"),
                    jni::jni_sig!("()V"),
                    &[],
                );
                assert!(matches!(failure, Err(jni::errors::Error::JavaException)));
                match outcome {
                    0 => Ok(()),
                    1 => Err(failure.unwrap_err().into()),
                    _ => Err(Error::Closing),
                }
            })
            .unwrap_err();
            let Error::JavaException {
                throwable,
                operation,
            } = error
            else {
                panic!("missing throwable: {error:?}")
            };
            assert_eq!(operation.is_some(), outcome != 0);
            let weak = scope::with_env(vm, |env| {
                assert!(!env.exception_check());
                assert_eq!(
                    throwable.get_message(env)?.try_to_string(env)?,
                    "host operation failed"
                );
                let weak = env.new_weak_ref(&throwable)?;
                drop(throwable);
                Ok(weak)
            })
            .unwrap();
            await_collection(vm, &weak);
        }
        scope::with_env(vm, |env| {
            let panic = std::panic::catch_unwind(|| {
                scope::with_env::<()>(vm, |inner| {
                    let _ = inner.throw_new(
                        jni::jni_str!("java/lang/IllegalStateException"),
                        jni::jni_str!("panic"),
                    );
                    panic!("expected panic with pending exception");
                })
            });
            assert!(panic.is_err());
            assert!(!env.exception_check());
            Ok(())
        })
        .unwrap();
    }
}

unsafe fn exercise_allocation_failures(vm: std::ptr::NonNull<jni::sys::JavaVM>) {
    thread_local! {
        static OOM: std::cell::Cell<jni::sys::jthrowable> = const { std::cell::Cell::new(std::ptr::null_mut()) };
    }
    unsafe extern "system" fn no_memory(
        _: *mut jni::sys::JNIEnv,
        _: jni::sys::jobject,
    ) -> jni::sys::jobject {
        std::ptr::null_mut()
    }
    unsafe extern "system" fn no_memory_with_exception(
        env: *mut jni::sys::JNIEnv,
        _: jni::sys::jobject,
    ) -> jni::sys::jobject {
        OOM.with(|oom| {
            assert_eq!(
                unsafe { ((**env).v1_1.Throw)(env, oom.get()) },
                jni::sys::JNI_OK
            )
        });
        std::ptr::null_mut()
    }
    unsafe extern "system" fn frame_no_memory(env: *mut jni::sys::JNIEnv, _: i32) -> i32 {
        unsafe { no_memory_with_exception(env, std::ptr::null_mut()) };
        jni::sys::JNI_ENOMEM
    }
    struct Restore(*mut jni::sys::JNIEnv, jni::sys::JNIEnv);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe { *self.0 = self.1 };
            OOM.with(|oom| oom.set(std::ptr::null_mut()));
        }
    }
    unsafe {
        for failure in 0..3 {
            scope::with_env(vm, |env| {
                let throwable = env.new_object(
                    jni::jni_str!("java/lang/IllegalStateException"),
                    jni::jni_sig!("()V"),
                    &[],
                )?;
                let throwable = env.cast_local::<jni::objects::JThrowable>(throwable)?;
                let oom = env.new_object(
                    jni::jni_str!("java/lang/OutOfMemoryError"),
                    jni::jni_sig!("()V"),
                    &[],
                )?;
                let raw = env.get_raw();
                // Java 21 supplies the JNI 1.6 table prefix; no newer slot is read.
                let mut table = (**raw).v1_6;
                match failure {
                    0 => table.NewGlobalRef = no_memory,
                    1 => table.NewGlobalRef = no_memory_with_exception,
                    _ => table.PushLocalFrame = frame_no_memory,
                }
                let restore = Restore(raw, *raw);
                OOM.with(|slot| slot.set(oom.as_raw()));
                *raw = (&table as *const _ as *const jni::sys::JNINativeInterface_).cast();
                let result = scope::with_env::<()>(vm, |inner| {
                    assert_ne!(failure, 2, "callback ran after failed frame allocation");
                    inner.throw(&throwable)?;
                    Ok(())
                });
                drop(restore);
                assert!(!env.exception_check());
                match result.unwrap_err() {
                    Error::ExceptionCapture { source, operation } if failure < 2 => {
                        match failure {
                            0 => assert!(matches!(
                                source,
                                jni::errors::Error::JniCall(jni::errors::JniError::NoMemory)
                            )),
                            _ => assert!(matches!(source, jni::errors::Error::JavaException)),
                        }
                        assert!(matches!(
                            operation.as_deref(),
                            Some(Error::Operation(jni::errors::Error::JavaException))
                        ));
                    }
                    Error::JavaException {
                        throwable,
                        operation,
                    } if failure == 2 => {
                        assert!(env.is_same_object(&throwable, &oom)?);
                        assert!(matches!(
                            operation.as_deref(),
                            Some(Error::Frame(jni::errors::Error::JniCall(
                                jni::errors::JniError::NoMemory
                            )))
                        ));
                    }
                    error => panic!("unexpected allocation failure {failure}: {error:?}"),
                }
                Ok(())
            })
            .unwrap();
        }
    }
}

unsafe fn await_collection<T>(vm: std::ptr::NonNull<jni::sys::JavaVM>, weak: &jni::refs::Weak<T>)
where
    T: jni::refs::Reference
        + Into<jni::objects::JObject<'static>>
        + AsRef<jni::objects::JObject<'static>>
        + Default
        + Send
        + Sync
        + 'static,
{
    unsafe {
        scope::with_env(vm, |env| {
            for _ in 0..100 {
                env.call_static_method(
                    jni::jni_str!("java/lang/System"),
                    jni::jni_str!("gc"),
                    jni::jni_sig!("()V"),
                    &[],
                )?;
                if weak.is_garbage_collected(env)? {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("reference was retained after its owning scope/global ended");
        })
        .unwrap();
    }
}

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
                assert!(matches!(
                    scope::with_env::<()>(vm, |_| panic!("pending exception must block entry")),
                    Err(Error::PendingJavaException)
                ));
                assert!(env.exception_check());
                env.exception_clear();
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

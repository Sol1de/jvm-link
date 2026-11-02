//! Technical JNI host adapter: compiles the private discovery module unchanged.
use jvm_link_contract::DiscoveryError;

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
    });
    passed.is_ok()
}

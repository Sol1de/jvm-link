//! Borrow the host's VM without loading or retaining its DLL.

use crate::DiscoveryError;
use jni::{errors::jni_error_code_to_result, sys};
use std::{ffi::c_void, ptr::NonNull};

type GetCreatedJavaVMs =
    unsafe extern "system" fn(*mut *mut sys::JavaVM, sys::jsize, *mut sys::jsize) -> sys::jint;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

/// # Safety
/// The host must keep its genuine JVM and jvm.dll alive during discovery and
/// every subsequent use of the returned pointer. This does not validate Java's
/// runtime version or initialize the binding's process-wide JavaVM singleton.
pub(crate) unsafe fn discover() -> Result<NonNull<sys::JavaVM>, DiscoveryError> {
    let name: Vec<u16> = "jvm.dll\0".encode_utf16().collect();
    let module = unsafe { GetModuleHandleW(name.as_ptr()) };
    let lookup = unsafe {
        resolve(module, |module| {
            GetProcAddress(module, c"JNI_GetCreatedJavaVMs".as_ptr().cast())
        })?
    };

    unsafe { query(|buffer, capacity, count| lookup(buffer, capacity, count)) }
}

// Safety: non-null module and symbol addresses must belong to the live host DLL;
// the symbol must implement GetCreatedJavaVMs with the declared ABI.
unsafe fn resolve(
    module: *mut c_void,
    symbol: impl FnOnce(*mut c_void) -> *mut c_void,
) -> Result<GetCreatedJavaVMs, DiscoveryError> {
    if module.is_null() {
        return Err(DiscoveryError::ModuleAbsent);
    }

    let address = symbol(module);
    if address.is_null() {
        return Err(DiscoveryError::SymbolAbsent);
    }

    Ok(unsafe { std::mem::transmute::<*mut c_void, GetCreatedJavaVMs>(address) })
}

// Safety: lookup must obey JNI buffer bounds and return a readable JavaVM pointer
// on success (if non-null and aligned). Pointer checks cannot prove host liveness.
unsafe fn query(
    lookup: impl FnOnce(*mut *mut sys::JavaVM, sys::jsize, *mut sys::jsize) -> sys::jint,
) -> Result<NonNull<sys::JavaVM>, DiscoveryError> {
    let mut vm = std::ptr::null_mut();
    let mut count = -1;
    let code = lookup(&mut vm, 1, &mut count);
    jni_error_code_to_result(code).map_err(DiscoveryError::Jni)?;
    match count {
        0 => return Err(DiscoveryError::NoVm),
        1 => {}
        2.. => return Err(DiscoveryError::MultipleVms),
        _ => return Err(DiscoveryError::InvalidVmCount { count }),
    }

    let vm = NonNull::new(vm).ok_or(DiscoveryError::InvalidVmPointer)?;
    if !vm.as_ptr().is_aligned() {
        return Err(DiscoveryError::InvalidVmPointer);
    }

    let table = unsafe { *vm.as_ptr() };
    if table.is_null() || !table.is_aligned() {
        return Err(DiscoveryError::InvalidVmPointer);
    }
    Ok(vm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    unsafe extern "system" fn no_vm(
        _: *mut *mut sys::JavaVM,
        _: sys::jsize,
        count: *mut sys::jsize,
    ) -> sys::jint {
        unsafe { *count = 0 };
        sys::JNI_OK
    }

    #[test]
    fn resolves_only_present_modules_and_symbols() {
        unsafe {
            assert!(matches!(
                resolve(ptr::null_mut(), |_| panic!(
                    "must not resolve a null module"
                )),
                Err(DiscoveryError::ModuleAbsent)
            ));
            let module = ptr::dangling_mut::<c_void>();
            assert!(matches!(
                resolve(module, |_| ptr::null_mut()),
                Err(DiscoveryError::SymbolAbsent)
            ));
            let lookup = resolve(module, |received| {
                assert_eq!(received, module);
                no_vm as *mut c_void
            })
            .unwrap();
            assert!(matches!(
                query(|b, n, c| lookup(b, n, c)),
                Err(DiscoveryError::NoVm)
            ));
        }
    }

    unsafe fn result(
        code: i32,
        count: i32,
        vm: *mut sys::JavaVM,
    ) -> Result<NonNull<sys::JavaVM>, DiscoveryError> {
        unsafe {
            query(|buffer, capacity, output_count| {
                assert_eq!(capacity, 1);
                assert!((*buffer).is_null());
                assert_eq!(*output_count, -1);
                *buffer = vm;
                *output_count = count;
                code
            })
        }
    }

    #[test]
    fn checks_jni_status_before_outputs() {
        for code in [sys::JNI_ERR, sys::JNI_EINVAL, sys::JNI_ENOMEM, 42] {
            let error = unsafe { result(code, 1, ptr::null_mut()) }.unwrap_err();
            assert!(matches!(error, DiscoveryError::Jni(_)));
            assert_eq!(
                format!("{error:?}"),
                format!("Jni({:?})", jni_error_code_to_result(code).unwrap_err())
            );
        }
    }

    #[test]
    fn rejects_missing_or_ambiguous_counts_before_reading_pointers() {
        for count in [-5, -1, 0, 2, i32::MAX] {
            let error = unsafe { result(sys::JNI_OK, count, ptr::null_mut()) }.unwrap_err();
            match count {
                0 => assert!(matches!(error, DiscoveryError::NoVm)),
                2.. => assert!(matches!(error, DiscoveryError::MultipleVms)),
                _ => assert!(
                    matches!(error, DiscoveryError::InvalidVmCount { count: c } if c == count)
                ),
            }
        }
        assert!(matches!(
            unsafe { query(|_, _, _| sys::JNI_OK) },
            Err(DiscoveryError::InvalidVmCount { count: -1 })
        ));
    }

    #[test]
    fn validates_vm_and_table_pointers() {
        unsafe {
            for vm in [ptr::null_mut(), ptr::without_provenance_mut(1)] {
                assert!(matches!(
                    result(sys::JNI_OK, 1, vm),
                    Err(DiscoveryError::InvalidVmPointer)
                ));
            }
            for mut table in [ptr::null(), ptr::without_provenance(1)] {
                assert!(matches!(
                    result(sys::JNI_OK, 1, &mut table),
                    Err(DiscoveryError::InvalidVmPointer)
                ));
            }
            // This table address is never dereferenced or called by discovery.
            let mut table = ptr::dangling::<sys::JNIInvokeInterface_>();
            let vm = &mut table as *mut sys::JavaVM;
            assert_eq!(result(sys::JNI_OK, 1, vm).unwrap().as_ptr(), vm);
        }
    }

    #[test]
    fn native_process_has_no_jvm_module() {
        assert!(matches!(
            unsafe { discover() },
            Err(DiscoveryError::ModuleAbsent)
        ));
    }
}

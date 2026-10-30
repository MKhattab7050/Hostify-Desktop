//! User-scoped DPAPI vault. Ciphertext is bound to the Windows user and Hostify.
// Keep the small Win32 ABI here, as import.rs does, without changing workspace lock metadata.
#[cfg(windows)]
#[allow(non_snake_case)]
mod dpapi {
    use std::ffi::c_void;
    #[repr(C)]
    pub struct Blob {
        pub cbData: u32,
        pub pbData: *mut u8,
    }
    pub const CRYPTPROTECT_UI_FORBIDDEN: u32 = 1;
    #[link(name = "crypt32")]
    unsafe extern "system" {
        pub fn CryptProtectData(
            input: *const Blob,
            description: *const u16,
            entropy: *const Blob,
            reserved: *const c_void,
            prompt: *const c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
        pub fn CryptUnprotectData(
            input: *const Blob,
            description: *mut *mut u16,
            entropy: *const Blob,
            reserved: *const c_void,
            prompt: *const c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }
}
#[cfg(windows)]
fn crypt(input: &[u8], protect: bool) -> Result<Vec<u8>, String> {
    use dpapi::*;
    const ENTROPY: &[u8] = b"Hostify.env.v1.user-scope";
    let mut owned = input.to_vec();
    let source = Blob {
        cbData: owned.len().try_into().map_err(|_| "Value too large")?,
        pbData: owned.as_mut_ptr(),
    };
    let entropy = Blob {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as *mut u8,
    };
    let mut out = Blob {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &source,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        } else {
            CryptUnprotectData(
                &source,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
    };
    let failure = (ok == 0).then(std::io::Error::last_os_error);
    wipe(&mut owned);
    if let Some(error) = failure {
        return Err(format!(
            "Windows could not protect or decrypt this value for the current user: {error}."
        ));
    }
    if out.cbData == 0 {
        if !out.pbData.is_null() {
            unsafe {
                LocalFree(out.pbData.cast());
            }
        }
        return Ok(Vec::new());
    }
    let result = unsafe {
        let bytes = std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize);
        let result = bytes.to_vec();
        wipe(bytes);
        LocalFree(out.pbData.cast());
        result
    };
    Ok(result)
}
pub(crate) fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe {
            std::ptr::write_volatile(byte, 0);
        }
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}
pub fn seal(value: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(windows)]
    {
        crypt(value, true)
    }
    #[cfg(not(windows))]
    {
        let _ = value;
        Err("Environment secrets require Windows DPAPI.".into())
    }
}
pub fn open(value: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(windows)]
    {
        crypt(value, false)
    }
    #[cfg(not(windows))]
    {
        let _ = value;
        Err("Environment secrets require Windows DPAPI.".into())
    }
}
#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn round_trip_and_tamper() {
        let mut sealed = super::seal(b"private-test-value").unwrap();
        assert_eq!(super::open(&sealed).unwrap(), b"private-test-value");
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        assert!(super::open(&sealed).is_err());
    }
}

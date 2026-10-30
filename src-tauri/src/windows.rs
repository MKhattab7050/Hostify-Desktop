use std::{ffi::c_void, os::windows::process::CommandExt, path::PathBuf};

#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
    fn GetTokenInformation(token: *mut c_void, class: u32, info: *mut c_void, len: u32, needed: *mut u32) -> i32;
    fn ConvertSidToStringSidW(sid: *mut c_void, text: *mut *mut u16) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}

pub fn pipe_name() -> Result<String, String> {
    // TokenUser begins with SID_AND_ATTRIBUTES. The pointer-aligned allocation
    // remains alive until ConvertSidToStringSidW has copied the SID.
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), 8, &mut token) == 0 {
            return Err(format!("Cannot read Windows user: {}", std::io::Error::last_os_error()));
        }
        let result = (|| {
            let mut needed = 0;
            GetTokenInformation(token, 1, std::ptr::null_mut(), 0, &mut needed);
            let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
            if GetTokenInformation(token, 1, buffer.as_mut_ptr().cast(), needed, &mut needed) == 0 {
                return Err(format!("Cannot read Windows SID: {}", std::io::Error::last_os_error()));
            }
            let sid = *(buffer.as_ptr().cast::<*mut c_void>());
            let mut text = std::ptr::null_mut();
            if ConvertSidToStringSidW(sid, &mut text) == 0 {
                return Err("Cannot format Windows SID".into());
            }
            let mut len = 0;
            while *text.add(len) != 0 { len += 1; }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            Ok(format!(r"\\.\pipe\hostify-engine-{sid}"))
        })();
        CloseHandle(token);
        result
    }
}

pub fn spawn_engine() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let beside = exe.parent().ok_or("Cannot locate Hostify installation")?.join("hostify-engine.exe");
    let path = if beside.is_file() { beside } else if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/debug/hostify-engine.exe")
    } else { beside };
    std::process::Command::new(&path)
        .creation_flags(0x08000000 | 0x00000200) // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().map_err(|e| format!("Cannot start hosting engine ({}): {e}", path.display()))?;
    Ok(())
}

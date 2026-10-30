//! Win32 resource wrappers. All owned handles/descriptors are released on drop.
use std::{io, ptr};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    System::{JobObjects::*, Threading::*},
};
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub struct Handle(pub HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                CloseHandle(self.0);
            }
        }
    }
}
static OWNER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
pub fn set_owner(s: &str) -> io::Result<()> {
    let caller = sid()?;
    if caller != "S-1-5-18" && caller != s {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Only SYSTEM may select another owner SID",
        ));
    }
    if !s.starts_with("S-1-")
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'-' || b == b'S')
    {
        return Err(io::Error::other("Invalid owner SID"));
    }
    OWNER
        .set(s.into())
        .map_err(|_| io::Error::other("Owner SID already configured"))
}
pub fn sid() -> io::Result<String> {
    if let Some(s) = OWNER.get() {
        return Ok(s.clone());
    }
    unsafe {
        let mut raw = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Handle(raw);
        let mut len = 0;
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut len);
        let mut data = vec![0usize; (len as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(token.0, TokenUser, data.as_mut_ptr().cast(), len, &mut len) == 0 {
            return Err(io::Error::last_os_error());
        }
        let user = &*data.as_ptr().cast::<TOKEN_USER>();
        let mut out = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut out) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut n = 0;
        while *out.add(n) != 0 {
            n += 1
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(out, n));
        LocalFree(out.cast());
        Ok(s)
    }
}
pub fn pipe_name() -> io::Result<String> {
    Ok(format!(r"\\.\pipe\hostify-engine-{}", sid()?))
}
pub struct Security {
    descriptor: PSECURITY_DESCRIPTOR,
}
impl Security {
    pub fn user_only() -> io::Result<Self> {
        unsafe {
            let text = wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{})", sid()?));
            let mut descriptor = ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { descriptor })
        }
    }
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        }
    }
}
impl Drop for Security {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.descriptor);
        }
    }
}
pub fn single_instance() -> io::Result<Handle> {
    unsafe {
        let security = Security::user_only()?;
        let attr = security.attributes();
        let name = wide(&format!("Global\\HostifyEngine-{}", sid()?));
        let h = CreateMutexW(&attr, 0, name.as_ptr());
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        let h = Handle(h);
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Hostify engine is already running",
            ));
        }
        Ok(h)
    }
}
pub struct Job(Handle);
impl Job {
    pub fn new() -> io::Result<Self> {
        unsafe {
            let h = CreateJobObjectW(ptr::null(), ptr::null());
            if h.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(Handle(h));
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0.0,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }
    // A HANDLE is an opaque kernel value passed straight to Win32; it is never dereferenced here.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn assign(&self, process: HANDLE) -> io::Result<()> {
        unsafe {
            if AssignProcessToJobObject(self.0.0, process) == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }
}

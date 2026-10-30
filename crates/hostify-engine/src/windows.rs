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
        Self::with_memory_limit(None)
    }
    pub fn with_memory_limit(memory_mb: Option<u64>) -> io::Result<Self> {
        unsafe {
            let h = CreateJobObjectW(ptr::null(), ptr::null());
            if h.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(Handle(h));
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if let Some(mb) = memory_mb {
                info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
                info.JobMemoryLimit = (mb * 1024 * 1024) as usize;
            }
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

impl Job {
    pub fn terminate(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.0.0, 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    pub fn process_ids(&self) -> io::Result<Vec<u32>> {
        // The list can grow while being queried. Retry with a larger, aligned buffer.
        let mut count = 32usize;
        loop {
            let mut buffer = vec![0usize; count + 2];
            let ok = unsafe {
                QueryInformationJobObject(
                    self.0.0,
                    JobObjectBasicProcessIdList,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * std::mem::size_of::<usize>()) as u32,
                    ptr::null_mut(),
                )
            };
            let info = unsafe { &*buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
            if ok != 0 {
                let ids = unsafe {
                    std::slice::from_raw_parts(
                        info.ProcessIdList.as_ptr(),
                        info.NumberOfProcessIdsInList as usize,
                    )
                };
                return Ok(ids.iter().map(|p| *p as u32).collect());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_MORE_DATA as i32) {
                return Err(error);
            }
            count = count
                .saturating_mul(2)
                .max(info.NumberOfAssignedProcesses as usize);
            if count > 1_000_000 {
                return Err(io::Error::other("Too many processes in the job"));
            }
        }
    }
    pub fn running(&self) -> io::Result<bool> {
        Ok(!self.process_ids()?.is_empty())
    }
}
/// Ctrl+C console attachment mutates process-wide state and must be serialized.
pub fn graceful_stop(job: &Job, pid: u32) -> bool {
    use windows_sys::Win32::System::Console::*;
    static CONSOLE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Ok(_lock) = CONSOLE.lock() else {
        let _ = job.terminate();
        return false;
    };
    // Console control dispatch is asynchronous. Keep our handler disabled and remain
    // attached until delivery has completed; immediately restoring the handler can
    // terminate the engine with its own queued Ctrl+C event.
    let sent = unsafe {
        FreeConsole();
        if AttachConsole(pid) == 0 {
            false
        } else {
            SetConsoleCtrlHandler(None, 1);
            GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) != 0
        }
    };
    let mut graceful = false;
    if sent {
        let began = std::time::Instant::now();
        let deadline = began + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if matches!(job.running(), Ok(false)) {
                graceful = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // Allow the asynchronous console dispatcher to finish before restoring
        // this process's original Ctrl+C behavior, even for an immediate exit.
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    unsafe {
        FreeConsole();
        SetConsoleCtrlHandler(None, 0);
    }
    if !graceful {
        let _ = job.terminate();
    }
    graceful
}

impl Job {
    pub fn peak_memory(&self) -> io::Result<u64> {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            QueryInformationJobObject(
                self.0.0,
                JobObjectExtendedLimitInformation,
                (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(info.PeakJobMemoryUsed as u64)
    }
}
/// Enumerate listening endpoints owned by this job, including IPv6 wildcard listeners.
pub fn exposed_addresses(pids: &[u32]) -> io::Result<Vec<String>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::*;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    let mut addresses = Vec::new();
    for family in [AF_INET, AF_INET6] {
        let mut size = 0u32;
        unsafe {
            GetExtendedTcpTable(
                ptr::null_mut(),
                &mut size,
                0,
                family as u32,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            );
        }
        loop {
            let mut bytes = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
            let status = unsafe {
                GetExtendedTcpTable(
                    bytes.as_mut_ptr().cast(),
                    &mut size,
                    0,
                    family as u32,
                    TCP_TABLE_OWNER_PID_LISTENER,
                    0,
                )
            };
            if status == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            if family == AF_INET {
                let table = unsafe { &*bytes.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>() };
                let rows = unsafe {
                    std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize)
                };
                for row in rows {
                    if pids.contains(&row.dwOwningPid) {
                        let address = std::net::Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes());
                        if !address.is_loopback() {
                            addresses.push(format!(
                                "{}:{}",
                                address,
                                u16::from_be(row.dwLocalPort as u16)
                            ));
                        }
                    }
                }
            } else {
                let table = unsafe { &*bytes.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>() };
                let rows = unsafe {
                    std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize)
                };
                for row in rows {
                    if pids.contains(&row.dwOwningPid) {
                        let address = std::net::Ipv6Addr::from(row.ucLocalAddr);
                        if !address.is_loopback() {
                            addresses.push(format!(
                                "[{}]:{}",
                                address,
                                u16::from_be(row.dwLocalPort as u16)
                            ));
                        }
                    }
                }
            }
            break;
        }
    }
    addresses.sort();
    addresses.dedup();
    Ok(addresses)
}

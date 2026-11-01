use anyhow::{Result, anyhow, bail};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};
use windows_service::{
    service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_manager::{ServiceManager, ServiceManagerAccess},
};
const SERVICE: &str = "HostifyEngine";
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn hidden(exe: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new(
        Path::new(&std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()))
            .join("System32")
            .join(exe),
    );
    c.creation_flags(0x08000000);
    c
}
pub fn task(install: bool, root: &Path) -> Result<()> {
    let sid = crate::windows::sid()?;
    let name = format!("HostifyEngine-{sid}");
    let mut command = hidden("schtasks.exe");
    if install {
        let exe = std::env::current_exe()?;
        std::fs::create_dir_all(root.join("run"))?;
        let path = root.join("run/logon-task.xml");
        let definition = format!(
            r#"<?xml version="1.0" encoding="UTF-16"?><Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task"><Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{}</UserId></LogonTrigger></Triggers><Principals><Principal id="Author"><UserId>{}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals><Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><RestartOnFailure><Interval>PT1M</Interval><Count>10</Count></RestartOnFailure></Settings><Actions Context="Author"><Exec><Command>{}</Command><Arguments>{}</Arguments></Exec></Actions></Task>"#,
            xml(&sid),
            xml(&sid),
            xml(&exe.to_string_lossy()),
            task_arguments(root)
        );
        let data = std::iter::once(0xfeffu16)
            .chain(definition.encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        std::fs::write(&path, data)?;
        command
            .args(["/Create", "/TN", &name, "/XML"])
            .arg(&path)
            .arg("/F");
    } else {
        command.args(["/Delete", "/TN", &name, "/F"]);
    }
    let output = command.output()?;
    if !output.status.success() {
        bail!(
            "Scheduled Task update failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
    Ok(())
}
fn service_arguments(root: &Path, sid: &str) -> Vec<OsString> {
    vec![
        "--service".into(),
        "--data-dir".into(),
        root.as_os_str().to_owned(),
        "--owner-sid".into(),
        sid.into(),
    ]
}
fn task_arguments(root: &Path) -> String {
    format!("--data-dir &quot;{}&quot;", xml(&root.to_string_lossy()))
}
pub fn service_install(install: bool, root: &Path) -> Result<()> {
    if install {
        anyhow::bail!(
            "Windows Service hosting is disabled until Node.js and PHP workers use restricted execution accounts. Use the per-user login task."
        );
    }
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT
            | if install {
                ServiceManagerAccess::CREATE_SERVICE
            } else {
                ServiceManagerAccess::empty()
            },
    )?;
    if install {
        let sid = crate::windows::sid()?;
        manager
            .create_service(
                &ServiceInfo {
                    name: OsString::from(SERVICE),
                    display_name: OsString::from("Hostify Engine"),
                    service_type: ServiceType::OWN_PROCESS,
                    start_type: ServiceStartType::AutoStart,
                    error_control: ServiceErrorControl::Normal,
                    executable_path: std::env::current_exe()?,
                    launch_arguments: service_arguments(root, &sid),
                    dependencies: vec![],
                    account_name: None,
                    account_password: None,
                },
                ServiceAccess::START | ServiceAccess::QUERY_STATUS,
            )?
            .start::<&str>(&[])?;
    } else {
        let service = manager.open_service(
            SERVICE,
            ServiceAccess::DELETE | ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
        )?;
        if service.query_status()?.current_state != ServiceState::Stopped {
            service.stop()?;
        }
        service.delete()?;
    }
    Ok(())
}
pub fn data_dir() -> Result<PathBuf> {
    Ok(hostify_core::paths::HostifyPaths::discover()?.root)
}
pub async fn run(
    root: PathBuf,
    service: bool,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    // LocalSystem services must never spawn dynamic customer workloads.
    if service {
        anyhow::bail!(
            "Service mode cannot safely host Node.js/PHP applications as LocalSystem. Use per-user hosting."
        );
    }
    let _instance = crate::windows::single_instance()?;
    let store = crate::state::Store::open(root)?;
    let supervisor = crate::supervisor::Supervisor::new(store.clone())?;
    let engine = crate::state::Engine::production(store.clone(), supervisor.clone(), service);
    store.activity("info", None, "Hostify engine started")?;
    let result = serve_engine(crate::windows::pipe_name()?, engine, shutdown).await;
    supervisor.shutdown().await;
    store.activity("info", None, "Hostify engine stopped")?;
    result
}
async fn serve_engine(
    name: String,
    engine: std::sync::Arc<crate::state::Engine>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let startup_engine = engine.clone();
    let startup = tokio::spawn(async move {
        if let Err(e) = startup_engine.startup().await {
            let _ = startup_engine.store.activity(
                "error",
                None,
                &format!("Startup recovery requires attention: {e}"),
            );
        }
    });
    let server = crate::pipe::serve(name, engine.clone(), shutdown.clone());
    tokio::pin!(server);
    let result = tokio::select! {result=&mut server=>result.map_err(anyhow::Error::from),_=shutdown.changed()=>Ok(())};
    startup.abort();
    let _ = startup.await;
    engine.shutdown().await;
    result
}
static CONSOLE_STOP: std::sync::OnceLock<tokio::sync::watch::Sender<bool>> =
    std::sync::OnceLock::new();
static CONSOLE_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
unsafe extern "system" fn console_handler(event: u32) -> i32 {
    if [0, 1, 2, 5, 6].contains(&event) {
        if let Some(tx) = CONSOLE_STOP.get() {
            let _ = tx.send(true);
        }
        if [2, 5, 6].contains(&event) {
            for _ in 0..40 {
                if CONSOLE_DONE.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        1
    } else {
        0
    }
}
pub fn main_entry() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(i) = args.iter().position(|a| a == "--owner-sid") {
        crate::windows::set_owner(
            args.get(i + 1)
                .ok_or_else(|| anyhow!("Missing owner SID"))?,
        )?;
    }
    let root = std::path::absolute(data_dir()?)?;
    match args.get(1).map(String::as_str) {
        Some("install-task") => return task(true, &root),
        Some("uninstall-task") => return task(false, &root),
        Some("install-service") => return service_install(true, &root),
        Some("uninstall-service") => return service_install(false, &root),
        _ => {}
    }
    if args.iter().any(|a| a == "--service") {
        return windows_service::service_dispatcher::start(SERVICE, ffi_service_main)
            .map_err(Into::into);
    }
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if args.get(1).map(String::as_str) == Some("status") {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            let mut pipe = tokio::net::windows::named_pipe::ClientOptions::new()
                .open(crate::windows::pipe_name()?)?;
            pipe.write_all(b"{\"id\":1,\"method\":\"engine_status\",\"params\":{}}\n")
                .await?;
            let mut response = String::new();
            tokio::time::timeout(
                Duration::from_secs(15),
                tokio::io::BufReader::new(pipe).read_line(&mut response),
            )
            .await??;
            let response: serde_json::Value = serde_json::from_str(&response)?;
            if let Some(e) = response["error"].as_str() {
                bail!("{e}")
            }
            let status = &response["result"];
            println!(
                "Data dir: {}\nBin dir: {}",
                status["dataDir"].as_str().unwrap_or("unknown"),
                status["binDir"].as_str().unwrap_or("unknown")
            );
            for name in ["caddy", "cloudflared"] {
                println!(
                    "{}: ready={}, version={}{}",
                    name,
                    status[name]["ready"],
                    status[name]["version"].as_str().unwrap_or("unavailable"),
                    status[name]["error"]
                        .as_str()
                        .map(|e| format!(" ({e})"))
                        .unwrap_or_default()
                );
            }
            return Ok(());
        }
        let (tx, rx) = tokio::sync::watch::channel(false);
        let _ = CONSOLE_STOP.set(tx);
        unsafe {
            if windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console_handler), 1)
                == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        let result = run(root, false, rx).await;
        CONSOLE_DONE.store(true, std::sync::atomic::Ordering::SeqCst);
        result
    })
}
windows_service::define_windows_service!(ffi_service_main, service_main);
fn service_main(_: Vec<OsString>) {
    let result = run_service();
    if let Err(e) = result {
        if let Ok(root) = data_dir() {
            let _ = std::fs::create_dir_all(root.join("logs"));
            let _ = crate::supervisor::append_log(
                &root.join("logs/engine.log"),
                &format!("Service failed: {e}"),
            );
        }
    }
}
fn run_service() -> Result<()> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = service_control_handler::register(SERVICE, move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    let report = |state, code| {
        handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code: ServiceExitCode::Win32(code),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        })
    };
    report(ServiceState::Running, 0)?;
    let result =
        tokio::runtime::Runtime::new()?.block_on(run(std::path::absolute(data_dir()?)?, true, rx));
    report(ServiceState::Stopped, if result.is_ok() { 0 } else { 1 })?;
    result
}
#[cfg(test)]
mod tests {
    #[test]
    fn xml_escape() {
        assert_eq!(super::xml("a&<\"'"), "a&amp;&lt;&quot;&apos;");
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;
    #[test]
    fn service_and_task_preserve_same_data_root() {
        let root = Path::new(r"C:\Users\Test User\Hostify & data");
        let args = service_arguments(root, "S-1-test");
        let resolved =
            hostify_core::paths::HostifyPaths::resolve(&args, Some("conflicting-env".into()), None)
                .unwrap();
        assert_eq!(resolved.root, root);
        assert_eq!(
            task_arguments(root),
            format!(
                "--data-dir &quot;{}&quot;",
                xml(&resolved.root.to_string_lossy())
            )
        );
        assert_eq!(args[0], "--service");
        assert_eq!(args[4], "S-1-test");
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;
    use hostify_core::deploy::SiteRuntime;
    use std::sync::Arc;
    struct Blocked;
    #[async_trait::async_trait]
    impl SiteRuntime for Blocked {
        async fn start_site(&self, _: &str) -> Result<bool, String> {
            std::future::pending().await
        }
        async fn stop_site(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        async fn start_tunnel(&self, _: &str) -> Result<bool, String> {
            Ok(true)
        }
        async fn stop_tunnel(&self) -> Result<(), String> {
            Ok(())
        }
        async fn site_responds(&self, _: u16) -> Result<bool, String> {
            Ok(true)
        }
    }
    #[tokio::test]
    async fn pipe_answers_while_startup_is_blocked() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let store = crate::state::Store::open(dir.path().join("data")).unwrap();
        store.db().unwrap().execute("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','s','source',9000,'now','now')",[]).unwrap();
        let engine = crate::state::Engine::new(store, Arc::new(Blocked), false);
        let name = format!(r"\\.\pipe\hostify-startup-{}", uuid::Uuid::new_v4());
        let (tx, rx) = tokio::sync::watch::channel(false);
        let server = tokio::spawn(serve_engine(name.clone(), engine, rx));
        let pipe = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(pipe) = tokio::net::windows::named_pipe::ClientOptions::new().open(&name)
                {
                    break pipe;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let mut client = BufReader::new(pipe);
        client
            .get_mut()
            .write_all(b"{\"id\":1,\"method\":\"ping\",\"params\":{}}\n")
            .await
            .unwrap();
        let mut response = String::new();
        tokio::time::timeout(Duration::from_secs(1), client.read_line(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&response).unwrap()["result"],
            "pong"
        );
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[cfg(test)]
mod uninstall_tests {
    use super::*;
    #[test]
    fn uninstall_stops_and_removes_only_matching_service() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("stop.ps1");
        std::fs::write(
            &script,
            include_str!("../../../src-tauri/resources/stop-engine.ps1"),
        )
        .unwrap();
        for matching in [true, false] {
            let path = if matching {
                r"C:\Hostify\engine.exe"
            } else {
                r"C:\Other\engine.exe"
            };
            let code = format!(
                r#"$global:stopped=$false
function Get-CimInstance {{ param($ClassName,$Filter) if ($ClassName -eq 'Win32_Service') {{ [pscustomobject]@{{PathName='"{path}" --service';State=$(if ($global:stopped) {{'Stopped'}} else {{'Running'}})}} }} }}
function Invoke-CimMethod {{ param($InputObject,$MethodName) $global:stopped=$true;[Console]::WriteLine($MethodName);[pscustomobject]@{{ReturnValue=0}} }}
& '{}' -EnginePath 'C:\Hostify\engine.exe' -RemoveService
"#,
                script.display().to_string().replace("'", "''")
            );
            let output = hidden("WindowsPowerShell/v1.0/powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &code,
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8_lossy(&output.stdout);
            assert_eq!(text.contains("StopService"), matching);
            assert_eq!(text.contains("Delete"), matching);
        }
    }
}

#[cfg(test)]
mod executable_tests {
    #[test]
    fn scheduler_uses_absolute_system_executable() {
        let command = super::hidden("schtasks.exe");
        let path = std::path::Path::new(command.get_program());
        assert!(path.is_absolute());
        assert!(path.ends_with("System32/schtasks.exe"));
    }
}

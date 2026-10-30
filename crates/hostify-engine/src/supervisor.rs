#[cfg(windows)]
use crate::windows::Job;
use crate::{port_available, restart_delay, state::Store};
use async_trait::async_trait;
use hostify_core::app_config::RestartPolicy;
use hostify_core::{
    deploy::SiteRuntime,
    secrets::{SecretStore, WindowsCredentialStore},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, watch};
const LOG_LIMIT: u64 = 5 * 1024 * 1024;
pub fn append_log(path: &Path, line: &str) -> io::Result<()> {
    // Each process stream shares this lock so rotation and writes stay ordered.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK
        .lock()
        .map_err(|_| io::Error::other("Log lock unavailable"))?;
    if std::fs::metadata(path)
        .map(|m| m.len() >= LOG_LIMIT)
        .unwrap_or(false)
    {
        for i in (1..=3).rev() {
            let old = if i == 1 {
                path.to_path_buf()
            } else {
                PathBuf::from(format!("{}.{}", path.display(), i - 1))
            };
            let new = PathBuf::from(format!("{}.{}", path.display(), i));
            if new.exists() {
                std::fs::remove_file(&new)?;
            }
            if old.exists() {
                std::fs::rename(old, new)?;
            }
        }
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}
pub fn tail_logs(path: &Path, lines: usize) -> io::Result<Vec<String>> {
    let mut tail = std::collections::VecDeque::new();
    for i in (0..=3).rev() {
        let p = if i == 0 {
            path.to_owned()
        } else {
            PathBuf::from(format!("{}.{}", path.display(), i))
        };
        let file = match std::fs::File::open(p) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for line in std::io::BufReader::new(file).lines() {
            tail.push_back(line?);
            while tail.len() > lines {
                tail.pop_front();
            }
        }
    }
    Ok(tail.into_iter().collect())
}
pub fn metrics_port(id: &str) -> u16 {
    let hash = id
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    45000 + (hash % 15000) as u16
}
pub async fn http_ok(port: u16, path: &str) -> bool {
    http_health(port, path, Health::Strict2xx).await
}
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Health {
    Strict2xx,
    AnyBelow500,
    TcpConnect,
}
pub(crate) async fn http_health(port: u16, path: &str, health: Health) -> bool {
    if health == Health::TcpConnect {
        return matches!(
            tokio::time::timeout(
                Duration::from_secs(2),
                tokio::net::TcpStream::connect(("127.0.0.1", port))
            )
            .await,
            Ok(Ok(_))
        );
    }
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: hostify-health.example\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await?;
        let mut bytes = [0; 128];
        let n = stream.read(&mut bytes).await?;
        let text = String::from_utf8_lossy(&bytes[..n]);
        let code=text.split_whitespace().nth(1).and_then(|s|s.parse::<u16>().ok());
        Ok::<bool, io::Error>(code.is_some_and(|c|match health {Health::Strict2xx=>(200..300).contains(&c),Health::AnyBelow500=>(100..500).contains(&c),Health::TcpConnect=>false}))
    })
    .await;
    matches!(result, Ok(Ok(true)))
}
pub async fn tunnel_connections(port: u16) -> u64 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let response = tokio::time::timeout(Duration::from_secs(2), async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        stream
            .write_all(b"GET /metrics HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut response = String::new();
        stream
            .take(1024 * 1024)
            .read_to_string(&mut response)
            .await?;
        Ok::<String, io::Error>(response)
    })
    .await;
    match response {
        Ok(Ok(text)) => text
            .lines()
            .filter(|line| {
                line.starts_with("cloudflared_tunnel_ha_connections ")
                    || line.starts_with("cloudflared_tunnel_ha_connections{")
            })
            .filter_map(|line| line.split_whitespace().last()?.parse::<u64>().ok())
            .sum(),
        _ => 0,
    }
}
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Role {
    Caddy,
    App,
    Connector,
}
#[derive(Clone)]
pub(crate) struct Spec {
    pub(crate) key: String,
    pub(crate) site: Option<String>,
    pub(crate) exe: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) token: Option<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) redact: Vec<String>,
    pub(crate) log: PathBuf,
    pub(crate) port: u16,
    pub(crate) path: String,
    pub(crate) role: Role,
    pub(crate) health: Health,
    pub(crate) restart: RestartPolicy,
    pub(crate) memory_limit_mb: Option<u64>,
}
/// A process needs a moment to open its port, so an early failed check is not yet news.
fn still_starting(healthy: Option<bool>, ok: bool, running_for: Duration) -> bool {
    !ok && healthy.is_none() && running_for < Duration::from_secs(15)
}
#[cfg(test)]
mod startup_grace {
    use super::*;
    #[test]
    fn only_an_early_first_failure_is_ignored() {
        let early = Duration::from_secs(2);
        let late = Duration::from_secs(16);
        assert!(still_starting(None, false, early));
        assert!(!still_starting(None, false, late));
        assert!(!still_starting(None, true, early));
        // Once a process has been healthy, losing health is reported straight away.
        assert!(!still_starting(Some(true), false, early));
    }
}
impl Spec {
    /// What people call this process; `key` holds internal ids and never reaches the activity feed.
    fn label(&self) -> &'static str {
        match self.role {
            Role::Caddy => "The website server",
            Role::App => "The application",
            Role::Connector => "The Cloudflare connector",
        }
    }
}
fn retained_static(store: &Store, spec: &Spec) -> bool {
    spec.cwd.is_none()
        && spec.env.is_empty()
        && spec
            .site
            .as_deref()
            .and_then(|id| store.site(id).ok())
            .is_some_and(|site| {
                matches!(
                    site.publish_state.as_deref(),
                    Some(
                        "published"
                            | "waiting_dns"
                            | "waiting_tls"
                            | "verifying_public"
                            | "recovery_required"
                    )
                )
            })
}
#[derive(Default)]
struct CrashLoop {
    short_exits: u32,
}
impl CrashLoop {
    fn php_exit(&mut self, duration: Duration, clean: bool) -> bool {
        !clean && self.exited(duration)
    }
    fn exited(&mut self, duration: Duration) -> bool {
        self.short_exits = if duration < Duration::from_secs(30) {
            self.short_exits.saturating_add(1)
        } else {
            0
        };
        self.short_exits >= 5
    }
}
struct Worker {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
pub struct Supervisor {
    pub store: Arc<Store>,
    workers: Arc<Mutex<HashMap<String, Worker>>>,
    scope: Option<String>,
    info: Arc<std::sync::Mutex<HashMap<String, ProcessInfo>>>,
    #[cfg(windows)]
    job: Arc<Job>,
}
impl Supervisor {
    pub fn for_site(&self, id: &str) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            workers: self.workers.clone(),
            scope: Some(id.into()),
            info: self.info.clone(),
            #[cfg(windows)]
            job: self.job.clone(),
        })
    }
    async fn launch_tunnel(&self, token: &str) -> Result<(String, bool), String> {
        let resources = {
            let db = self.store.db().map_err(|e| e.to_string())?;
            let mut st=db.prepare("SELECT created_resources_json FROM deployments WHERE state IN ('running','validating','starting_local','provisioning_tunnel','configuring_dns','starting_connector','waiting_dns','waiting_tls','verifying_public','recovery_required','published')").map_err(|e|e.to_string())?;
            let rows = st
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?
        };
        let secrets = WindowsCredentialStore;
        let mut id = None;
        for r in resources {
            let r: serde_json::Value = serde_json::from_str(&r).map_err(|e| e.to_string())?;
            if let Some(t) = r["tunnel_id"].as_str() {
                if secrets
                    .get(&format!("tunnel:{t}"))
                    .map_err(|e| e.to_string())?
                    .as_deref()
                    == Some(token)
                {
                    id = Some(t.to_owned());
                    break;
                }
            }
        }
        let id = id.ok_or("Tunnel credential is not associated with a deployment")?;
        let started = self.launch_connector(&id, token).await?;
        Ok((id, started))
    }
    async fn launch_connector(&self, id: &str, token: &str) -> Result<bool, String> {
        let port = metrics_port(&id);
        let started = self
            .launch(Spec {
                key: format!("tunnel:{id}"),
                site: None,
                exe: self
                    .store
                    .root
                    .join("bin")
                    .join(hostify_core::paths::CLOUDFLARED_EXE),
                args: vec![
                    "tunnel".into(),
                    "--no-autoupdate".into(),
                    "--metrics".into(),
                    format!("127.0.0.1:{port}"),
                    "run".into(),
                ],
                token: Some(token.into()),
                cwd: None,
                env: vec![],
                redact: vec![],
                log: self
                    .store
                    .root
                    .join("logs")
                    .join(format!("tunnel-{id}.log")),
                port,
                path: "/ready".into(),
                role: Role::Connector,
                health: Health::Strict2xx,
                restart: RestartPolicy::Always,
                memory_limit_mb: None,
            })
            .await?;
        Ok(started)
    }
    pub async fn stop_site_tunnel(&self, id: &str) -> Result<(), String> {
        self.for_site(id).stop_tunnel().await
    }
    pub fn new(store: Arc<Store>) -> io::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            store,
            workers: Arc::new(Mutex::new(HashMap::new())),
            scope: None,
            info: Arc::new(std::sync::Mutex::new(HashMap::new())),
            #[cfg(windows)]
            job: Arc::new(Job::new()?),
        }))
    }
    pub(crate) fn spawn(&self, spec: &Spec) -> io::Result<ManagedChild> {
        let mut cmd = Command::new(&spec.exe);
        cmd.args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if spec.cwd.is_some() || !spec.env.is_empty() {
            cmd.env_clear();
            for name in [
                "SystemRoot",
                "SystemDrive",
                "WINDIR",
                "ComSpec",
                "PATHEXT",
                "TEMP",
                "TMP",
                "USERPROFILE",
                "APPDATA",
                "LOCALAPPDATA",
                "PROGRAMDATA",
                "NUMBER_OF_PROCESSORS",
                "PROCESSOR_ARCHITECTURE",
                "OS",
            ] {
                if let Some(value) = std::env::var_os(name) {
                    cmd.env(name, value);
                }
            }
            cmd.envs(spec.env.iter().map(|(key, value)| (key, value)));
        }
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        if let Some(token) = &spec.token {
            cmd.env("TUNNEL_TOKEN", token);
        } else {
            cmd.env_remove("TUNNEL_TOKEN");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(if spec.role == Role::App {
                0x00000010 | 0x00000004
            } else {
                0x08000000 | 0x00000004
            });
        }
        #[cfg(windows)]
        let process_job = Arc::new(Job::with_memory_limit(spec.memory_limit_mb)?);
        let mut child = cmd.spawn()?;
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            #[link(name = "ntdll")]
            unsafe extern "system" {
                fn NtResumeProcess(handle: *mut std::ffi::c_void) -> i32;
            }
            let handle = child.as_raw_handle();
            if let Err(e) = self.job.assign(handle) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
            if let Err(e) = process_job.assign(handle) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
            if unsafe { NtResumeProcess(handle) } < 0 {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other("Could not resume managed child"));
            }
        }
        let mut pumps = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            pumps.push(pump_redacted(
                stdout,
                spec.log.clone(),
                spec.token.clone(),
                spec.redact.clone(),
            ));
        }
        if let Some(stderr) = child.stderr.take() {
            pumps.push(pump_redacted(
                stderr,
                spec.log.clone(),
                spec.token.clone(),
                spec.redact.clone(),
            ));
        }
        Ok(ManagedChild {
            child,
            pumps,
            #[cfg(windows)]
            job: process_job,
        })
    }
    async fn launch(self: &Self, spec: Spec) -> Result<bool, String> {
        let mut workers = self.workers.lock().await;
        if workers
            .get(&spec.key)
            .is_some_and(|worker| !worker.task.is_finished())
        {
            return Ok(false);
        }
        workers.remove(&spec.key);
        if !port_available(spec.port) {
            return Err(format!(
                "Port {} is already in use. Stop the other application and retry.",
                spec.port
            ));
        }
        let child = self.spawn(&spec).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound && spec.role == Role::Caddy {
                hostify_core::components::CADDY_UNAVAILABLE.into()
            } else if spec.role == Role::App && e.kind() == io::ErrorKind::NotFound {
                let version = spec
                    .site
                    .as_deref()
                    .and_then(|id| self.store.site(id).ok())
                    .and_then(|site| site.runtime_version)
                    .unwrap_or_else(|| "the selected version".into());
                format!("Node {version} is unavailable. Install it on the Runtimes page.")
            } else {
                format!(
                    "{} could not start: {e}. Check Settings > Components.",
                    spec.label()
                )
            }
        })?;
        let info = self.info.clone();
        if let Ok(mut all) = info.lock() {
            all.insert(spec.key.clone(), ProcessInfo::new(&child));
        }
        if let Some(id) = &spec.site {
            let _ = self.store.process_status(id, &spec.key, "starting", None);
        }
        let (stop, mut rx) = watch::channel(false);
        let store = self.store.clone();
        #[cfg(windows)]
        let job = self.job.clone();
        let key = spec.key.clone();
        let task = tokio::spawn(async move {
            let mut child = Some(child);
            let mut failures = 0u32;
            let mut quiet_restart = false;
            let mut crash_loop = CrashLoop::default();
            let mut next = tokio::time::Instant::now();
            let mut started = tokio::time::Instant::now();
            let mut healthy = None;
            let mut checked_exposure = false;
            let mut last_probe = tokio::time::Instant::now();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {_ = rx.changed()=>{break;},_=tick.tick()=>{}}
                if *rx.borrow() {
                    break;
                }
                if let Some(process) = child.as_mut() {
                    match process.try_wait() {
                        Ok(None) => {
                            // A health path can touch a database, so a healthy app is asked less
                            // often. Process exit is still noticed every second above.
                            if (spec.role == Role::App
                                || (spec.role == Role::Caddy
                                    && spec.site.as_deref().is_some_and(|id| {
                                        store.site(id).is_ok_and(|s| s.kind == "php")
                                    })))
                                && healthy == Some(true)
                                && last_probe.elapsed() < Duration::from_secs(10)
                            {
                                continue;
                            }
                            last_probe = tokio::time::Instant::now();
                            let mut ok = http_health(spec.port, &spec.path, spec.health).await;
                            if spec.role == Role::Caddy
                                && let Some(id) = &spec.site
                                && store.site(id).is_ok_and(|s| s.kind == "php")
                                && let Ok(c) = store.app_config(id)
                            {
                                ok &= http_health(spec.port, &c.health_path, Health::AnyBelow500)
                                    .await;
                            }
                            if still_starting(healthy, ok, started.elapsed()) {
                                continue;
                            }
                            if ok && spec.role == Role::App && !checked_exposure {
                                #[cfg(windows)]
                                if let Ok(addresses) = process
                                    .job
                                    .process_ids()
                                    .and_then(|ids| crate::windows::exposed_addresses(&ids))
                                {
                                    for address in &addresses {
                                        let _=store.activity("warn",spec.site.as_deref(),&format!("The application explicitly listens on {address}, which may be reachable from other computers."));
                                    }
                                    if let Ok(mut all) = info.lock()
                                        && let Some(i) = all.get_mut(&spec.key)
                                    {
                                        i.exposed_addresses = addresses;
                                    }
                                }
                                checked_exposure = true;
                            }
                            if healthy != Some(ok) {
                                // A first pass is the normal start, so only a change of health is news.
                                let recovered = healthy == Some(false);
                                healthy = Some(ok);
                                if let Some(id) = &spec.site {
                                    let _ = store.process_status(
                                        id,
                                        &spec.key,
                                        if ok { "running" } else { "error" },
                                        if ok {
                                            None
                                        } else {
                                            Some(format!(
                                                "{} isn't responding. Hostify keeps checking.",
                                                spec.label()
                                            ))
                                        },
                                    );
                                }
                                if !ok {
                                    let _ = store.activity(
                                        "warn",
                                        spec.site.as_deref(),
                                        &format!(
                                            "{} isn't responding. Hostify keeps checking.",
                                            spec.label()
                                        ),
                                    );
                                } else if recovered {
                                    let _ = store.activity(
                                        "info",
                                        spec.site.as_deref(),
                                        &format!("{} is responding again", spec.label()),
                                    );
                                }
                            }
                            if started.elapsed() > Duration::from_secs(60) && ok {
                                failures = 0
                            }
                            continue;
                        }
                        result => {
                            let code = match result {
                                Ok(Some(status)) => status
                                    .code()
                                    .map(|c| c.to_string())
                                    .unwrap_or_else(|| "unknown".into()),
                                Err(e) => format!("unknown ({e})"),
                                _ => unreachable!(),
                            };
                            let last_exit = if spec.health == Health::TcpConnect
                                && code == "-1073741515"
                            {
                                "The Microsoft Visual C++ runtime is absent. Install Microsoft's Visual C++ Redistributable for x64 from https://aka.ms/vs/17/release/vc_redist.x64.exe, then retry.".to_owned()
                            } else {
                                format!("exit code {code}")
                            };
                            #[cfg(windows)]
                            let _ = process.job.terminate();
                            if let Ok(mut all) = info.lock()
                                && let Some(i) = all.get_mut(&spec.key)
                            {
                                i.last_exit_code = code.parse().ok();
                                i.last_exit_at = Some(chrono::Utc::now().to_rfc3339());
                                i.pid = None;
                            }
                            let _ = process.kill();
                            let _ = process.wait();
                            process.finish_output().await;
                            child = None;
                            let recycling = spec.health == Health::TcpConnect && code == "0";
                            quiet_restart = recycling;
                            if spec.restart == RestartPolicy::Never
                                || (spec.restart == RestartPolicy::OnFailure
                                    && code == "0"
                                    && !recycling)
                            {
                                let message = format!("{} stopped ({last_exit}).", spec.label());
                                if let Some(id) = &spec.site {
                                    let _ = store.process_status(
                                        id,
                                        &spec.key,
                                        "stopped",
                                        Some(message.clone()),
                                    );
                                }
                                if !recycling {
                                    let _ = store.activity("info", spec.site.as_deref(), &message);
                                }
                                break;
                            }
                            if crash_loop.php_exit(started.elapsed(), recycling)
                                && spec.site.is_some()
                                && !(spec.role == Role::Caddy && retained_static(&store, &spec))
                            {
                                let message = format!(
                                    "{} stopped after five exits within 30 seconds of starting ({last_exit}). Start the website manually to try again.",
                                    spec.label()
                                );
                                if let Some(id) = &spec.site {
                                    let _ = store.process_status(
                                        id,
                                        &spec.key,
                                        "error",
                                        Some(message.clone()),
                                    );
                                }
                                let _ = store.activity("error", spec.site.as_deref(), &message);
                                break;
                            }
                            if !recycling {
                                failures = failures.saturating_add(1);
                                next = tokio::time::Instant::now() + restart_delay(failures);
                                if let Some(id) = &spec.site {
                                    let _ = store.process_status(
                                    id, &spec.key,
                                    "error",
                                    Some(format!(
                                        "{} stopped unexpectedly ({last_exit}). Restarting in {} seconds.",
                                        spec.label(),
                                        restart_delay(failures).as_secs()
                                    )),
                                );
                                }
                                let _ = store.activity(
                                "warn",
                                spec.site.as_deref(),
                                &format!(
                                    "{} stopped unexpectedly ({last_exit}). Restarting in {} seconds.",
                                    spec.label(),
                                    restart_delay(failures).as_secs()
                                ),
                            );
                            } else {
                                next = tokio::time::Instant::now();
                            }
                        }
                    }
                }
                if tokio::time::Instant::now() < next {
                    continue;
                }
                if !port_available(spec.port) {
                    if let Some(id) = &spec.site {
                        let _ = store.process_status(
                            id,
                            &spec.key,
                            "error",
                            Some(format!(
                                "Port {} is already in use. Stop the other application and retry.",
                                spec.port
                            )),
                        );
                    }
                    next = tokio::time::Instant::now() + restart_delay(failures.saturating_add(1));
                    continue;
                }
                // Reuse the same Job Object for every restarted process.
                let temporary = Supervisor {
                    store: store.clone(),
                    workers: Arc::new(Mutex::new(HashMap::new())),
                    scope: None,
                    info: info.clone(),
                    #[cfg(windows)]
                    job: job.clone(),
                };
                let preflight = spec.role != Role::Caddy || store.detection.caddy_ready().await;
                if !preflight {
                    if let Some(id) = &spec.site {
                        let _ = store.process_status(
                            id,
                            &spec.key,
                            "error",
                            Some(hostify_core::components::CADDY_UNAVAILABLE.into()),
                        );
                    }
                    next = tokio::time::Instant::now() + restart_delay(failures.saturating_add(1));
                    continue;
                }
                match temporary.spawn(&spec) {
                    Ok(c) => {
                        if let Ok(mut all) = info.lock() {
                            let i = all
                                .entry(spec.key.clone())
                                .or_insert_with(|| ProcessInfo::new(&c));
                            i.pid = Some(c.id());
                            #[cfg(windows)]
                            {
                                i.job = c.job.clone();
                            }
                            i.restarts += 1;
                        }
                        child = Some(c);
                        started = tokio::time::Instant::now();
                        healthy = None;
                        checked_exposure = false;
                        if !quiet_restart {
                            let _ = store.activity(
                                "info",
                                spec.site.as_deref(),
                                &format!("{} restarted", spec.label()),
                            );
                        }
                    }
                    Err(e) => {
                        failures = failures.saturating_add(1);
                        next = tokio::time::Instant::now() + restart_delay(failures);
                        let message =
                            if e.kind() == io::ErrorKind::NotFound && spec.role == Role::Caddy {
                                hostify_core::components::CADDY_UNAVAILABLE
                            } else {
                                "Hostify could not restart this process and will try again"
                            };
                        if let Some(id) = &spec.site {
                            let _ =
                                store.process_status(id, &spec.key, "error", Some(message.into()));
                        }
                        let _ = store.activity("error", spec.site.as_deref(), message);
                    }
                }
            }
            if let Some(mut c) = child {
                if spec.role == Role::App {
                    #[cfg(windows)]
                    {
                        let j = c.job.clone();
                        let pid = c.id();
                        let seconds = if spec.health == Health::TcpConnect {
                            3
                        } else {
                            10
                        };
                        let graceful = tokio::task::spawn_blocking(move || {
                            crate::windows::graceful_stop_timeout(
                                &j,
                                pid,
                                Duration::from_secs(seconds),
                            )
                        })
                        .await
                        .unwrap_or(false);
                        let _=store.activity("info",spec.site.as_deref(),if graceful {"The application stopped gracefully."}else{"The application was terminated after graceful stop was unavailable or timed out."});
                    }
                }
                let _ = c.kill();
                let _ = c.wait();
            }
        });
        workers.insert(key, Worker { stop, task });
        Ok(true)
    }
    async fn stop(&self, key: &str) -> Result<(), String> {
        let worker = self.workers.lock().await.remove(key);
        if let Some(w) = worker {
            let _ = w.stop.send(true);
            w.task.await.map_err(|e| e.to_string())?;
        }
        if let Ok(mut all) = self.info.lock()
            && let Some(i) = all.get_mut(key)
        {
            i.pid = None;
            i.exposed_addresses.clear();
        }
        if let Some(id) = key
            .strip_prefix("app:")
            .or_else(|| key.strip_prefix("site:"))
        {
            let exists = self
                .store
                .process_health
                .lock()
                .map(|all| all.get(id).is_some_and(|p| p.contains_key(key)))
                .unwrap_or(false);
            if exists {
                let _ = self.store.process_status(id, key, "stopped", None);
            }
        }
        Ok(())
    }
    pub async fn shutdown(&self) {
        let keys = self
            .workers
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            let _ = self.stop(&key).await;
        }
    }
}
#[cfg(test)]
fn pump(
    input: impl std::io::Read + Send + 'static,
    path: PathBuf,
    secret: Option<String>,
) -> std::thread::JoinHandle<()> {
    pump_redacted(input, path, secret, vec![])
}
pub(crate) fn pump_redacted(
    input: impl std::io::Read + Send + 'static,
    path: PathBuf,
    secret: Option<String>,
    mut redact: Vec<String>,
) -> std::thread::JoinHandle<()> {
    if let Some(value) = &secret {
        redact.push(value.clone());
    }
    redact.retain(|v| !v.is_empty());
    redact.sort_by_key(|v| std::cmp::Reverse(v.len()));
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(input).lines() {
            match line {
                Ok(mut line) => {
                    for value in &redact {
                        line = line.replace(value, "[redacted]");
                    }
                    let _ = append_log(&path, &line);
                }
                Err(_) => break,
            }
        }
    })
}
#[async_trait]
impl SiteRuntime for Supervisor {
    async fn start_site(&self, id: &str) -> Result<bool, String> {
        if matches!(
            self.store
                .site(id)
                .map_err(|e| e.to_string())?
                .kind
                .as_str(),
            "node" | "php"
        ) {
            return self.start_node(id).await;
        }
        if !self.store.detection.caddy_ready().await {
            let message = hostify_core::components::CADDY_UNAVAILABLE.to_owned();
            let _ = self.store.process_status(
                id,
                &format!("site:{id}"),
                "error",
                Some(message.clone()),
            );
            return Err(message);
        }
        std::fs::create_dir_all(self.store.root.join("run/verify").join(id))
            .map_err(|e| e.to_string())?;
        let site = self.store.site(id).map_err(|e| e.to_string())?;
        let config = hostify_core::caddy::generate(&[hostify_core::caddy::Site {
            verify_root: self.store.root.join("run/verify").join(id),
            port: site.local_port,
            backend: hostify_core::caddy::Backend::Static {
                root: self.store.root.join("sites").join(id).join("current"),
            },
            log: self.store.root.join("logs").join(format!("{id}.log")),
        }])
        .map_err(|e| e.to_string())?;
        let path = self.store.root.join("run").join(format!("{id}.Caddyfile"));
        std::fs::write(&path, config).map_err(|e| e.to_string())?;
        let result = self
            .launch(Spec {
                key: format!("site:{id}"),
                site: Some(id.into()),
                exe: self
                    .store
                    .root
                    .join("bin")
                    .join(hostify_core::paths::CADDY_EXE),
                args: vec![
                    "run".into(),
                    "--config".into(),
                    path.to_string_lossy().into_owned(),
                    "--adapter".into(),
                    "caddyfile".into(),
                ],
                token: None,
                cwd: None,
                env: vec![],
                redact: vec![],
                log: self
                    .store
                    .root
                    .join("logs")
                    .join(format!("{id}.process.log")),
                port: site.local_port,
                path: "/".into(),
                role: Role::Caddy,
                health: Health::Strict2xx,
                restart: RestartPolicy::Always,
                memory_limit_mb: None,
            })
            .await;
        match result {
            Err(e) => {
                let _ = self.store.status(id, "error", Some(e.clone()));
                Err(e)
            }
            Ok(false) => Ok(false),
            Ok(true) => {
                if self.site_responds(site.local_port).await? {
                    self.store
                        .process_status(id, &format!("site:{id}"), "running", None)
                        .map_err(|e| e.to_string())?;
                    Ok(true)
                } else {
                    let _ = self.stop(&format!("site:{id}")).await;
                    let message = format!(
                        "Website did not become healthy on port {}. Check its logs and whether another application took the port.",
                        site.local_port
                    );
                    let _ = self.store.process_status(
                        id,
                        &format!("site:{id}"),
                        "error",
                        Some(message.clone()),
                    );
                    Err(message)
                }
            }
        }
    }
    async fn stop_site(&self, id: &str) -> Result<(), String> {
        self.stop(&format!("site:{id}")).await?;
        self.stop(&format!("app:{id}")).await?;
        let prefix = format!("php:{id}:");
        let keys = self
            .workers
            .lock()
            .await
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            self.stop(&key).await?;
        }
        if let Ok(mut health) = self.store.process_health.lock()
            && let Some(p) = health.get_mut(id)
        {
            p.retain(|key, _| !key.starts_with(&prefix));
        }
        self.store
            .status(id, "stopped", None)
            .map_err(|e| e.to_string())
    }
    async fn resume_tunnel(&self, id: &str, token: &str) -> Result<bool, String> {
        self.launch_connector(id, token).await
    }
    async fn start_tunnel(&self, token: &str) -> Result<bool, String> {
        let (id, started) = self.launch_tunnel(token).await?;
        let port = metrics_port(&id);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if http_ok(port, "/ready").await && tunnel_connections(port).await > 0 {
                return Ok(started);
            }
            if tokio::time::Instant::now() >= deadline {
                if started {
                    self.stop(&format!("tunnel:{id}")).await?;
                }
                return Err(
                    "Cloudflare connector did not connect. Check tunnel logs and network access."
                        .into(),
                );
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    async fn stop_owned_tunnel(&self, id: &str) -> Result<(), String> {
        self.stop(&format!("tunnel:{id}")).await
    }
    async fn stop_tunnel(&self) -> Result<(), String> {
        let allowed = if let Some(site) = &self.scope {
            let db = self.store.db().map_err(|e| e.to_string())?;
            let mut st = db
                .prepare("SELECT created_resources_json FROM deployments WHERE site_id=?1")
                .map_err(|e| e.to_string())?;
            let rows = st
                .query_map([site], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            let mut ids = Vec::new();
            for r in rows {
                let r: serde_json::Value = serde_json::from_str(&r.map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                if let Some(id) = r["tunnel_id"].as_str() {
                    ids.push(format!("tunnel:{id}"));
                }
            }
            Some(ids)
        } else {
            None
        };
        let keys = self
            .workers
            .lock()
            .await
            .keys()
            .filter(|k| {
                k.starts_with("tunnel:") && allowed.as_ref().is_none_or(|ids| ids.contains(k))
            })
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            self.stop(&key).await?
        }
        Ok(())
    }
    async fn connector_health(&self, id: &str) -> Result<(bool, u64), String> {
        let port = metrics_port(id);
        Ok((
            http_ok(port, "/ready").await,
            tunnel_connections(port).await,
        ))
    }
    async fn site_responds(&self, port: u16) -> Result<bool, String> {
        let node = self
            .store
            .sites()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|s| s.local_port == port && s.kind == "node");
        // An app's home page may legitimately fail or redirect; its chosen health path decides.
        let (path, health) = match node {
            Some(site) => (
                self.store
                    .app_config(&site.id)
                    .map_err(|e| e.to_string())?
                    .health_path,
                Health::AnyBelow500,
            ),
            None => ("/".to_owned(), Health::Strict2xx),
        };
        for _ in 0..20 {
            if http_health(port, &path, health).await {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok(false)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotation_tail() {
        let dir = tempfile::tempdir().expect("temp");
        let p = dir.path().join("log");
        std::fs::write(&p, "x".repeat(LOG_LIMIT as usize)).expect("write");
        append_log(&p, "new").expect("rotate");
        assert!(dir.path().join("log.1").exists());
        assert_eq!(tail_logs(&p, 1).expect("tail"), vec!["new"]);
    }
}

#[cfg(test)]
mod secrecy_tests {
    use super::*;
    #[test]
    fn realistic_cloudflared_error_redacts_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tunnel-test.log");
        let token = "eyJhIjoiYWNjb3VudCIsInQiOiJ0dW5uZWwiLCJzIjoic2VjcmV0In0=";
        let error = format!(
            "2026-10-30T09:00:00Z ERR Unable to establish connection with Cloudflare edge error=\"Unauthorized: tunnel token {token} is invalid\" connIndex=0 event=0 ip=198.41.192.167\n"
        );
        pump(
            std::io::Cursor::new(error.into_bytes()),
            path.clone(),
            Some(token.into()),
        )
        .join()
        .unwrap();
        let logged = std::fs::read_to_string(path).unwrap();
        assert!(!logged.contains(token));
        assert!(logged.contains("[redacted]"));
        assert!(logged.contains("connIndex=0"));
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[tokio::test]
    async fn startup_connector_returns_offline_and_keeps_supervised_worker() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("data")).unwrap();
        // The test harness exits on cloudflared arguments; the worker must remain to retry it.
        std::fs::copy(
            std::env::current_exe().unwrap(),
            store
                .root
                .join("bin")
                .join(hostify_core::paths::CLOUDFLARED_EXE),
        )
        .unwrap();
        let supervisor = Supervisor::new(store).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        tokio::time::timeout(
            Duration::from_secs(3),
            supervisor.resume_tunnel(&id, "fake-token"),
        )
        .await
        .unwrap()
        .unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(
            supervisor
                .workers
                .lock()
                .await
                .contains_key(&format!("tunnel:{id}"))
        );
        assert!(!supervisor.resume_tunnel(&id, "fake-token").await.unwrap());
        supervisor.shutdown().await;
        assert!(supervisor.workers.lock().await.is_empty());
    }
}

#[cfg(all(test, windows))]
mod ownership_tests {
    use super::*;
    #[tokio::test]
    async fn closing_job_kills_child_and_foreign_port_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("data")).unwrap();
        let supervisor = Supervisor::new(store).unwrap();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut spec = Spec {
            key: "test".into(),
            site: None,
            exe: std::env::current_exe().unwrap(),
            args: vec![
                "--exact".into(),
                "supervisor::ownership_tests::waiting_child_fixture".into(),
            ],
            token: None,
            cwd: None,
            env: vec![],
            redact: vec![],
            log: dir.path().join("child.log"),
            port,
            path: "/".into(),
            role: Role::Caddy,
            health: Health::Strict2xx,
            restart: RestartPolicy::Always,
            memory_limit_mb: None,
        };
        assert!(
            supervisor
                .launch(spec.clone())
                .await
                .unwrap_err()
                .contains("already in use")
        );
        assert!(listener.local_addr().is_ok());
        drop(listener);
        // Reuse the pump's token environment mechanism to select a separate waiting fixture.
        spec.args = vec![
            "--exact".into(),
            "supervisor::ownership_tests::waiting_child_fixture".into(),
        ];
        spec.token = Some("test".into());
        let mut child = supervisor.spawn(&spec).unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(child.try_wait().unwrap().is_none());
        drop(supervisor);
        tokio::time::timeout(Duration::from_secs(3), async {
            while child.try_wait().unwrap().is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let _ = child.wait();
    }
    #[test]
    fn waiting_child_fixture() {
        if std::env::var("TUNNEL_TOKEN").as_deref() == Ok("test") {
            std::thread::sleep(Duration::from_secs(30));
        }
    }
}
#[cfg(test)]
mod runtime_groundwork_tests {
    use super::*;
    #[test]
    fn rapid_exits_reset_after_stable_run_and_redaction_handles_overlaps() {
        let mut crashes = CrashLoop::default();
        for _ in 0..4 {
            assert!(!crashes.exited(Duration::from_millis(10)));
        }
        assert!(!crashes.exited(Duration::from_secs(30)));
        for _ in 0..4 {
            assert!(!crashes.exited(Duration::from_millis(10)));
        }
        assert!(crashes.exited(Duration::from_millis(10)));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        pump_redacted(
            std::io::Cursor::new(b"token abcdef abc\n".to_vec()),
            path.clone(),
            Some("token".into()),
            vec!["abc".into(), "abcdef".into()],
        )
        .join()
        .unwrap();
        assert_eq!(
            tail_logs(&path, 1).unwrap(),
            ["[redacted] [redacted] [redacted]"]
        );
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn real_short_lived_child_hits_limit_and_manual_start_resets() {
        struct Ready;
        impl hostify_core::components::ProbeRunner for Ready {
            fn version(&self, _: &Path, _: &str) -> Result<String, String> {
                Ok("test version".into())
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open_with_probe(tmp.path().into(), Arc::new(Ready)).unwrap();
        std::fs::write(
            tmp.path().join("bin/caddy.exe"),
            b"probe fixture, never executed",
        )
        .unwrap();
        store.db().unwrap().execute("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','s','x',9010,'now','now')",[]).unwrap();
        let supervisor = Supervisor::new(store.clone()).unwrap();
        let spec = Spec {
            key: "site:s".into(),
            site: Some("s".into()),
            exe: PathBuf::from(std::env::var_os("ComSpec").unwrap()),
            args: vec!["/D".into(), "/C".into(), "exit 7".into()],
            token: None,
            cwd: Some(tmp.path().into()),
            env: vec![("HOSTIFY_TEST".into(), "ok".into())],
            redact: vec![],
            log: tmp.path().join("child.log"),
            port: std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port(),
            path: "/".into(),
            role: Role::Caddy,
            health: Health::Strict2xx,
            restart: RestartPolicy::Always,
            memory_limit_mb: None,
        };
        assert!(supervisor.launch(spec.clone()).await.unwrap());
        tokio::time::timeout(Duration::from_secs(40), async {
            loop {
                if store
                    .site("s")
                    .unwrap()
                    .status_detail
                    .unwrap_or_default()
                    .contains("five exits")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        let detail = store.site("s").unwrap().status_detail.unwrap();
        assert!(detail.contains("exit code 7"));
        let message: String = store
            .db()
            .unwrap()
            .query_row(
                "SELECT message FROM activity_logs ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(message.contains("exit code 7"));
        assert!(supervisor.launch(spec).await.unwrap());
        supervisor.shutdown().await;
    }
}

pub(crate) struct ManagedChild {
    child: Child,
    pumps: Vec<std::thread::JoinHandle<()>>,
    #[cfg(windows)]
    pub(crate) job: Arc<Job>,
}
impl std::ops::Deref for ManagedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}
impl std::ops::DerefMut for ManagedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        #[cfg(windows)]
        let _ = self.job.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
struct ProcessInfo {
    pid: Option<u32>,
    #[cfg(windows)]
    job: Arc<Job>,
    restarts: u64,
    last_exit_code: Option<i32>,
    last_exit_at: Option<String>,
    exposed_addresses: Vec<String>,
}
impl ProcessInfo {
    fn new(child: &ManagedChild) -> Self {
        Self {
            pid: Some(child.id()),
            #[cfg(windows)]
            job: child.job.clone(),
            restarts: 0,
            last_exit_code: None,
            last_exit_at: None,
            exposed_addresses: Vec::new(),
        }
    }
}

impl ManagedChild {
    pub(crate) fn terminate_tree(&mut self) {
        #[cfg(windows)]
        let _ = self.job.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
    pub(crate) async fn finish_output(&mut self) {
        let pumps = std::mem::take(&mut self.pumps);
        let _ = tokio::task::spawn_blocking(move || {
            for pump in pumps {
                let _ = pump.join();
            }
        })
        .await;
    }
}
impl Supervisor {
    fn release_root(&self, id: &str, release: &str) -> Result<PathBuf, String> {
        uuid::Uuid::parse_str(release).map_err(|_| "The release ID is invalid.")?;
        Ok(self
            .store
            .root
            .join("sites")
            .join(id)
            .join("app")
            .join(release))
    }
    pub fn app_status(&self, id: &str) -> Result<Value, String> {
        let config = self.store.app_config(id).map_err(|e| e.to_string())?;
        let php = self.store.site(id).map_err(|e| e.to_string())?.kind == "php";
        let health = self
            .store
            .process_health
            .lock()
            .map_err(|_| "Process health unavailable.")?
            .get(id)
            .cloned()
            .unwrap_or_default();
        let mut value = json!({"health": if php { self.store.site(id).map_err(|e| e.to_string())?.status } else { health.get(&format!("app:{id}")).map(|p| p.0.clone()).unwrap_or_else(|| "stopped".into()) },"exposedAddresses":[]});
        if let Some(port) = config.internal_port {
            value["internalPort"] = json!(port);
        }
        if let Some(release) = config.active_release {
            value["activeRelease"] = json!(release);
        }
        let all = self
            .info
            .lock()
            .map_err(|_| "Process information unavailable.")?;
        // Keep Node's optional statistics absent until its first launch.
        if !php && !all.contains_key(&format!("app:{id}")) {
            return Ok(value);
        }
        let keys = if php {
            (0..config.php_ports.len())
                .map(|n| format!("php:{id}:{n}"))
                .collect::<Vec<_>>()
        } else {
            vec![format!("app:{id}")]
        };
        let mut restarts = 0u64;
        let mut workers = Vec::new();
        #[cfg(windows)]
        let mut ids = Vec::new();
        #[cfg(windows)]
        let mut peak = 0u64;
        let mut latest = None;
        for (n, key) in keys.iter().enumerate() {
            let info = all.get(key);
            if php {
                workers.push(json!({"port":config.php_ports[n],"pid": info.and_then(|i| i.pid),"health":health.get(key).map(|p| p.0.as_str()).unwrap_or("stopped")}));
            }
            if let Some(i) = info {
                if !php {
                    if let Some(pid) = i.pid {
                        value["pid"] = json!(pid);
                    }
                    value["exposedAddresses"] = json!(i.exposed_addresses);
                }
                restarts += i.restarts;
                if let Some(at) = &i.last_exit_at
                    && latest.as_ref().is_none_or(|(last, _)| at > last)
                {
                    latest = Some((at.clone(), i.last_exit_code));
                }
                #[cfg(windows)]
                {
                    ids.extend(i.job.process_ids().map_err(|e| e.to_string())?);
                    peak += i.job.peak_memory().map_err(|e| e.to_string())?;
                }
            }
        }
        value["restarts"] = json!(restarts);
        if let Some((at, code)) = latest {
            value["lastExitAt"] = json!(at);
            value["lastExitCode"] = json!(code);
        }
        if php {
            value["workers"] = json!(workers);
        }
        #[cfg(windows)]
        {
            ids.sort_unstable();
            ids.dedup();
            value["processCount"] = json!(ids.len());
            let pids = ids
                .iter()
                .map(|id| sysinfo::Pid::from_u32(*id))
                .collect::<Vec<_>>();
            let mut system = sysinfo::System::new();
            system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&pids), true);
            value["memoryBytes"] = json!(
                pids.iter()
                    .filter_map(|p| system.process(*p))
                    .map(|p| p.memory())
                    .sum::<u64>()
            );
            value["peakMemoryBytes"] = json!(peak);
            let mut cpu = 0.0;
            if !pids.is_empty() {
                std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
                system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&pids), true);
                cpu = pids
                    .iter()
                    .filter_map(|p| system.process(*p))
                    .map(|p| p.cpu_usage())
                    .sum::<f32>()
                    / std::thread::available_parallelism()
                        .map(|n| n.get())
                        .unwrap_or(1) as f32;
            }
            if php || !pids.is_empty() {
                value["cpuPercent"] = json!(cpu);
            }
        }
        Ok(value)
    }
    async fn wait_app(&self, id: &str, port: u16, path: &str) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if http_health(port, path, Health::AnyBelow500).await {
                return Ok(());
            }
            {
                let all = self
                    .info
                    .lock()
                    .map_err(|_| "Process information unavailable.")?;
                if let Some(i) = all.get(&format!("app:{id}"))
                    && i.pid.is_none()
                {
                    let lines = tail_logs(
                        &self.store.root.join("logs").join(format!("{id}.app.log")),
                        20,
                    )
                    .unwrap_or_default()
                    .join("\n");
                    return Err(format!(
                        "The application exited with code {} before becoming healthy.\n{lines}",
                        i.last_exit_code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "unknown".into())
                    ));
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("The application did not become healthy within 60 seconds. Check its application log.".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn start_node_release(
        &self,
        id: &str,
        config: &mut hostify_core::app_config::AppConfig,
        release: &str,
    ) -> Result<bool, String> {
        use hostify_core::app_config::{Start, inside};
        let root = self.release_root(id, release)?;
        let kind = self.store.site(id).map_err(|e| e.to_string())?.kind;
        config.validate_kind(&root, &kind)?;
        if kind == "php" {
            return self.start_php_release(id, config, &root).await;
        }
        let site = self.store.site(id).map_err(|e| e.to_string())?;
        let exe = self.store.runtimes.resolve(
            "node",
            site.runtime_version
                .as_deref()
                .ok_or("Choose a Node runtime version on the Runtimes page.")?,
        )?;
        let cwd = inside(&root, &config.working_dir)?;
        let key = format!("app:{id}");
        if self
            .workers
            .lock()
            .await
            .get(&key)
            .is_some_and(|w| !w.task.is_finished())
        {
            return Ok(false);
        }
        if config.internal_port.is_none_or(|p| !port_available(p)) {
            config.internal_port = Some(
                hostify_core::caddy::allocate_port(
                    self.store
                        .settings()
                        .map_err(|e| e.to_string())?
                        .base_port_start,
                    &self.store.reserved_ports().map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?,
            );
            self.store
                .save_app_config(id, config)
                .map_err(|e| e.to_string())?;
        }
        let port = config.internal_port.unwrap();
        let (mut env, redact) = crate::build::environment(&self.store, id, &exe, &cwd, false)?;
        let preload = self.store.root.join("run/hostify-loopback.cjs");
        // Another site's Node may be reading this file, so leave it alone when it is current.
        if std::fs::read_to_string(&preload).ok().as_deref() != Some(include_str!("loopback.cjs")) {
            std::fs::write(&preload, include_str!("loopback.cjs")).map_err(|e| e.to_string())?;
        }
        let options = env
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("NODE_OPTIONS"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        env.retain(|(n, _)| {
            !n.eq_ignore_ascii_case("NODE_OPTIONS")
                && !n.eq_ignore_ascii_case("PORT")
                && !n.eq_ignore_ascii_case("HOST")
        });
        env.extend([
            (
                "NODE_OPTIONS".into(),
                format!(
                    "--require \"{}\" {options}",
                    preload.to_string_lossy().replace('\\', "/")
                ),
            ),
            ("PORT".into(), port.to_string()),
            ("HOST".into(), "127.0.0.1".into()),
        ]);
        let args = match &config.start {
            Start::NpmScript { name } => vec![crate::build::npm(&exe), "run".into(), name.clone()],
            Start::Node { args } => {
                let mut argv = args
                    .iter()
                    .map(|a| a.replace("{PORT}", &port.to_string()))
                    .collect::<Vec<_>>();
                let entry = inside(&root, &format!("{}/{}", config.working_dir, argv[0]))?;
                if !entry.is_file() {
                    return Err("The Node entry path must name a file inside the release.".into());
                }
                argv[0] = entry.to_string_lossy().into_owned();
                argv
            }
        };
        self.launch(Spec {
            key: key.clone(),
            site: Some(id.into()),
            exe,
            args,
            token: None,
            cwd: Some(cwd),
            env,
            redact,
            log: self.store.root.join("logs").join(format!("{id}.app.log")),
            port,
            path: config.health_path.clone(),
            role: Role::App,
            health: Health::AnyBelow500,
            restart: config.restart,
            memory_limit_mb: config.memory_limit_mb,
        })
        .await?;
        self.wait_app(id, port, &config.health_path).await?;
        self.build_phase(
            id,
            "start",
            "done",
            "The application is healthy on its internal port.",
        )?;
        self.build_phase(
            id,
            "verify",
            "running",
            "Checking the application through the website server.",
        )?;
        self.check_exposure(id, &key)?;
        self.store
            .process_status(id, &key, "running", None)
            .map_err(|e| e.to_string())?;
        self.start_proxy(id, hostify_core::caddy::Backend::Proxy { port })
            .await?;
        if !http_health(site.local_port, &config.health_path, Health::AnyBelow500).await {
            return Err("The application did not respond through the website server with a public Host header.".into());
        }
        self.store
            .process_status(id, &format!("site:{id}"), "running", None)
            .map_err(|e| e.to_string())?;
        Ok(true)
    }
    async fn start_php_release(
        &self,
        id: &str,
        config: &mut hostify_core::app_config::AppConfig,
        root: &Path,
    ) -> Result<bool, String> {
        let prefix = format!("php:{id}:");
        if self
            .workers
            .lock()
            .await
            .iter()
            .any(|(k, w)| k.starts_with(&prefix) && !w.task.is_finished())
        {
            return Ok(false);
        }
        let site = self.store.site(id).map_err(|e| e.to_string())?;
        let exe = self.store.runtimes.resolve(
            "php",
            site.runtime_version
                .as_deref()
                .ok_or("Choose a PHP runtime version.")?,
        )?;
        let ini = crate::php::write(config, &self.store.root, id, &exe)?;
        let cwd = hostify_core::app_config::inside(root, &config.document_root)?;
        let mut reserved = self.store.reserved_ports().map_err(|e| e.to_string())?;
        let start = self
            .store
            .settings()
            .map_err(|e| e.to_string())?
            .base_port_start;
        config.php_ports.clear();
        for _ in 0..config.php_workers {
            let port =
                hostify_core::caddy::allocate_port(start, &reserved).map_err(|e| e.to_string())?;
            config.php_ports.push(port);
            reserved.push(port);
        }
        self.store
            .save_app_config(id, config)
            .map_err(|e| e.to_string())?;
        let (mut env, redact) = crate::build::environment(&self.store, id, &exe, &cwd, false)?;
        env.retain(|(n, _)| {
            !["PORT", "HOST", "PHP_FCGI_MAX_REQUESTS", "PHP_INI_SCAN_DIR"]
                .iter()
                .any(|key| n.eq_ignore_ascii_case(key))
        });
        env.extend([
            ("PHP_FCGI_MAX_REQUESTS".into(), "10000".into()),
            ("PHP_INI_SCAN_DIR".into(), String::new()),
        ]);
        for (n, port) in config.php_ports.iter().enumerate() {
            let key = format!("php:{id}:{n}");
            self.launch(Spec {
                key: key.clone(),
                site: Some(id.into()),
                exe: exe.clone(),
                args: vec![
                    "-c".into(),
                    ini.to_string_lossy().into_owned(),
                    "-b".into(),
                    format!("127.0.0.1:{port}"),
                ],
                token: None,
                cwd: Some(cwd.clone()),
                env: env.clone(),
                redact: redact.clone(),
                log: self.store.root.join("logs").join(format!("{id}.app.log")),
                port: *port,
                path: String::new(),
                role: Role::App,
                health: Health::TcpConnect,
                restart: config.restart,
                memory_limit_mb: config.memory_limit_mb,
            })
            .await?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
            while !http_health(*port, "", Health::TcpConnect).await {
                if let Some((status, detail)) = self
                    .store
                    .process_health
                    .lock()
                    .map_err(|_| "Process health unavailable.")?
                    .get(id)
                    .and_then(|p| p.get(&key))
                    .cloned()
                    && status == "error"
                {
                    return Err(detail.unwrap_or("PHP worker failed.".into()));
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err("PHP worker did not open its TCP port within 60 seconds.".into());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            self.store
                .process_status(id, &key, "running", None)
                .map_err(|e| e.to_string())?;
        }
        self.start_proxy(
            id,
            hostify_core::caddy::Backend::Php {
                root: cwd,
                upstreams: config.php_ports.clone(),
            },
        )
        .await?;
        if !http_health(site.local_port, &config.health_path, Health::AnyBelow500).await {
            return Err("PHP did not respond through Caddy with a foreign Host header.".into());
        }
        self.store
            .process_status(id, &format!("site:{id}"), "running", None)
            .map_err(|e| e.to_string())?;
        Ok(true)
    }
    async fn start_proxy(
        &self,
        id: &str,
        backend: hostify_core::caddy::Backend,
    ) -> Result<(), String> {
        if !self.store.detection.caddy_ready().await {
            return Err(hostify_core::components::CADDY_UNAVAILABLE.into());
        }
        let site = self.store.site(id).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(self.store.root.join("run/verify").join(id))
            .map_err(|e| e.to_string())?;
        let config = hostify_core::caddy::generate(&[hostify_core::caddy::Site {
            verify_root: self.store.root.join("run/verify").join(id),
            port: site.local_port,
            backend,
            log: self.store.root.join("logs").join(format!("{id}.log")),
        }])
        .map_err(|e| e.to_string())?;
        let path = self.store.root.join("run").join(format!("{id}.Caddyfile"));
        let temp = self
            .store
            .root
            .join("run")
            .join(format!("{id}-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&temp, config).map_err(|e| e.to_string())?;
        let mut command = tokio::process::Command::new(self.store.root.join("bin/caddy.exe"));
        command
            .args(["validate", "--config"])
            .arg(&temp)
            .args(["--adapter", "caddyfile"])
            .stdin(Stdio::null());
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let validation =
            tokio::time::timeout(Duration::from_secs(15), command.kill_on_drop(true).output())
                .await;
        let result = match validation {
            Ok(Ok(out)) if out.status.success() => Ok(()),
            Ok(Ok(out)) => Err(format!(
                "Caddy rejected the configuration: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("Caddy configuration validation timed out.".into()),
        };
        if let Err(e) = result {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(format!("Cannot replace the Caddy configuration: {e}"));
        }
        self.launch(Spec {
            key: format!("site:{id}"),
            site: Some(id.into()),
            exe: self.store.root.join("bin/caddy.exe"),
            args: vec![
                "run".into(),
                "--config".into(),
                path.to_string_lossy().into_owned(),
                "--adapter".into(),
                "caddyfile".into(),
            ],
            token: None,
            cwd: None,
            env: vec![],
            redact: vec![],
            log: self
                .store
                .root
                .join("logs")
                .join(format!("{id}.process.log")),
            port: site.local_port,
            path: "/.well-known/hostify-caddy-ok".into(),
            role: Role::Caddy,
            health: Health::Strict2xx,
            restart: RestartPolicy::Always,
            memory_limit_mb: None,
        })
        .await?;
        for _ in 0..100 {
            if http_ok(site.local_port, "/.well-known/hostify-caddy-ok").await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("The website server did not become healthy.".into())
    }
    async fn start_node(&self, id: &str) -> Result<bool, String> {
        let mut config = self.store.app_config(id).map_err(|e| e.to_string())?;
        let candidate = config
            .built_release
            .clone()
            .filter(|r| config.pending_release.as_ref() == Some(r))
            .or(config.active_release.clone())
            .ok_or("Build and trust a release before starting this website.")?;
        let promoting = config.active_release.as_ref() != Some(&candidate);
        if config.built_release.as_ref() != Some(&candidate)
            || config.trusted_release.as_ref() != Some(&candidate)
        {
            return Err(
                "Build and acknowledge trust for this release before starting this website.".into(),
            );
        }
        let old = config.active_release.clone();
        if promoting {
            if let Some(previous) = &old {
                hostify_core::import::copy_persistent(
                    &self.release_root(id, previous)?,
                    &self.release_root(id, &candidate)?,
                    &config.persistent_paths,
                )?;
            }
            self.stop_site(id).await?;
        }
        let result = self.start_node_release(id, &mut config, &candidate).await;
        match result {
            Ok(started) => {
                if promoting {
                    config.previous_release = old;
                    config.active_release = Some(candidate.clone());
                    config.pending_release = None;
                }
                config.built_release = Some(candidate.clone());
                config.trusted_release = Some(candidate.clone());
                self.store
                    .save_app_config(id, &config)
                    .map_err(|e| e.to_string())?;
                let keep = [
                    config.active_release.as_deref(),
                    config.previous_release.as_deref(),
                    config.pending_release.as_deref(),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                hostify_core::import::prune_app_releases(
                    &self.store.root.join("sites").join(id),
                    &keep,
                )
                .map_err(|e| e.to_string())?;
                Ok(started)
            }
            Err(error) => {
                self.stop_site(id).await?;
                if promoting && let Some(previous) = old {
                    config.active_release = Some(previous.clone());
                    config.built_release = Some(previous.clone());
                    config.trusted_release = Some(previous.clone());
                    self.store
                        .save_app_config(id, &config)
                        .map_err(|e| e.to_string())?;
                    match self.start_node_release(id, &mut config, &previous).await {
                        Ok(_) => {
                            self.store
                                .save_app_config(id, &config)
                                .map_err(|e| e.to_string())?;
                            let message = format!(
                                "The update failed; the previous version is running. {error}"
                            );
                            let _ = self.store.activity("warn", Some(id), &message);
                            return Err(message);
                        }
                        Err(rollback) => {
                            let _ = self.stop_site(id).await;
                            return Err(format!(
                                "The update failed: {error}. The previous version could not restart: {rollback}"
                            ));
                        }
                    }
                }
                Err(error)
            }
        }
    }
    fn build_phase(&self, id: &str, step: &str, state: &str, detail: &str) -> Result<(), String> {
        if let Some(mut status) = self
            .store
            .saved(&format!("build:{id}"))
            .map_err(|e| e.to_string())?
            && (status["state"] == "running"
                || (status["step"] == "start" && status["state"] == "done"))
            && matches!(status["step"].as_str(), Some("start" | "verify"))
        {
            status["step"] = json!(step);
            status["state"] = json!(state);
            status["detail"] = json!(detail);
            self.store
                .save(&format!("build:{id}"), &status)
                .map_err(|e| e.to_string())?;
            self.store.emit("app-build", status);
        }
        Ok(())
    }
    fn check_exposure(&self, id: &str, key: &str) -> Result<(), String> {
        #[cfg(windows)]
        {
            let mut all = self
                .info
                .lock()
                .map_err(|_| "Process information unavailable.")?;
            if let Some(i) = all.get_mut(key) {
                let addresses = crate::windows::exposed_addresses(
                    &i.job.process_ids().map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                for address in &addresses {
                    let _=self.store.activity("warn",Some(id),&format!("The application explicitly listens on {address}, which may be reachable from other computers."));
                }
                i.exposed_addresses = addresses;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod php_recycle_tests {
    use super::*;
    #[test]
    fn clean_recycling_does_not_count_or_reset_real_crashes() {
        let mut crashes = CrashLoop::default();
        for _ in 0..4 {
            assert!(!crashes.php_exit(Duration::ZERO, false));
        }
        for _ in 0..100 {
            assert!(!crashes.php_exit(Duration::ZERO, true));
        }
        assert!(crashes.php_exit(Duration::ZERO, false));
    }
}

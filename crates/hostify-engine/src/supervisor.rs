#[cfg(windows)]
use crate::windows::Job;
use crate::{port_available, restart_delay, state::Store};
use async_trait::async_trait;
use hostify_core::{
    deploy::SiteRuntime,
    secrets::{SecretStore, WindowsCredentialStore},
};
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await?;
        let mut bytes = [0; 128];
        let n = stream.read(&mut bytes).await?;
        let text = String::from_utf8_lossy(&bytes[..n]);
        Ok::<bool, io::Error>(text.starts_with("HTTP/1.1 2") || text.starts_with("HTTP/1.0 2"))
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
#[derive(Clone)]
struct Spec {
    key: String,
    site: Option<String>,
    exe: PathBuf,
    args: Vec<String>,
    token: Option<String>,
    cwd: Option<PathBuf>,
    env: Vec<(String, String)>,
    redact: Vec<String>,
    log: PathBuf,
    port: u16,
    path: &'static str,
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
        if self.site.is_some() {
            "The website server"
        } else {
            "The Cloudflare connector"
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
    #[cfg(windows)]
    job: Arc<Job>,
}
impl Supervisor {
    pub fn for_site(&self, id: &str) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            workers: self.workers.clone(),
            scope: Some(id.into()),
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
                path: "/ready",
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
            #[cfg(windows)]
            job: Arc::new(Job::new()?),
        }))
    }
    fn spawn(&self, spec: &Spec) -> io::Result<Child> {
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
            cmd.creation_flags(0x08000000 | 0x00000004);
        }
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
            if unsafe { NtResumeProcess(handle) } < 0 {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other("Could not resume managed child"));
            }
        }
        if let Some(stdout) = child.stdout.take() {
            pump_redacted(
                stdout,
                spec.log.clone(),
                spec.token.clone(),
                spec.redact.clone(),
            );
        }
        if let Some(stderr) = child.stderr.take() {
            pump_redacted(
                stderr,
                spec.log.clone(),
                spec.token.clone(),
                spec.redact.clone(),
            );
        }
        Ok(child)
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
            if e.kind() == io::ErrorKind::NotFound && spec.site.is_some() {
                hostify_core::components::CADDY_UNAVAILABLE.into()
            } else {
                format!(
                    "{} could not start: {e}. Check Settings > Components.",
                    spec.label()
                )
            }
        })?;
        let (stop, mut rx) = watch::channel(false);
        let store = self.store.clone();
        #[cfg(windows)]
        let job = self.job.clone();
        let key = spec.key.clone();
        let task = tokio::spawn(async move {
            let mut child = Some(child);
            let mut failures = 0u32;
            let mut crash_loop = CrashLoop::default();
            let mut next = tokio::time::Instant::now();
            let mut started = tokio::time::Instant::now();
            let mut healthy = None;
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {_ = rx.changed()=>{break;},_=tick.tick()=>{}}
                if *rx.borrow() {
                    break;
                }
                if let Some(process) = child.as_mut() {
                    match process.try_wait() {
                        Ok(None) => {
                            let ok = http_ok(spec.port, spec.path).await;
                            if still_starting(healthy, ok, started.elapsed()) {
                                continue;
                            }
                            if healthy != Some(ok) {
                                // A first pass is the normal start, so only a change of health is news.
                                let recovered = healthy == Some(false);
                                healthy = Some(ok);
                                if let Some(id) = &spec.site {
                                    let _ = store.status(
                                        id,
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
                            let last_exit = format!("exit code {code}");
                            let _ = process.kill();
                            let _ = process.wait();
                            child = None;
                            if crash_loop.exited(started.elapsed())
                                && spec.site.is_some()
                                && !retained_static(&store, &spec)
                            {
                                let message = format!(
                                    "{} stopped after five exits within 30 seconds of starting ({last_exit}). Start the website manually to try again.",
                                    spec.label()
                                );
                                if let Some(id) = &spec.site {
                                    let _ = store.status(id, "error", Some(message.clone()));
                                }
                                let _ = store.activity("error", spec.site.as_deref(), &message);
                                break;
                            }
                            failures = failures.saturating_add(1);
                            next = tokio::time::Instant::now() + restart_delay(failures);
                            if let Some(id) = &spec.site {
                                let _ = store.status(
                                    id,
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
                        }
                    }
                }
                if tokio::time::Instant::now() < next {
                    continue;
                }
                if !port_available(spec.port) {
                    if let Some(id) = &spec.site {
                        let _ = store.status(
                            id,
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
                    #[cfg(windows)]
                    job: job.clone(),
                };
                let preflight = spec.site.is_none() || store.detection.caddy_ready().await;
                if !preflight {
                    if let Some(id) = &spec.site {
                        let _ = store.status(
                            id,
                            "error",
                            Some(hostify_core::components::CADDY_UNAVAILABLE.into()),
                        );
                    }
                    next = tokio::time::Instant::now() + restart_delay(failures.saturating_add(1));
                    continue;
                }
                match temporary.spawn(&spec) {
                    Ok(c) => {
                        child = Some(c);
                        started = tokio::time::Instant::now();
                        healthy = None;
                        let _ = store.activity(
                            "info",
                            spec.site.as_deref(),
                            &format!("{} restarted", spec.label()),
                        );
                    }
                    Err(e) => {
                        failures = failures.saturating_add(1);
                        next = tokio::time::Instant::now() + restart_delay(failures);
                        let message = if e.kind() == io::ErrorKind::NotFound && spec.site.is_some()
                        {
                            hostify_core::components::CADDY_UNAVAILABLE
                        } else {
                            "Hostify could not restart this process and will try again"
                        };
                        if let Some(id) = &spec.site {
                            let _ = store.status(id, "error", Some(message.into()));
                        }
                        let _ = store.activity("error", spec.site.as_deref(), message);
                    }
                }
            }
            if let Some(mut c) = child {
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
fn pump_redacted(
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
        if !self.store.detection.caddy_ready().await {
            let message = hostify_core::components::CADDY_UNAVAILABLE.to_owned();
            let _ = self.store.status(id, "error", Some(message.clone()));
            return Err(message);
        }
        std::fs::create_dir_all(self.store.root.join("run/verify").join(id))
            .map_err(|e| e.to_string())?;
        let site = self.store.site(id).map_err(|e| e.to_string())?;
        let config = hostify_core::caddy::generate(&[hostify_core::caddy::Site {
            verify_root: self.store.root.join("run/verify").join(id),
            port: site.local_port,
            root: self.store.root.join("sites").join(id).join("current"),
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
                path: "/",
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
                        .status(id, "running", None)
                        .map_err(|e| e.to_string())?;
                    Ok(true)
                } else {
                    let _ = self.stop(&format!("site:{id}")).await;
                    let message = format!(
                        "Website did not become healthy on port {}. Check its logs and whether another application took the port.",
                        site.local_port
                    );
                    let _ = self.store.status(id, "error", Some(message.clone()));
                    Err(message)
                }
            }
        }
    }
    async fn stop_site(&self, id: &str) -> Result<(), String> {
        self.stop(&format!("site:{id}")).await?;
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
        for _ in 0..20 {
            if http_ok(port, "/").await {
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
            path: "/",
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
            path: "/",
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

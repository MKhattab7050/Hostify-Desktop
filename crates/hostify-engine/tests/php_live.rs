#![cfg(windows)]
//! Real PHP runtimes and real Caddy. Isolated data directories; nothing touches a real installation.
use hostify_engine::{
    state::{Engine, Store},
    supervisor::Supervisor,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn live_root() -> PathBuf {
    PathBuf::from(std::env::var_os("HOSTIFY_LIVE_RUNTIMES").expect("Set HOSTIFY_LIVE_RUNTIMES"))
}
fn copy_tree(source: &Path, dest: &Path) {
    std::fs::create_dir_all(dest).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let path = dest.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &path);
        } else {
            std::fs::copy(entry.path(), path).unwrap();
        }
    }
}
struct Fixture {
    engine: Arc<Engine>,
    temp: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        let store = Store::open(root.clone()).unwrap();
        let caddy =
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("Hostify/bin/caddy.exe");
        std::fs::copy(caddy, root.join("bin/caddy.exe")).unwrap();
        for runtime in ["php/8.4.27-1", "php/8.5.12-1", "node/24.22.2"] {
            copy_tree(
                &live_root().join("runtimes").join(runtime),
                &root.join("runtimes").join(runtime),
            );
        }
        let supervisor = Supervisor::new(store.clone()).unwrap();
        let engine = Engine::production(store, supervisor, false);
        Self { engine, temp }
    }
    async fn call(&self, method: &str, params: Value) -> Value {
        self.engine
            .dispatch(method, params)
            .await
            .unwrap_or_else(|e| panic!("{method}: {e}"))
    }
    async fn add_project(&self, source: &Path, config: Option<Value>) -> String {
        let mut request =
            json!({"name":"Live","sourcePath":source,"kind":"directory","siteKind":"php"});
        if let Some(config) = config {
            request["config"] = config;
        }
        let site = self.call("add_site", request).await;
        let id = site["id"].as_str().unwrap().to_owned();
        let config = self.engine.store.app_config(&id).unwrap();
        self.call(
            "set_app_config",
            json!({"siteId":id,"config":config,"phpVersion":"8.5.12-1"}),
        )
        .await;
        id
    }
    fn source(&self, code: &str) -> PathBuf {
        let source = self.temp.path().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("index.php"), code).unwrap();
        source
    }
    async fn add_code(&self, code: &str, workers: u8) -> String {
        self.add_project(&self.source(code), Some(json!({"phpWorkers":workers})))
            .await
    }
    async fn status(&self, id: &str) -> Value {
        self.call("app_status", json!({"siteId":id})).await
    }
    async fn stop(&self, id: &str) {
        self.call("stop_site", json!({"id":id})).await;
    }
    async fn build(&self, id: &str) {
        let target = self.engine.store.app_config(id).unwrap().pending_release;
        let build = self
            .call("build_app", json!({"siteId":id,"trustAcknowledged":true}))
            .await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        loop {
            let status = self.call("get_build_status", json!({"siteId":id})).await;
            assert_eq!(status["buildId"], build["buildId"]);
            assert_ne!(status["state"], "failed", "{status}");
            let config = self.engine.store.app_config(id).unwrap();
            if config.built_release.is_some()
                && (config.built_release == target
                    || (config.pending_release.is_none()
                        && config.built_release == config.active_release))
                && status["state"] == "done"
            {
                // Lets the finished task release the per-site build slot.
                tokio::time::sleep(Duration::from_millis(200)).await;
                return;
            }
            assert!(tokio::time::Instant::now() < deadline, "{status}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn start(&self, id: &str) {
        self.call("start_site", json!({"id":id})).await;
    }
    fn port(&self, id: &str) -> u16 {
        self.engine.store.site(id).unwrap().local_port
    }
    /// Waits until the supervisor observes the condition on the app's status.
    async fn wait_status(&self, id: &str, seconds: u64, check: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
        loop {
            let status = self.call("app_status", json!({"siteId":id})).await;
            if check(&status) {
                return status;
            }
            assert!(tokio::time::Instant::now() < deadline, "{status}");
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}
async fn get(port: u16, path: &str) -> String {
    get_with_host(port, path, "localhost").await
}

async fn get_with_host(port: u16, path: &str, host: &str) -> String {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        socket
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();
        response
    })
    .await
    .unwrap()
}

const APP: &str = "<?php header('Content-Type: application/json'); echo json_encode(['version'=>PHP_VERSION, 'sapi'=>PHP_SAPI, 'pid'=>getmypid()]);";

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real Caddy"]
async fn sample_versions_extensions_source_disclosure_and_user_ini() {
    let f = Fixture::new();
    let sample = f.temp.path().join("sample");
    copy_tree(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/php-basic"),
        &sample,
    );
    std::fs::write(
        sample.join("secret.php"),
        "<?php $secret = 'UNIQUE-PHP-SOURCE-51d90a'; echo 'safe';",
    )
    .unwrap();
    std::fs::write(sample.join(".env"), "UNIQUE-PHP-SOURCE-51d90a").unwrap();
    std::fs::write(sample.join("composer.json"), r#"{"name":"fixture/source"}"#).unwrap();
    std::fs::write(sample.join("php.ini"), "UNIQUE-PHP-SOURCE-51d90a").unwrap();
    std::fs::write(sample.join(".user.ini"), "memory_limit=7M\nexpose_php=On").unwrap();
    std::fs::write(sample.join("extensions.php"), "<?php echo json_encode([extension_loaded('mbstring'),extension_loaded('pdo_mysql'),extension_loaded('openssl'),ini_get('memory_limit'),ini_get('expose_php')]);").unwrap();
    let id = f
        .add_project(
            &sample,
            Some(json!({"healthPath":"/health","phpSettings":{"memory_limit":"128M"}})),
        )
        .await;
    f.call(
        "set_env_var",
        json!({"siteId":id,"name":"SAMPLE_GREETING","value":"hello","secret":false}),
    )
    .await;
    f.build(&id).await;
    f.start(&id).await;
    let response = get(f.port(&id), "/health").await;
    assert!(response.contains("\"sample\":\"php\""), "{response}");
    assert!(response.contains("cgi-fcgi"));
    assert!(
        get(f.port(&id), "/")
            .await
            .contains("SAMPLE_GREETING is set")
    );
    assert!(
        get(f.port(&id), "/extensions.php")
            .await
            .contains("[true,true,true,\"128M\",\"\"]")
    );
    for path in [
        "/secret.php",
        "/SECRET.PHP",
        "/secret.php::$DATA",
        "/secret.php.",
        "/secret.php%20",
        "/secret.php%00",
        "/secret.php/",
        "/.env",
        "/composer.json",
        "/php.ini",
        "/vendor/file.php",
        "/deep/test.log",
    ] {
        let response = get(f.port(&id), path).await;
        assert!(
            !response.contains("UNIQUE-PHP-SOURCE-51d90a")
                && !response.contains("<?php")
                && !response.contains("fixture/source"),
            "{path}: {response}"
        );
    }
    let other = f.add_code(APP, 2).await;
    let c = f.engine.store.app_config(&other).unwrap();
    f.call(
        "set_app_config",
        json!({"siteId":other,"config":c,"phpVersion":"8.4.27-1"}),
    )
    .await;
    f.build(&other).await;
    f.start(&other).await;
    assert!(get(f.port(&id), "/health").await.contains("8.5.12"));
    assert!(get(f.port(&other), "/").await.contains("8.4.27"));
    let ports = f.engine.store.app_config(&id).unwrap().php_ports;
    let pids = f.status(&id).await["workers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["pid"].as_u64().unwrap() as u32)
        .collect::<Vec<_>>();
    f.stop(&id).await;
    for port in ports {
        assert!(hostify_engine::port_available(port));
    }
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    for pid in pids {
        assert!(sys.process(sysinfo::Pid::from_u32(pid)).is_none());
    }
    assert!(get(f.port(&other), "/").await.contains("8.4.27"));
    f.engine.shutdown().await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real Caddy"]
async fn four_workers_run_concurrently_and_one_serializes() {
    let f = Fixture::new();
    let mut times = Vec::new();
    for workers in [4, 1] {
        let id = f
            .add_code(
                "<?php if ($_SERVER['REQUEST_URI'] === '/slow') { sleep(2); } echo 'done';",
                workers,
            )
            .await;
        f.build(&id).await;
        f.start(&id).await;
        let started = std::time::Instant::now();
        let mut requests = Vec::new();
        for _ in 0..4 {
            let port = f.port(&id);
            requests.push(tokio::spawn(async move { get(port, "/slow").await }));
        }
        for request in requests {
            let r = request.await.unwrap();
            assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        }
        let elapsed = started.elapsed().as_secs_f64();
        println!("{workers} PHP workers, four 2-second requests: {elapsed:.3}s");
        times.push(elapsed);
        f.stop(&id).await;
    }
    assert!(times[0] < 4.0, "parallel: {}", times[0]);
    assert!((7.5..14.0).contains(&times[1]), "serial: {}", times[1]);
    f.engine.shutdown().await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real Caddy"]
async fn killed_worker_recovers_while_other_workers_serve() {
    let f = Fixture::new();
    let id = f.add_code(APP, 4).await;
    f.build(&id).await;
    f.start(&id).await;
    let before = f.status(&id).await;
    let pid = before["workers"][0]["pid"].as_u64().unwrap();
    unsafe {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid as u32);
        assert!(
            !handle.is_null(),
            "OpenProcess: {}",
            std::io::Error::last_os_error()
        );
        let result = TerminateProcess(handle, 9);
        windows_sys::Win32::Foundation::CloseHandle(handle);
        assert_ne!(
            result,
            0,
            "TerminateProcess: {}",
            std::io::Error::last_os_error()
        );
    }
    let other_pids = before["workers"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .map(|w| w["pid"].clone())
        .collect::<Vec<_>>();
    for _ in 0..8 {
        assert!(get(f.port(&id), "/").await.contains("cgi-fcgi"));
    }
    let after = f
        .wait_status(&id, 30, |s| {
            s["workers"][0]["pid"].as_u64().is_some_and(|p| p != pid) && s["health"] == "running"
        })
        .await;
    for (n, pid) in other_pids.iter().enumerate() {
        assert_eq!(after["workers"][n + 1]["pid"], *pid);
    }
    assert!(after["restarts"].as_u64().unwrap() >= 1);
    f.engine.shutdown().await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES, rustc and real Caddy"]
async fn clean_worker_recycling_stays_running_without_crash_activity() {
    let f = Fixture::new();
    // Test-only wrapper overrides the real child's request count. Production always uses 10000.
    let runtime = f.engine.store.root.join("runtimes/php/8.5.12-1");
    std::fs::rename(
        runtime.join("php-cgi.exe"),
        runtime.join("php-cgi-real.exe"),
    )
    .unwrap();
    let source = f.temp.path().join("recycle.rs");
    std::fs::write(&source, r#"fn main() {
        let exe = std::env::current_exe().unwrap().with_file_name("php-cgi-real.exe");
        let status = std::process::Command::new(exe).args(std::env::args_os().skip(1)).env("PHP_FCGI_MAX_REQUESTS", "3").status().unwrap();
        std::process::exit(status.code().unwrap_or(1));
    }"#).unwrap();
    let rustc =
        PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(".cargo/bin/rustc.exe");
    let output = std::process::Command::new(rustc)
        .arg(&source)
        .arg("-o")
        .arg(runtime.join("php-cgi.exe"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let id = f.add_code(APP, 4).await;
    let mut config = f.engine.store.app_config(&id).unwrap();
    config.restart = hostify_core::app_config::RestartPolicy::OnFailure;
    f.call("set_app_config", json!({"siteId":id,"config":config}))
        .await;
    f.build(&id).await;
    f.start(&id).await;
    for _ in 0..80 {
        let r = get(f.port(&id), "/").await;
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    }
    let status = f
        .wait_status(&id, 30, |s| {
            s["restarts"].as_u64().unwrap_or(0) >= 8 && s["health"] == "running"
        })
        .await;
    println!("Recycled workers: {}", status["restarts"]);
    assert_eq!(f.engine.store.site(&id).unwrap().status, "running");
    let activity = f
        .call("list_activity", json!({"siteId":id,"limit":500}))
        .await
        .to_string();
    assert!(
        !activity.contains("five exits")
            && !activity.contains("unexpectedly")
            && !activity.contains("restarted"),
        "{activity}"
    );
    f.engine.shutdown().await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real Caddy"]
async fn persistent_paths_promote_copy_failure_preserves_running_and_bad_health_rolls_back() {
    let f = Fixture::new();
    let source = f.source("<?php echo 'old-release';");
    std::fs::create_dir(source.join("uploads")).unwrap();
    let id = f
        .add_project(
            &source,
            Some(json!({"persistentPaths":["uploads","config.php"]})),
        )
        .await;
    f.build(&id).await;
    f.start(&id).await;
    let c = f.engine.store.app_config(&id).unwrap();
    let old = c.active_release.unwrap();
    let root = f
        .engine
        .store
        .root
        .join("sites")
        .join(&id)
        .join("app")
        .join(&old);
    std::fs::write(root.join("uploads/file.txt"), "survives").unwrap();
    std::fs::write(root.join("config.php"), "<?php // survives").unwrap();
    std::fs::write(source.join("index.php"), "<?php echo 'new-release';").unwrap();
    f.call(
        "update_site_files",
        json!({"id":id,"sourcePath":source,"kind":"directory"}),
    )
    .await;
    f.build(&id).await;
    f.wait_status(&id, 30, |_| {
        f.engine
            .store
            .app_config(&id)
            .unwrap()
            .pending_release
            .is_none()
    })
    .await;
    assert!(get(f.port(&id), "/").await.contains("new-release"));
    assert!(
        get(f.port(&id), "/uploads/file.txt")
            .await
            .contains("survives")
    );
    let new = f
        .engine
        .store
        .app_config(&id)
        .unwrap()
        .active_release
        .unwrap();
    assert!(
        std::fs::read_to_string(
            f.engine
                .store
                .root
                .join("sites")
                .join(&id)
                .join("app")
                .join(&new)
                .join("config.php")
        )
        .unwrap()
        .contains("survives")
    );
    // A destination type mismatch makes copying fail before the active pool is stopped.
    std::fs::write(source.join("config.php"), "<?php").unwrap();
    std::fs::remove_dir(source.join("uploads")).unwrap();
    std::fs::write(source.join("uploads"), "wrong type").unwrap();
    f.call(
        "update_site_files",
        json!({"id":id,"sourcePath":source,"kind":"directory"}),
    )
    .await;
    let before = f.status(&id).await;
    f.call("build_app", json!({"siteId":id,"trustAcknowledged":true}))
        .await;
    wait_build_failed(&f, &id).await;
    assert_eq!(f.status(&id).await["workers"], before["workers"]);
    assert!(get(f.port(&id), "/").await.contains("new-release"));
    std::fs::remove_file(source.join("uploads")).unwrap();
    std::fs::create_dir(source.join("uploads")).unwrap();
    std::fs::write(
        source.join("index.php"),
        "<?php http_response_code(503); echo 'bad-release';",
    )
    .unwrap();
    f.call(
        "update_site_files",
        json!({"id":id,"sourcePath":source,"kind":"directory"}),
    )
    .await;
    f.call("build_app", json!({"siteId":id,"trustAcknowledged":true}))
        .await;
    wait_build_failed(&f, &id).await;
    assert_eq!(
        f.engine
            .store
            .app_config(&id)
            .unwrap()
            .active_release
            .as_deref(),
        Some(new.as_str())
    );
    assert!(get(f.port(&id), "/").await.contains("new-release"));
    f.engine.shutdown().await;
}
async fn wait_build_failed(f: &Fixture, id: &str) {
    // A failed health check may wait up to 60 seconds before rollback.
    for _ in 0..1200 {
        let status = f.call("get_build_status", json!({"siteId":id})).await;
        if status["state"] == "failed" {
            tokio::time::sleep(Duration::from_millis(200)).await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("build did not fail");
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real Caddy"]
async fn trust_document_root_and_junction_refusals_and_mixed_sites() {
    let f = Fixture::new();
    let source = f.source(APP);
    let refused = f.engine.dispatch("add_site", json!({"name":"bad","sourcePath":source,"kind":"directory","siteKind":"php","config":{"documentRoot":"../outside"}})).await;
    assert!(refused.is_err());
    let id = f.add_project(&source, None).await;
    assert!(
        f.engine
            .dispatch("build_app", json!({"siteId":id}))
            .await
            .unwrap_err()
            .to_string()
            .contains("trust")
    );
    assert!(
        f.engine
            .dispatch("start_site", json!({"id":id}))
            .await
            .is_err()
    );
    let c = f.engine.store.app_config(&id).unwrap();
    let release = f
        .engine
        .store
        .root
        .join("sites")
        .join(&id)
        .join("app")
        .join(c.pending_release.as_ref().unwrap());
    let outside = f.temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let junction = release.join("junction");
    assert!(
        std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .status()
            .unwrap()
            .success()
    );
    let mut bad = c.clone();
    bad.document_root = "junction".into();
    assert!(
        f.engine
            .dispatch("set_app_config", json!({"siteId":id,"config":bad}))
            .await
            .unwrap_err()
            .to_string()
            .contains("inside")
    );
    std::fs::remove_dir(junction).unwrap();
    f.build(&id).await;
    f.start(&id).await;
    let static_source = f.temp.path().join("static");
    std::fs::create_dir(&static_source).unwrap();
    std::fs::write(static_source.join("index.html"), "static-alive").unwrap();
    let static_site = f
        .call(
            "add_site",
            json!({"name":"static","sourcePath":static_source,"kind":"directory"}),
        )
        .await;
    let static_id = static_site["id"].as_str().unwrap();
    f.start(static_id).await;
    let node_source = f.temp.path().join("node");
    std::fs::create_dir(&node_source).unwrap();
    std::fs::write(
        node_source.join("server.cjs"),
        "require('http').createServer((q,s)=>s.end('node-alive')).listen(+process.env.PORT);",
    )
    .unwrap();
    std::fs::write(node_source.join("package.json"), "{}").unwrap();
    let node = f.call("add_site", json!({"name":"node","sourcePath":node_source,"kind":"directory","siteKind":"node","config":{"install":"none","start":{"type":"node","args":["server.cjs"]}}})).await;
    let node_id = node["id"].as_str().unwrap();
    let config = f.engine.store.app_config(node_id).unwrap();
    f.call(
        "set_app_config",
        json!({"siteId":node_id,"config":config,"nodeVersion":"24.22.2"}),
    )
    .await;
    f.build(node_id).await;
    f.start(node_id).await;
    let before = f.status(node_id).await["pid"].clone();
    f.call("restart_site", json!({"id":id})).await;
    assert_eq!(f.status(node_id).await["pid"], before);
    assert!(get(f.port(node_id), "/").await.contains("node-alive"));
    assert!(get(f.port(static_id), "/").await.contains("static-alive"));
    let php_pids = f.status(&id).await["workers"].clone();
    f.call("restart_site", json!({"id":node_id})).await;
    f.call("restart_site", json!({"id":static_id})).await;
    assert_eq!(f.status(&id).await["workers"], php_pids);
    assert!(get(f.port(&id), "/").await.contains("cgi-fcgi"));
    f.engine.shutdown().await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real Caddy"]
async fn regression_host_isolation_foreign_host_rejection_and_diagnostics() {
    let f = Fixture::new();

    // 1. Set up PHP Site 1 with domain 'php1.example.test'
    let source1 =
        f.source("<?php echo 'php1-app-response:' . ($_SERVER['HTTP_HOST'] ?? 'no-host');");
    let id1 = f.add_project(&source1, None).await;
    f.engine
        .store
        .db()
        .unwrap()
        .execute(
            "INSERT INTO domains(id, hostname, zone_name, zone_id, site_id, dns_status, https_verified) VALUES(?1, ?2, 'example.test', 'z1', ?3, 'active', 1)",
            [uuid::Uuid::new_v4().to_string(), "php1.example.test".into(), id1.clone()],
        )
        .unwrap();
    f.build(&id1).await;
    f.start(&id1).await;

    // 2. Set up PHP Site 2 with domain 'php2.example.test'
    let source2 =
        f.source("<?php echo 'php2-app-response:' . ($_SERVER['HTTP_HOST'] ?? 'no-host');");
    let id2 = f.add_project(&source2, None).await;
    f.engine
        .store
        .db()
        .unwrap()
        .execute(
            "INSERT INTO domains(id, hostname, zone_name, zone_id, site_id, dns_status, https_verified) VALUES(?1, ?2, 'example.test', 'z2', ?3, 'active', 1)",
            [uuid::Uuid::new_v4().to_string(), "php2.example.test".into(), id2.clone()],
        )
        .unwrap();
    f.build(&id2).await;
    f.start(&id2).await;

    let port1 = f.port(&id1);
    let port2 = f.port(&id2);

    // REGRESSION TEST 1: Valid hostname reaches the intended PHP website
    let r1 = get_with_host(port1, "/", "php1.example.test").await;
    assert!(
        r1.starts_with("HTTP/1.1 200 OK"),
        "Expected 200 OK, got: {r1}"
    );
    assert!(
        r1.contains("php1-app-response:php1.example.test"),
        "Got: {r1}"
    );

    let r2 = get_with_host(port2, "/", "php2.example.test").await;
    assert!(
        r2.starts_with("HTTP/1.1 200 OK"),
        "Expected 200 OK, got: {r2}"
    );
    assert!(
        r2.contains("php2-app-response:php2.example.test"),
        "Got: {r2}"
    );

    // Also loopback addresses reach the PHP website
    let r1_loopback = get_with_host(port1, "/", "localhost").await;
    assert!(
        r1_loopback.starts_with("HTTP/1.1 200 OK"),
        "Loopback should reach site: {r1_loopback}"
    );
    assert!(r1_loopback.contains("php1-app-response:localhost"));

    let r1_ip = get_with_host(port1, "/", "127.0.0.1").await;
    assert!(
        r1_ip.starts_with("HTTP/1.1 200 OK"),
        "Loopback IP should reach site: {r1_ip}"
    );

    // REGRESSION TEST 2: Foreign hostname cannot reach the PHP website
    let r1_foreign = get_with_host(port1, "/", "foreign.example").await;
    assert!(
        r1_foreign.starts_with("HTTP/1.1 421"),
        "Expected 421 Misdirected Request, got: {r1_foreign}"
    );
    assert!(
        !r1_foreign.contains("php1-app-response"),
        "Foreign host must never reach PHP application!"
    );

    let r1_evil = get_with_host(port1, "/", "evil-attacker.com").await;
    assert!(
        r1_evil.starts_with("HTTP/1.1 421"),
        "Expected 421 for evil.com: {r1_evil}"
    );
    assert!(!r1_evil.contains("php1-app-response"));

    // REGRESSION TEST 3: Multiple PHP websites remain isolated
    let r1_cross = get_with_host(port1, "/", "php2.example.test").await;
    assert!(
        r1_cross.starts_with("HTTP/1.1 421"),
        "Cross-site Host on port 1 must be rejected: {r1_cross}"
    );
    assert!(!r1_cross.contains("php2-app-response") && !r1_cross.contains("php1-app-response"));

    let r2_cross = get_with_host(port2, "/", "php1.example.test").await;
    assert!(
        r2_cross.starts_with("HTTP/1.1 421"),
        "Cross-site Host on port 2 must be rejected: {r2_cross}"
    );
    assert!(!r2_cross.contains("php1-app-response") && !r2_cross.contains("php2-app-response"));

    // REGRESSION TEST 4: PHP and Node.js websites coexist
    let node_source = f.temp.path().join("node-coexist");
    std::fs::create_dir(&node_source).unwrap();
    std::fs::write(
        node_source.join("server.cjs"),
        "require('http').createServer((q,s)=>s.end('node-coexist-ok:'+q.headers.host)).listen(+process.env.PORT);",
    )
    .unwrap();
    std::fs::write(node_source.join("package.json"), "{}").unwrap();
    let node_site = f
        .call(
            "add_site",
            json!({"name":"node-coexist","sourcePath":node_source,"kind":"directory","siteKind":"node","config":{"install":"none","start":{"type":"node","args":["server.cjs"]}}}),
        )
        .await;
    let node_id = node_site["id"].as_str().unwrap();
    let node_config = f.engine.store.app_config(node_id).unwrap();
    f.call(
        "set_app_config",
        json!({"siteId":node_id,"config":node_config,"nodeVersion":"24.22.2"}),
    )
    .await;
    f.build(node_id).await;
    f.start(node_id).await;
    let node_port = f.port(node_id);

    // Both PHP and Node serve concurrently
    assert!(
        get_with_host(port1, "/", "php1.example.test")
            .await
            .contains("php1-app-response")
    );
    assert!(get(node_port, "/").await.contains("node-coexist-ok"));

    // Restarting PHP does not affect Node
    f.call("restart_site", json!({"id": id1})).await;
    assert!(get(node_port, "/").await.contains("node-coexist-ok"));
    assert!(
        get_with_host(port1, "/", "php1.example.test")
            .await
            .contains("php1-app-response")
    );

    // REGRESSION TEST 5: No PHP source code is exposed
    for path in ["/.env", "/composer.json", "/php.ini", "/index.php::$DATA"] {
        let resp = get_with_host(port1, path, "php1.example.test").await;
        assert!(!resp.contains("<?php"), "Source leaked on {path}: {resp}");
    }

    // REGRESSION TEST 6: Deployment startup succeeds when the correct local route is healthy
    assert_eq!(f.engine.store.site(&id1).unwrap().status, "running");

    // REGRESSION TEST 7: Deployment fails with accurate diagnostics when routing/service is genuinely broken
    let broken_source =
        f.source("<?php http_response_code(500); echo 'Fatal database connection error';");
    let broken_id = f.add_project(&broken_source, None).await;
    f.build(&broken_id).await;

    let start_result = f
        .engine
        .dispatch("start_site", json!({"id": broken_id}))
        .await;
    assert!(start_result.is_err(), "Broken PHP site must fail to start");
    let err_msg = start_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("500"),
        "Error message must report HTTP 500: {err_msg}"
    );
    assert!(
        !err_msg.contains("PHP did not respond through Caddy with a foreign Host header."),
        "Error message must not use the misleading foreign Host header text: {err_msg}"
    );

    f.engine.shutdown().await;
}

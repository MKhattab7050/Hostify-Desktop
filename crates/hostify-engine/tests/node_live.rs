#![cfg(windows)]
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
    _temp: tempfile::TempDir,
    source: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let live = PathBuf::from(
            std::env::var_os("HOSTIFY_LIVE_RUNTIMES")
                .expect("Set HOSTIFY_LIVE_RUNTIMES to the runtime data root"),
        );
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        let store = Store::open(root.clone()).unwrap();
        let caddy =
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("Hostify/bin/caddy.exe");
        assert!(
            caddy.is_file(),
            "The existing Caddy binary is required: {}",
            caddy.display()
        );
        std::fs::copy(caddy, root.join("bin/caddy.exe")).unwrap();
        copy_tree(
            &live.join("runtimes/node/24.22.2"),
            &root.join("runtimes/node/24.22.2"),
        );
        let supervisor = Supervisor::new(store.clone()).unwrap();
        let engine = Engine::production(store, supervisor, false);
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        Self {
            engine,
            _temp: temp,
            source,
        }
    }
    async fn add(&self, code: &str) -> String {
        std::fs::write(self.source.join("server.cjs"), code).unwrap();
        std::fs::write(
            self.source.join("package.json"),
            r#"{"name":"fixture","scripts":{"start":"node server.cjs","build":"node build.cjs"}}"#,
        )
        .unwrap();
        std::fs::write(self.source.join("build.cjs"),"require('node:fs').writeFileSync('built.txt','built'); console.log('build env='+process.env.NODE_ENV)").unwrap();
        let site=self.engine.dispatch("add_site",json!({"name":"Node fixture","sourcePath":self.source,"kind":"directory","siteKind":"node","config":{"install":"none","buildScript":"build","start":{"type":"node","args":["server.cjs"]}}})).await.unwrap();
        let id = site["id"].as_str().unwrap().to_owned();
        self.engine.dispatch("set_app_config",json!({"siteId":id,"config":self.engine.store.app_config(&id).unwrap(),"nodeVersion":"24.22.2"})).await.unwrap();
        id
    }
    async fn build(&self, id: &str) -> Value {
        let was_running = self.engine.store.site(id).unwrap().status == "running";
        let response = self
            .engine
            .dispatch("build_app", json!({"siteId":id,"trustAcknowledged":true}))
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            let status = self
                .engine
                .dispatch("get_build_status", json!({"siteId":id}))
                .await
                .unwrap();
            assert_eq!(status["buildId"], response["buildId"]);
            // Config/verify is durable before the task completes; give it one scheduler turn.
            if status["state"] == "failed"
                || status["state"] == "cancelled"
                || (status["state"] == "done"
                    && if was_running {
                        status["step"] == "verify"
                    } else {
                        let c = self.engine.store.app_config(id).unwrap();
                        c.built_release.is_some() && c.built_release == c.pending_release
                    })
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
                return status;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "build timed out: {status}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    async fn start(&self, id: &str) {
        self.engine
            .dispatch("start_site", json!({"id":id}))
            .await
            .unwrap();
    }
    async fn stop(&self, id: &str) {
        self.engine
            .dispatch("stop_site", json!({"id":id}))
            .await
            .unwrap();
    }
    async fn status(&self, id: &str) -> Value {
        self.engine
            .dispatch("app_status", json!({"siteId":id}))
            .await
            .unwrap()
    }
    async fn get(&self, id: &str, path: &str) -> String {
        request(self.engine.store.site(id).unwrap().local_port, path).await
    }
    async fn update(&self, id: &str, code: &str, build: &str) {
        std::fs::write(self.source.join("server.cjs"), code).unwrap();
        std::fs::write(self.source.join("build.cjs"), build).unwrap();
        self.engine
            .dispatch(
                "update_site_files",
                json!({"id":id,"sourcePath":self.source,"kind":"directory"}),
            )
            .await
            .unwrap();
    }
}
async fn request(port: u16, path: &str) -> String {
    tokio::time::timeout(Duration::from_secs(4), async {
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        socket
            .write_all(
                format!(
                    "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                )
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
const APP: &str = r#"
const http=require('node:http');
const server=http.createServer((req,res)=>{
 if(req.url==='/crash'){res.end('crashing');setTimeout(()=>process.exit(7),30);return;}
 res.end('fixture:'+process.env.VISIBLE+':'+process.pid);
});
server.on('upgrade',(req,socket)=>{socket.write('HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n');socket.on('data',data=>socket.write(data));});
process.on('SIGINT',()=>{require('node:fs').writeFileSync('graceful.txt','yes');server.close(()=>process.exit(0));});
server.listen(Number(process.env.PORT),()=>console.log('listening'));
"#;
#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and installed Caddy; uses isolated data"]
async fn live_build_proxy_websocket_restart_updates_and_three_sites() {
    let f = Fixture::new();
    let id = f.add(APP).await;
    assert!(
        f.engine
            .dispatch("build_app", json!({"siteId":id,"trustAcknowledged":false}))
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
    let mut bad = f.engine.store.app_config(&id).unwrap();
    bad.working_dir = "..\\outside".into();
    assert!(
        f.engine
            .dispatch("set_app_config", json!({"siteId":id,"config":bad}))
            .await
            .is_err()
    );
    f.engine
        .dispatch(
            "set_env_var",
            json!({"siteId":id,"name":"VISIBLE","value":"injected","secret":false}),
        )
        .await
        .unwrap();
    assert_eq!(f.build(&id).await["state"], "done");
    f.start(&id).await;
    let response = f.get(&id, "/").await;
    assert!(response.contains("fixture:injected"), "{response}");
    assert!(
        response
            .to_lowercase()
            .contains("x-hostify-verify: hostify")
    );
    let first = f.status(&id).await;
    assert!(first["pid"].is_number());
    assert!(first["processCount"].as_u64().unwrap() > 0);
    assert!(first["memoryBytes"].as_u64().unwrap() > 0);
    assert_eq!(first["exposedAddresses"], json!([]));
    let mut websocket =
        tokio::net::TcpStream::connect(("127.0.0.1", f.engine.store.site(&id).unwrap().local_port))
            .await
            .unwrap();
    websocket.write_all(b"GET /socket HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").await.unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        websocket.read_exact(&mut byte).await.unwrap();
        header.push(byte[0]);
    }
    assert!(String::from_utf8_lossy(&header).contains("101 Switching Protocols"));
    let frame = [0x81, 0x82, 1, 2, 3, 4, b'h' ^ 1, b'i' ^ 2];
    websocket.write_all(&frame).await.unwrap();
    let mut echo = [0; 8];
    websocket.read_exact(&mut echo).await.unwrap();
    assert_eq!(frame, echo);
    drop(websocket);
    f.get(&id, "/crash").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let status = f.status(&id).await;
        if status["pid"].is_number()
            && status["pid"] != first["pid"]
            && f.engine.store.site(&id).unwrap().status == "running"
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // A second app and static site keep serving while the first is updated.
    let second = f.add(APP).await;
    assert_eq!(f.build(&second).await["state"], "done");
    f.start(&second).await;
    let static_source = f.source.join("static");
    std::fs::create_dir(&static_source).unwrap();
    std::fs::write(static_source.join("index.html"), "static fixture").unwrap();
    let static_site = f
        .engine
        .dispatch(
            "add_site",
            json!({"name":"Static","sourcePath":static_source,"kind":"directory"}),
        )
        .await
        .unwrap();
    let static_id = static_site["id"].as_str().unwrap();
    f.start(static_id).await;
    let ports = f.engine.store.reserved_ports().unwrap();
    let unique = ports.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(ports.len(), unique.len());
    let active = f
        .engine
        .store
        .app_config(&id)
        .unwrap()
        .active_release
        .unwrap();
    f.update(
        &id,
        APP,
        "console.error('real npm build failure');process.exit(23)",
    )
    .await;
    let failed = f.build(&id).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["exitCode"], 23);
    assert!(
        failed["detail"]
            .as_str()
            .unwrap()
            .contains("real npm build failure")
    );
    assert!(f.get(&id, "/").await.contains("fixture:injected"));
    assert_eq!(
        f.engine
            .store
            .app_config(&id)
            .unwrap()
            .active_release
            .as_deref(),
        Some(active.as_str())
    );
    // A build succeeds but the new app exits before health: restore the previous release.
    f.update(&id, "process.exit(17)", "console.log('build ok')")
        .await;
    let rollback = f.build(&id).await;
    assert_eq!(rollback["state"], "failed", "{rollback}");
    assert!(
        rollback["detail"]
            .as_str()
            .unwrap()
            .contains("previous version is running"),
        "{rollback}"
    );
    assert!(f.get(&id, "/").await.contains("fixture:injected"));
    assert_eq!(f.engine.store.site(&id).unwrap().status, "running");
    assert!(f.get(&second, "/").await.contains("fixture:"));
    assert!(f.get(static_id, "/").await.contains("static fixture"));
    f.stop(&id).await;
    assert!(
        f.engine
            .store
            .root
            .join("sites")
            .join(&id)
            .join("app")
            .join(&active)
            .join("graceful.txt")
            .exists()
    );
    // The test/engine process remains alive after delivering Ctrl+C.
    assert_eq!(f.engine.dispatch("ping", json!({})).await.unwrap(), "pong");
    let logs = f
        .engine
        .dispatch("app_logs", json!({"siteId":id,"stream":"app","lines":100}))
        .await
        .unwrap();
    assert!(!logs.to_string().contains("injected"));
    f.engine.shutdown().await;
    // Trusted/built releases auto-start after reopening an isolated engine store.
    f.engine
        .store
        .db()
        .unwrap()
        .execute("UPDATE websites SET auto_start=1 WHERE id=?1", [&id])
        .unwrap();
    let reopened = Store::open(f.engine.store.root.clone()).unwrap();
    let supervisor = Supervisor::new(reopened.clone()).unwrap();
    let mut restarted = Engine::production(reopened.clone(), supervisor, false);
    Arc::get_mut(&mut restarted).unwrap().secrets = Arc::new(NoSecrets);
    restarted.startup().await.unwrap();
    assert!(
        request(reopened.site(&id).unwrap().local_port, "/")
            .await
            .contains("fixture:injected")
    );
    assert_eq!(
        reopened.app_config(&id).unwrap().active_release.as_deref(),
        Some(active.as_str())
    );
    restarted.shutdown().await;
}

#[tokio::test]
#[ignore = "requires real Node and Caddy; process tree and network fixtures"]
async fn live_grandchild_loopback_explicit_exposure_esm_and_crash_loop() {
    let f = Fixture::new();
    let child_port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let code = format!(
        r#"require('node:child_process').spawn(process.execPath,['-e',"require('node:net').createServer().listen({child_port})"],{{stdio:'ignore'}});{APP}"#
    );
    let id = f.add(&code).await;
    assert_eq!(f.build(&id).await["state"], "done");
    f.start(&id).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while hostify_engine::port_available(child_port) {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(f.status(&id).await["processCount"].as_u64().unwrap() >= 2);
    f.stop(&id).await;
    assert!(hostify_engine::port_available(child_port));
    f.update(&id,"require('node:http').createServer((q,s)=>s.end('explicit')).listen(Number(process.env.PORT),'0.0.0.0')","console.log('ok')").await;
    assert_eq!(f.build(&id).await["state"], "done");
    f.start(&id).await;
    let status = f.status(&id).await;
    assert!(
        status["exposedAddresses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a.as_str().unwrap().starts_with("0.0.0.0:"))
    );
    f.stop(&id).await;
    // ESM and listen(port, callback) use the same preload without changing module format.
    let esm = "import http from 'node:http';http.createServer((q,s)=>{s.statusCode=404;s.end('esm');}).listen(Number(process.env.PORT),()=>console.log('ready'));";
    std::fs::write(f.source.join("server.mjs"), esm).unwrap();
    f.update(&id, "import('./server.mjs')", "console.log('ok')")
        .await;
    assert_eq!(f.build(&id).await["state"], "done");
    f.start(&id).await;
    assert!(f.get(&id, "/").await.contains("esm"));
    let internal = f
        .engine
        .store
        .app_config(&id)
        .unwrap()
        .internal_port
        .unwrap();
    let udp = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    udp.connect("192.0.2.1:80").unwrap();
    let lan = udp.local_addr().unwrap().ip();
    if !lan.is_loopback() {
        assert!(
            tokio::net::TcpStream::connect((lan, internal))
                .await
                .is_err(),
            "Host-less listen was reachable on LAN address {lan}"
        );
    }
    assert_eq!(f.status(&id).await["exposedAddresses"], json!([]));
    f.stop(&id).await;
    f.update(
        &id,
        "setTimeout(()=>process.exit(9),100)",
        "console.log('ok')",
    )
    .await;
    assert_eq!(f.build(&id).await["state"], "done");
    assert!(
        f.engine
            .dispatch("start_site", json!({"id":id}))
            .await
            .is_err()
    );
    // Exercise crash-loop handling on an app that first passes health and then repeatedly exits.
    f.update(&id,"require('node:http').createServer((q,s)=>s.end('loop')).listen(Number(process.env.PORT));setTimeout(()=>process.exit(9),1500)","console.log('ok')").await;
    assert_eq!(f.build(&id).await["state"], "done");
    f.start(&id).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        if f.engine
            .store
            .site(&id)
            .unwrap()
            .status_detail
            .unwrap_or_default()
            .contains("five exits")
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(f.status(&id).await["lastExitCode"], 9);
    f.engine.shutdown().await;
}

#[tokio::test]
#[ignore = "requires real Node and Caddy; build cancellation and persisted state"]
async fn live_build_cancel_and_interrupted_state() {
    let f = Fixture::new();
    let id = f.add(APP).await;
    let mut config = f.engine.store.app_config(&id).unwrap();
    config.install = hostify_core::app_config::Install::Install;
    f.engine.store.save_app_config(&id, &config).unwrap();
    // No dependencies: real npm install runs offline and ignore-scripts suppresses this hook.
    std::fs::write(f.source.join("package.json"),r#"{"name":"fixture","scripts":{"start":"node server.cjs","build":"node build.cjs","postinstall":"node -e \"process.exit(42)\""}}"#).unwrap();
    f.update(&id, APP, "console.log('build dependencies available')")
        .await;
    assert_eq!(f.build(&id).await["state"], "done");
    f.update(&id, APP, "setInterval(()=>{},1000)").await;
    f.engine
        .dispatch("build_app", json!({"siteId":id,"trustAcknowledged":true}))
        .await
        .unwrap();
    assert!(
        f.engine
            .dispatch("build_app", json!({"siteId":id,"trustAcknowledged":true}))
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    f.engine
        .dispatch("cancel_build", json!({"siteId":id}))
        .await
        .unwrap();
    assert_eq!(
        f.engine
            .dispatch("get_build_status", json!({"siteId":id}))
            .await
            .unwrap()["state"],
        "cancelled"
    );
    f.engine
        .store
        .save(
            &format!("build:{id}"),
            &json!({"siteId":id,"state":"running","step":"build"}),
        )
        .unwrap();
    let reopened = Store::open(f.engine.store.root.clone()).unwrap();
    let status = reopened.saved(&format!("build:{id}")).unwrap().unwrap();
    assert_eq!(status["state"], "failed");
    assert!(status["detail"].as_str().unwrap().contains("interrupted"));
    f.engine.shutdown().await;
}

struct NoSecrets;
impl hostify_core::secrets::SecretStore for NoSecrets {
    fn get(&self, _: &str) -> Result<Option<String>, hostify_core::secrets::SecretError> {
        Ok(None)
    }
    fn set(&self, _: &str, _: &str) -> Result<(), hostify_core::secrets::SecretError> {
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<(), hostify_core::secrets::SecretError> {
        Ok(())
    }
}
#[tokio::test]
#[ignore = "requires working Windows DPAPI, real Node and installed Caddy"]
async fn live_secret_environment_is_redacted() {
    let f = Fixture::new();
    let id = f.add("console.log(process.env.PRIVATE_VALUE);").await;
    f.engine.dispatch("set_env_var",json!({"siteId":id,"name":"PRIVATE_VALUE","value":"do-not-log-this-secret","secret":true})).await.unwrap();
    f.update(
        &id,
        &format!("console.log(process.env.PRIVATE_VALUE);{APP}"),
        "console.log(process.env.PRIVATE_VALUE)",
    )
    .await;
    assert_eq!(f.build(&id).await["state"], "done");
    f.start(&id).await;
    for stream in ["app", "build"] {
        let logs = f
            .engine
            .dispatch("app_logs", json!({"siteId":id,"stream":stream,"lines":100}))
            .await
            .unwrap();
        assert!(logs.to_string().contains("[redacted]"));
        assert!(!logs.to_string().contains("do-not-log-this-secret"));
    }
    f.engine.shutdown().await;
}

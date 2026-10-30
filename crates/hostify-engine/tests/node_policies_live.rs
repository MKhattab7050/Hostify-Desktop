#![cfg(windows)]
//! Real Node runtime and real Caddy. Isolated data directories; nothing touches a real installation.
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
fn node_exe() -> PathBuf {
    live_root().join("runtimes/node/24.22.2/node.exe")
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
        copy_tree(
            &live_root().join("runtimes/node/24.22.2"),
            &root.join("runtimes/node/24.22.2"),
        );
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
            json!({"name":"Live","sourcePath":source,"kind":"directory","siteKind":"node"});
        if let Some(config) = config {
            request["config"] = config;
        }
        let site = self.call("add_site", request).await;
        let id = site["id"].as_str().unwrap().to_owned();
        let config = self.engine.store.app_config(&id).unwrap();
        self.call(
            "set_app_config",
            json!({"siteId":id,"config":config,"nodeVersion":"24.22.2"}),
        )
        .await;
        id
    }
    async fn add_code(&self, code: &str, restart: &str, memory: Option<u64>) -> String {
        let source = self.temp.path().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("server.cjs"), code).unwrap();
        std::fs::write(source.join("package.json"), r#"{"name":"policy"}"#).unwrap();
        let mut config = json!({"install":"none","start":{"type":"node","args":["server.cjs"]},"restart":restart});
        if let Some(memory) = memory {
            config["memoryLimitMb"] = json!(memory);
        }
        self.add_project(&source, Some(config)).await
    }
    async fn build(&self, id: &str) {
        let build = self
            .call("build_app", json!({"siteId":id,"trustAcknowledged":true}))
            .await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        loop {
            let status = self.call("get_build_status", json!({"siteId":id})).await;
            assert_eq!(status["buildId"], build["buildId"]);
            assert_ne!(status["state"], "failed", "{status}");
            let config = self.engine.store.app_config(id).unwrap();
            if config.built_release.is_some() && config.built_release == config.pending_release {
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
    async fn pid(&self, id: &str) -> Option<u64> {
        self.call("app_status", json!({"siteId":id})).await["pid"].as_u64()
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
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        socket
            .write_all(
                format!(
                    "GET {path} HTTP/1.1\r\nHost: foreign.example\r\nConnection: close\r\n\r\n"
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
http.createServer((req,res)=>{
 if(req.url==='/crash'){res.end('crashing');setTimeout(()=>process.exit(7),30);return;}
 if(req.url==='/finish'){res.end('finishing');setTimeout(()=>process.exit(0),30);return;}
 if(req.url==='/eat'){try{const b=Buffer.alloc(900*1024*1024,1);res.end('alloc-ok:'+b.length);}catch(e){res.end('alloc-failed');}return;}
 res.end('policy:'+process.pid);
}).listen(Number(process.env.PORT));
"#;

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and installed Caddy"]
async fn live_never_leaves_the_app_stopped_with_its_exit_code() {
    let f = Fixture::new();
    let id = f.add_code(APP, "never", None).await;
    f.build(&id).await;
    f.start(&id).await;
    assert!(get(f.port(&id), "/").await.contains("policy:"));
    assert!(get(f.port(&id), "/crash").await.contains("crashing"));
    let status = f
        .wait_status(&id, 10, |s| s["lastExitCode"] == 7 && s["pid"].is_null())
        .await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(f.pid(&id).await.is_none(), "{status}");
    let site = f.engine.store.site(&id).unwrap();
    assert_ne!(site.status, "running", "{site:?}");
    println!(
        "never: status={} detail={:?}",
        site.status, site.status_detail
    );
    f.engine.shutdown().await;
    f.engine.supervisor.as_ref().unwrap().shutdown().await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and installed Caddy"]
async fn live_on_failure_restarts_a_crash_but_not_a_clean_exit() {
    let f = Fixture::new();
    let id = f.add_code(APP, "onFailure", None).await;
    f.build(&id).await;
    f.start(&id).await;
    let first = f.pid(&id).await.unwrap();
    get(f.port(&id), "/crash").await;
    f.wait_status(&id, 20, |s| {
        s["pid"].as_u64().is_some_and(|p| p != first) && s["lastExitCode"] == 7
    })
    .await;
    // The restarted process needs its port back before it can answer.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !get(f.port(&id), "/").await.contains("policy:") {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    get(f.port(&id), "/finish").await;
    f.wait_status(&id, 10, |s| s["lastExitCode"] == 0 && s["pid"].is_null())
        .await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(f.pid(&id).await.is_none());
    f.engine.shutdown().await;
    f.engine.supervisor.as_ref().unwrap().shutdown().await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and installed Caddy"]
async fn live_memory_limit_is_enforced_by_the_job() {
    let f = Fixture::new();
    let limited = f.add_code(APP, "never", Some(256)).await;
    let free = f.add_code(APP, "never", None).await;
    for id in [&limited, &free] {
        f.build(id).await;
        f.start(id).await;
    }
    // Control: the same request succeeds without a limit, so a refusal is the limit's doing.
    assert!(get(f.port(&free), "/eat").await.contains("alloc-ok"));
    let limited_port = f.port(&limited);
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::net::TcpStream::connect(("127.0.0.1", limited_port)),
    )
    .await
    .unwrap();
    assert!(response.is_ok());
    let before = f.call("app_status", json!({"siteId":limited})).await;
    println!("memory before the attempt: {before}");
    assert!(
        before["peakMemoryBytes"].as_u64().unwrap() < 256 * 1024 * 1024,
        "{before}"
    );
    let body = get(limited_port, "/eat").await;
    assert!(!body.contains("alloc-ok"), "{body}");
    let status = f.call("app_status", json!({"siteId":limited})).await;
    println!(
        "memory limit: body={:?} status={status}",
        body.lines().last()
    );
    f.engine.shutdown().await;
    f.engine.supervisor.as_ref().unwrap().shutdown().await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES"]
async fn live_every_listen_form_without_a_host_uses_loopback() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("forms.cjs");
    std::fs::write(
        &script,
        r#"
const net=require('node:net'),http=require('node:http');
const forms={
 none:s=>s.listen(),
 cb:(s,c)=>s.listen(c),
 zero:(s,c)=>s.listen(0,c),
 zeroBacklog:(s,c)=>s.listen(0,50,c),
 text:(s,c)=>s.listen('0',c),
 undef:(s,c)=>s.listen(undefined,c),
 object:(s,c)=>s.listen({port:0},c),
 explicit:(s,c)=>s.listen(0,'0.0.0.0',c),
 explicitObject:(s,c)=>s.listen({port:0,host:'0.0.0.0'},c),
};
(async()=>{for(const [name,form] of Object.entries(forms)){
 const s=http.createServer();
 await new Promise(r=>{s.once('listening',r);form(s,()=>{});});
 console.log(name+'='+s.address().address);
 s.close();
}})();
"#,
    )
    .unwrap();
    let preload = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/loopback.cjs");
    let output = std::process::Command::new(node_exe())
        .arg("--require")
        .arg(preload)
        .arg(&script)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in text.lines() {
        let (name, address) = line.split_once('=').unwrap();
        let expected = if name.starts_with("explicit") {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        };
        assert_eq!(address, expected, "{name}");
    }
    assert_eq!(text.lines().count(), 9, "{text}");
}

/// Real `npm ci` over the network, so it needs internet and is kept apart from the fixture tests.
#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES, installed Caddy and access to the npm registry"]
async fn live_real_express_and_socketio_samples() {
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples");
    let f = Fixture::new();

    let source = samples.join("express-http");
    let report = f
        .call("inspect_project", json!({"path":source,"kind":"directory"}))
        .await;
    assert_eq!(report["siteKind"], "node", "{report}");
    assert_eq!(report["framework"], "express", "{report}");
    let express = f.add_project(&source, None).await;
    f.build(&express).await;
    f.start(&express).await;
    let health = get(f.port(&express), "/health").await;
    assert!(health.contains(r#""sample":"express""#), "{health}");
    let first = f.pid(&express).await.unwrap();
    get(f.port(&express), "/crash").await;
    f.wait_status(&express, 20, |s| {
        s["pid"].as_u64().is_some_and(|p| p != first) && s["lastExitCode"] == 42
    })
    .await;

    let source = samples.join("socketio-chat");
    let report = f
        .call("inspect_project", json!({"path":source,"kind":"directory"}))
        .await;
    assert_eq!(report["framework"], "socket.io", "{report}");
    let chat = f.add_project(&source, None).await;
    f.build(&chat).await;
    f.start(&chat).await;
    // Through Caddy on a real WebSocket, and across a server heartbeat so the connection has really stayed up.
    let script = f.temp.path().join("client.cjs");
    std::fs::write(
        &script,
        r#"
const ws=new WebSocket(`ws://127.0.0.1:${process.argv[2]}/socket.io/?EIO=4&transport=websocket`);
let beats=0;
ws.onmessage=e=>{const m=String(e.data);
 if(m[0]==='0')ws.send('40');
 else if(m==='2'){ws.send('3');beats++;ws.send('42["chat","through-caddy"]');}
 else if(m.includes('through-caddy')){console.log('ok beats='+beats);process.exit(0);}
};
ws.onerror=e=>{console.error(String(e.message));process.exit(2);};
setTimeout(()=>{console.error('timed out');process.exit(3);},60000);
"#,
    )
    .unwrap();
    let output = tokio::process::Command::new(node_exe())
        .arg(&script)
        .arg(f.port(&chat).to_string())
        .output()
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("ok beats=1"), "{text}");

    f.call("stop_site", json!({"id":express})).await;
    let logs = f
        .call(
            "app_logs",
            json!({"siteId":express,"stream":"app","lines":50}),
        )
        .await;
    println!("express app log: {logs}");
    assert!(logs.to_string().contains("graceful shutdown"), "{logs}");
    f.engine.shutdown().await;
    f.engine.supervisor.as_ref().unwrap().shutdown().await;
}

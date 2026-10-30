#![cfg(windows)]
use async_trait::async_trait;
use hostify_core::deploy::SiteRuntime;
use hostify_engine::{
    pipe,
    state::{Engine, Store},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[derive(Default)]
struct Fake {
    running: Mutex<std::collections::HashSet<String>>,
}
#[async_trait]
impl SiteRuntime for Fake {
    async fn start_site(&self, id: &str) -> Result<bool, String> {
        Ok(self
            .running
            .lock()
            .map_err(|e| e.to_string())?
            .insert(id.into()))
    }
    async fn stop_site(&self, id: &str) -> Result<(), String> {
        self.running.lock().map_err(|e| e.to_string())?.remove(id);
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
struct Probe;
impl hostify_core::components::ProbeRunner for Probe {
    fn version(&self, _: &std::path::Path, _: &str) -> Result<String, String> {
        Ok("test caddy".into())
    }
}
async fn call(
    pipe: &mut BufReader<tokio::net::windows::named_pipe::NamedPipeClient>,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    let mut request =
        serde_json::to_vec(&json!({"id":id,"method":method,"params":params})).expect("request");
    request.push(b'\n');
    pipe.get_mut().write_all(&request).await.expect("write");
    let mut line = String::new();
    loop {
        line.clear();
        tokio::time::timeout(std::time::Duration::from_secs(5), pipe.read_line(&mut line))
            .await
            .expect("response timeout")
            .expect("read");
        assert!(!line.is_empty());
        let response: Value = serde_json::from_str(&line).expect("response JSON");
        if response.get("event").is_some() {
            continue;
        }
        assert_eq!(response["id"], id);
        assert!(response.get("error").is_none(), "{response}");
        return response["result"].clone();
    }
}
#[tokio::test]
async fn pipe_lifecycle_with_fake_runtime() {
    let dir = tempfile::tempdir().expect("temp");
    let source = dir.path().join("source");
    std::fs::create_dir(&source).expect("source");
    std::fs::write(source.join("index.html"), "hello").expect("index");
    let store = Store::open_with_probe(dir.path().join("data"), Arc::new(Probe)).expect("store");
    let runtime = Arc::new(Fake::default());
    let engine = Engine::new(store.clone(), runtime.clone(), false);
    let name = format!(r"\\.\pipe\hostify-test-{}", uuid::Uuid::new_v4());
    let (tx, rx) = tokio::sync::watch::channel(false);
    let server_name = name.clone();
    let server = tokio::spawn(async move { pipe::serve(server_name, engine, rx).await });
    let mut client = None;
    for _ in 0..100 {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(&name) {
            Ok(c) => {
                client = Some(c);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    let mut client = BufReader::new(client.expect("connect"));
    assert_eq!(call(&mut client, 1, "ping", json!({})).await, "pong");
    assert_eq!(
        call(&mut client, 2, "list_sites", json!({})).await,
        json!([])
    );
    let status = call(&mut client, 20, "engine_status", json!({})).await;
    assert_eq!(status["dataDir"], store.root.to_string_lossy().as_ref());
    assert_eq!(
        status["binDir"],
        store.root.join("bin").to_string_lossy().as_ref()
    );
    for name in ["caddy", "cloudflared"] {
        assert_eq!(status[name]["installed"], false);
        assert_eq!(status[name]["ready"], false);
        assert!(status[name]["path"].is_string());
        assert!(status[name]["error"].is_string());
    }
    assert_eq!(status["localHostingReady"], false);
    assert_eq!(status["componentsReady"], false);
    call(&mut client, 3, "subscribe", json!({})).await;
    let site = call(
        &mut client,
        4,
        "add_site",
        json!({"name":"test","sourcePath":source,"kind":"directory"}),
    )
    .await;
    let id = site["id"].as_str().expect("id");
    assert_eq!(site["fileCount"], 1);
    assert_eq!(site["status"], "stopped");
    assert!(
        store
            .root
            .join("sites")
            .join(id)
            .join("current/index.html")
            .is_file()
    );
    // Missing Caddy fails before reaching even the fake runtime and stores the action.
    let error = engine_missing_start(&mut client, id).await;
    assert_eq!(error, hostify_core::components::CADDY_UNAVAILABLE);
    assert!(runtime.running.lock().unwrap().is_empty());
    assert_eq!(
        store.site(id).unwrap().status_detail.as_deref(),
        Some(error.as_str())
    );
    std::fs::write(
        store.root.join("bin").join(hostify_core::paths::CADDY_EXE),
        b"fake executable",
    )
    .unwrap();
    store.detection.invalidate().await;
    let status = call(&mut client, 22, "engine_status", json!({})).await;
    assert_eq!(status["caddy"]["ready"], true);
    assert_eq!(status["caddy"]["version"], "test caddy");
    assert_eq!(status["localHostingReady"], true);
    assert_eq!(status["componentsReady"], false);
    assert_eq!(status["cloudflared"]["installed"], false);
    let started = call(&mut client, 5, "start_site", json!({"id":id})).await;
    assert_eq!(started["status"], "running");
    assert!(runtime.running.lock().expect("lock").contains(id));
    let stopped = call(&mut client, 6, "stop_site", json!({"id":id})).await;
    assert_eq!(stopped["status"], "stopped");
    assert!(!runtime.running.lock().expect("lock").contains(id));
    assert_eq!(
        call(
            &mut client,
            7,
            "remove_site",
            json!({"id":id,"deleteFiles":true})
        )
        .await,
        Value::Null
    );
    assert_eq!(
        call(&mut client, 8, "list_sites", json!({})).await,
        json!([])
    );
    assert!(!store.root.join("sites").join(id).exists());
    tx.send(true).expect("shutdown");
    server.await.expect("join").expect("server");
}

async fn engine_missing_start(
    pipe: &mut BufReader<tokio::net::windows::named_pipe::NamedPipeClient>,
    id: &str,
) -> String {
    let request = json!({"id":21,"method":"start_site","params":{"id":id}}).to_string() + "\n";
    pipe.get_mut().write_all(request.as_bytes()).await.unwrap();
    loop {
        let mut line = String::new();
        pipe.read_line(&mut line).await.unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["id"] == 21 {
            return v["error"].as_str().unwrap().to_owned();
        }
    }
}

async fn fixture() -> (
    tempfile::TempDir,
    Arc<Store>,
    Arc<Fake>,
    Arc<Engine>,
    String,
) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("index.html"), "hello").unwrap();
    let store = Store::open_with_probe(dir.path().join("data"), Arc::new(Probe)).unwrap();
    let runtime = Arc::new(Fake::default());
    let engine = Engine::new(store.clone(), runtime.clone(), false);
    let site = engine
        .dispatch(
            "add_site",
            json!({"name":"test","sourcePath":source,"kind":"directory"}),
        )
        .await
        .unwrap();
    (
        dir,
        store,
        runtime,
        engine,
        site["id"].as_str().unwrap().to_owned(),
    )
}
#[tokio::test]
async fn missing_caddy_start_and_restart_do_not_spawn_and_set_status_detail() {
    let (_dir, store, runtime, engine, id) = fixture().await;
    for method in ["start_site", "restart_site"] {
        let e = engine
            .dispatch(method, json!({"id":id}))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(e, hostify_core::components::CADDY_UNAVAILABLE);
        assert_eq!(store.site(&id).unwrap().status_detail, Some(e));
        assert!(runtime.running.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn local_start_and_restart_succeed_without_cloudflared() {
    let (_dir, store, runtime, engine, id) = fixture().await;
    std::fs::write(
        store.root.join("bin").join(hostify_core::paths::CADDY_EXE),
        b"fake exe",
    )
    .unwrap();
    for method in ["start_site", "restart_site"] {
        assert_eq!(
            engine.dispatch(method, json!({"id":id})).await.unwrap()["status"],
            "running"
        );
    }
    assert!(runtime.running.lock().unwrap().contains(&id));
    let status = engine.engine_status().await;
    assert_eq!(status["localHostingReady"], true);
    assert_eq!(status["componentsReady"], false);
}
#[tokio::test]
async fn publish_preflight_names_exactly_the_unready_components() {
    let (_dir, store, runtime, engine, id) = fixture().await;
    for (caddy, cloudflared, names) in [
        (false, false, "Caddy and cloudflared"),
        (true, false, "cloudflared"),
        (false, true, "Caddy"),
    ] {
        for (name, present) in [
            (hostify_core::paths::CADDY_EXE, caddy),
            (hostify_core::paths::CLOUDFLARED_EXE, cloudflared),
        ] {
            let path = store.root.join("bin").join(name);
            if present {
                std::fs::write(path, b"fake").unwrap();
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
        let e = engine
            .dispatch(
                "publish_site",
                json!({"siteId":id,"hostname":"example.com","consentDns":true}),
            )
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            e,
            format!(
                "Cannot publish: {names} is not installed or can't run. Open the Dashboard and choose \"Install components\", then publish again."
            )
        );
        assert!(runtime.running.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn ensure_components_validates_names_and_supports_caddy_only() {
    let (_dir, store, _runtime, engine, _id) = fixture().await;
    assert!(
        engine
            .dispatch("ensure_components", json!({"names":["invalid"]}))
            .await
            .unwrap_err()
            .to_string()
            .contains("Unknown component")
    );
    std::fs::write(
        store.root.join("bin").join(hostify_core::paths::CADDY_EXE),
        b"fake",
    )
    .unwrap();
    let status = engine
        .dispatch("ensure_components", json!({"names":["caddy"]}))
        .await
        .unwrap();
    assert_eq!(status["localHostingReady"], true);
    assert_eq!(status["componentsReady"], false);
    assert!(
        !store
            .root
            .join("bin")
            .join(hostify_core::paths::CLOUDFLARED_EXE)
            .exists()
    );
}

#[tokio::test]
async fn engine_status_observes_immediate_external_empty_replace_and_delete() {
    let (_dir, store, _runtime, engine, _id) = fixture().await;
    for name in [
        hostify_core::paths::CADDY_EXE,
        hostify_core::paths::CLOUDFLARED_EXE,
    ] {
        std::fs::write(store.root.join("bin").join(name), b"fake executable").unwrap();
    }
    assert_eq!(engine.engine_status().await["componentsReady"], true);
    for name in ["caddy", "cloudflared"] {
        let path = store
            .root
            .join("bin")
            .join(hostify_core::components::executable(name).unwrap());
        std::fs::write(&path, b"").unwrap();
        let status = engine.engine_status().await;
        assert_eq!(status[name]["installed"], true);
        assert_eq!(status[name]["ready"], false);
        assert!(status[name]["error"].as_str().unwrap().contains("empty"));
        assert_eq!(status["componentsReady"], false);
        std::fs::write(&path, b"replacement executable").unwrap();
        assert_eq!(engine.engine_status().await[name]["ready"], true);
        std::fs::remove_file(&path).unwrap();
        let status = engine.engine_status().await;
        assert_eq!(status[name]["installed"], false);
        assert_eq!(status[name]["ready"], false);
    }
}

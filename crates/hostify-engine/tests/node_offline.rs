use hostify_engine::{
    state::{Engine, Store},
    supervisor::Supervisor,
};
use serde_json::json;
#[tokio::test]
async fn node_import_only_allocates_ports_and_requires_trust() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(
        source.join("server.js"),
        "throw new Error('must never execute during import')",
    )
    .unwrap();
    let store = Store::open(temp.path().join("data")).unwrap();
    let supervisor = Supervisor::new(store.clone()).unwrap();
    let engine = Engine::production(store.clone(), supervisor, false);
    let site = engine
        .dispatch(
            "add_site",
            json!({"name":"Node","sourcePath":source,"kind":"directory","siteKind":"node"}),
        )
        .await
        .unwrap();
    let id = site["id"].as_str().unwrap();
    assert_eq!(site["status"], "stopped");
    let config = store.app_config(id).unwrap();
    assert!(config.pending_release.is_some());
    assert!(config.active_release.is_none());
    assert!(config.trusted_release.is_none());
    assert!(config.built_release.is_none());
    assert_ne!(
        config.internal_port.unwrap(),
        site["localPort"].as_u64().unwrap() as u16
    );
    assert!(
        engine
            .dispatch("build_app", json!({"siteId":id,"trustAcknowledged":false}))
            .await
            .unwrap_err()
            .to_string()
            .contains("trust")
    );
    assert!(store.app_config(id).unwrap().trusted_release.is_none());
    assert!(
        engine
            .dispatch("build_app", json!({"siteId":id,"trustAcknowledged":true}))
            .await
            .unwrap_err()
            .to_string()
            .contains("Runtimes")
    );
    let php = engine
        .dispatch(
            "add_site",
            json!({"name":"PHP","sourcePath":source,"kind":"directory","siteKind":"php"}),
        )
        .await
        .unwrap();
    assert_eq!(php["kind"], "php");
    assert_eq!(php["status"], "stopped");
    assert!(hostify_core::runtimes::is_pinned(
        "php",
        php["runtimeVersion"].as_str().unwrap()
    ));
    let php_id = php["id"].as_str().unwrap();
    let config = store.app_config(php_id).unwrap();
    assert!(config.trusted_release.is_none() && config.built_release.is_none());
    assert!(config.internal_port.is_none() && config.php_ports.is_empty());
    assert!(
        engine
            .dispatch(
                "set_app_config",
                json!({"siteId":php_id,"config":config,"phpVersion":"99.0.0"})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("pinned")
    );
    let mut requested = serde_json::to_value(&config).unwrap();
    requested["phpPorts"] = json!([1, 2]);
    engine
        .dispatch(
            "set_app_config",
            json!({"siteId":php_id,"config":requested,"phpVersion":"8.4.27-1"}),
        )
        .await
        .unwrap();
    assert!(store.app_config(php_id).unwrap().php_ports.is_empty());
    assert_eq!(
        store.site(php_id).unwrap().runtime_version.as_deref(),
        Some("8.4.27-1")
    );

    assert!(
        engine
            .dispatch(
                "build_app",
                json!({"siteId":php_id,"trustAcknowledged":false})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("trust")
    );
    engine.shutdown().await;
}
#[test]
fn interrupted_build_is_failed_when_store_reopens() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let store = Store::open(root.clone()).unwrap();
    store
        .save(
            "build:s",
            &json!({"state":"running","step":"install","exitCode":null}),
        )
        .unwrap();
    drop(store);
    let store = Store::open(root).unwrap();
    let status = store.saved("build:s").unwrap().unwrap();
    assert_eq!(status["state"], "failed");
    assert!(
        status["detail"]
            .as_str()
            .unwrap()
            .contains("engine restart")
    );
    assert!(status["exitCode"].is_null());
}

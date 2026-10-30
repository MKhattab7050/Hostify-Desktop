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
    assert!(
        engine
            .dispatch(
                "add_site",
                json!({"name":"PHP","sourcePath":source,"kind":"directory","siteKind":"php"})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("PHP hosting is not available")
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

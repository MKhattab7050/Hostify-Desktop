#![cfg(windows)]
//! Real MySQL management tests against official MySQL 8.4.10 runtime.
use hostify_engine::{
    state::{Engine, Store},
    supervisor::Supervisor,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn live_root() -> PathBuf {
    PathBuf::from(std::env::var_os("HOSTIFY_LIVE_RUNTIMES").expect("Set HOSTIFY_LIVE_RUNTIMES"))
}

fn link_dir(target: &Path, link: &Path) {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let target_str = target.to_str().unwrap().replace('/', "\\");
    let link_str = link.to_str().unwrap().replace('/', "\\");
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J", &link_str, &target_str])
        .status()
        .expect("mklink /J execution failed");
    assert!(status.success(), "mklink failed for {:?}", link);
}

fn unlink_dir(link: &Path) {
    let link_str = link.to_str().unwrap().replace('/', "\\");
    let _ = Command::new("cmd")
        .args(["/C", "rmdir", &link_str])
        .status();
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
    mysql_link: PathBuf,
    php_link: Option<PathBuf>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        let store = Store::open(root.clone()).unwrap();

        let caddy =
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("Hostify/bin/caddy.exe");
        if caddy.is_file() {
            std::fs::create_dir_all(root.join("bin")).unwrap();
            std::fs::copy(caddy, root.join("bin/caddy.exe")).unwrap();
        }

        let mysql_link = root.join("runtimes/mysql");
        link_dir(&live_root().join("runtimes/mysql"), &mysql_link);

        let supervisor = Supervisor::new(store.clone()).unwrap();
        let engine = Engine::production(store, supervisor, false);

        Self {
            engine,
            temp,
            mysql_link,
            php_link: None,
        }
    }

    fn with_php(mut self) -> Self {
        let root = self.temp.path().join("data");
        let php_link = root.join("runtimes/php");
        link_dir(&live_root().join("runtimes/php"), &php_link);
        self.php_link = Some(php_link);
        self
    }

    async fn call(&self, method: &str, params: Value) -> Value {
        self.engine
            .dispatch(method, params)
            .await
            .unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    fn mysql(&self) -> Arc<hostify_engine::mysql::Manager> {
        self.engine.supervisor.as_ref().unwrap().mysql.clone()
    }

    async fn setup_mysql(&self) {
        self.call("mysql_setup", json!({"version": "8.4.10"})).await;
        self.call("mysql_start", json!({})).await;
        let status = self.call("mysql_status", json!({})).await;
        assert_eq!(status["running"], true);
        assert_eq!(status["healthy"], true);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unlink_dir(&self.mysql_link);
        if let Some(link) = &self.php_link {
            unlink_dir(link);
        }
    }
}

async fn http_get(port: u16, path: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).await.unwrap();
    out
}

async fn http_post(port: u16, path: &str, body: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).await.unwrap();
    out
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real MySQL 8.4.10"]
async fn test_1_setup_and_security_audit() {
    let f = Fixture::new();
    f.setup_mysql().await;

    let status = f.call("mysql_status", json!({})).await;
    let port = status["port"].as_u64().unwrap() as u16;

    // Verify 127.0.0.2 is refused (bind-address=127.0.0.1 only)
    let non_loopback = tokio::net::TcpStream::connect(("127.0.0.2", port)).await;
    assert!(
        non_loopback.is_err(),
        "Non-127.0.0.1 connection was unexpectedly accepted"
    );

    // Verify X plugin (mysqlx) port 33060 or port+1000 is closed
    let x_port = tokio::net::TcpStream::connect(("127.0.0.1", 33060)).await;
    assert!(x_port.is_err(), "mysqlx port 33060 should be closed");

    // Query user accounts: root must only exist on localhost, no '%' host
    let root_users = f
        .mysql()
        .sql("SELECT host FROM mysql.user WHERE user='root';")
        .unwrap();
    assert!(root_users.contains("localhost"));
    assert!(!root_users.contains('%'));

    // Check my.ini does not contain root password
    let my_ini = std::fs::read_to_string(f.engine.store.root.join("mysql/my.ini")).unwrap();
    let root_pass = f.mysql().root_password().unwrap();
    assert!(!my_ini.contains(&root_pass));

    // Check logs do not contain root password
    let err_log = std::fs::read_to_string(f.engine.store.root.join("logs/mysql-error.log"))
        .unwrap_or_default();
    assert!(!err_log.contains(&root_pass));

    f.call("mysql_stop", json!({})).await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real MySQL 8.4.10"]
async fn test_2_least_privilege_databases() {
    let f = Fixture::new();
    f.setup_mysql().await;

    f.call("create_database", json!({"name": "alpha_db"})).await;
    f.call("create_database", json!({"name": "beta_db"})).await;

    let (_db_alpha, p_alpha) = f.mysql().user("alpha_db_app").unwrap();
    let (_db_beta, _p_beta) = f.mysql().user("beta_db_app").unwrap();
    let u_alpha = "alpha_db_app";

    // User alpha can create tables and insert in alpha_db
    let sql_ok = f.mysql().run_sql(
        u_alpha,
        &p_alpha,
        Some("alpha_db"),
        "CREATE TABLE test_tbl (id INT); INSERT INTO test_tbl VALUES (1); SELECT * FROM test_tbl;",
    );
    assert!(
        sql_ok.is_ok(),
        "alpha user should have table privileges on alpha_db: {:?}",
        sql_ok
    );

    // User alpha CANNOT read beta_db
    let read_beta = f.mysql().run_sql(
        u_alpha,
        &p_alpha,
        Some("alpha_db"),
        "SELECT * FROM beta_db.test_tbl;",
    );
    assert!(read_beta.is_err(), "alpha user must NOT read beta_db");

    // User alpha CANNOT read mysql.user
    let read_users = f.mysql().run_sql(
        u_alpha,
        &p_alpha,
        Some("alpha_db"),
        "SELECT * FROM mysql.user;",
    );
    assert!(read_users.is_err(), "alpha user must NOT read mysql.user");

    // User alpha CANNOT create users or grant permissions
    let create_user = f.mysql().run_sql(
        u_alpha,
        &p_alpha,
        Some("alpha_db"),
        "CREATE USER 'hacker'@'127.0.0.1' IDENTIFIED BY 'Pass1234567890123456789012345678';",
    );
    assert!(
        create_user.is_err(),
        "alpha user must NOT be able to create users"
    );

    f.call("mysql_stop", json!({})).await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real MySQL 8.4.10"]
async fn test_3_backup_and_restore_round_trip() {
    let f = Fixture::new();
    f.setup_mysql().await;

    f.call("create_database", json!({"name": "store_db"})).await;
    let (_db, p) = f.mysql().user("store_db_app").unwrap();
    let u = "store_db_app";

    // Populate initial data
    f.mysql().run_sql(
        u,
        &p,
        Some("store_db"),
        "CREATE TABLE items (id INT, name VARCHAR(50)); INSERT INTO items VALUES (1, 'Apples'), (2, 'Oranges');",
    ).unwrap();

    // Take backup
    let b = f.call("backup_database", json!({"name": "store_db"})).await;
    let backup_file = b["file"].as_str().unwrap();

    // Insert more items
    f.mysql()
        .run_sql(
            u,
            &p,
            Some("store_db"),
            "INSERT INTO items VALUES (3, 'Bananas'), (4, 'Pears');",
        )
        .unwrap();

    // Truncated backup test: creating a bad backup file must be rejected
    let bad_file = f
        .engine
        .store
        .root
        .join("backups/mysql/store_db/corrupt.sql");
    std::fs::write(&bad_file, "INSERT INTO items VALUES (999, 'Bad');").unwrap();
    let restore_bad = f
        .engine
        .dispatch(
            "restore_database",
            json!({"name": "store_db", "file": "corrupt.sql", "confirmName": "store_db"}),
        )
        .await;
    assert!(
        restore_bad.is_err(),
        "Truncated dump without completion comment must be rejected"
    );

    // Restore from genuine backup
    let restore_ok = f
        .call(
            "restore_database",
            json!({"name": "store_db", "file": backup_file, "confirmName": "store_db"}),
        )
        .await;
    assert!(restore_ok["safetyBackup"].is_string());

    // Verify exactly 2 rows remain
    let count = f
        .mysql()
        .run_sql(u, &p, Some("store_db"), "SELECT count(*) FROM items;")
        .unwrap();
    assert_eq!(count.trim(), "2");

    f.call("mysql_stop", json!({})).await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real MySQL 8.4.10"]
async fn test_4_durability_and_process_recovery() {
    let f = Fixture::new();
    f.setup_mysql().await;

    f.call("create_database", json!({"name": "durable_db"}))
        .await;
    let (_db, p) = f.mysql().user("durable_db_app").unwrap();
    let u = "durable_db_app";

    f.mysql()
        .run_sql(
            u,
            &p,
            Some("durable_db"),
            "CREATE TABLE records (k VARCHAR(20)); INSERT INTO records VALUES ('persist');",
        )
        .unwrap();

    // Graceful stop and restart
    f.call("mysql_stop", json!({})).await;
    f.call("mysql_start", json!({})).await;

    let res = f
        .mysql()
        .run_sql(u, &p, Some("durable_db"), "SELECT k FROM records;")
        .unwrap();
    assert_eq!(res.trim(), "persist");

    // Force kill the mysqld process to verify InnoDB crash recovery
    let status = f.call("mysql_status", json!({})).await;
    let pid = status["pid"].as_u64().unwrap() as u32;
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output();

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Start again: engine should start and recover InnoDB cleanly
    f.call("mysql_start", json!({})).await;
    let res_after_kill = f
        .mysql()
        .run_sql(u, &p, Some("durable_db"), "SELECT k FROM records;")
        .unwrap();
    assert_eq!(res_after_kill.trim(), "persist");

    f.call("mysql_stop", json!({})).await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real MySQL 8.4.10"]
async fn test_5_delete_database_safety() {
    let f = Fixture::new();
    f.setup_mysql().await;

    f.call("create_database", json!({"name": "safe_db"})).await;

    // Reject wrong confirmName
    let wrong_name = f
        .engine
        .dispatch(
            "delete_database",
            json!({"name": "safe_db", "confirmName": "wrong"}),
        )
        .await;
    assert!(
        wrong_name.is_err(),
        "Mismatching confirmName must be rejected"
    );

    // Create an unmanaged database outside Hostify
    f.mysql()
        .sql("CREATE DATABASE external_unmanaged;")
        .unwrap();

    let delete_unmanaged = f
        .engine
        .dispatch(
            "delete_database",
            json!({"name": "external_unmanaged", "confirmName": "external_unmanaged"}),
        )
        .await;
    assert!(
        delete_unmanaged.is_err(),
        "Unmanaged databases must not be deleted"
    );

    f.call("mysql_stop", json!({})).await;
}

#[tokio::test]
#[ignore = "requires HOSTIFY_LIVE_RUNTIMES and real Caddy and PHP"]
async fn test_8_sample_php_mysql_end_to_end() {
    let f = Fixture::new().with_php();
    f.setup_mysql().await;

    let sample_src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/php-mysql");
    let dest_sample = f.temp.path().join("php-mysql-sample");
    copy_tree(&sample_src, &dest_sample);

    let site = f
        .call(
            "add_site",
            json!({
                "name": "PHP MySQL Sample",
                "sourcePath": dest_sample,
                "kind": "directory",
                "siteKind": "php",
                "config": {
                    "documentRoot": ".",
                    "phpWorkers": 2,
                    "phpExtensions": ["pdo_mysql", "mysqli", "mbstring", "openssl", "curl"],
                    "healthPath": "/health"
                }
            }),
        )
        .await;
    let site_id = site["id"].as_str().unwrap();

    // Create database and inject DB_* env vars
    f.call(
        "create_database",
        json!({"name": "sample_notes", "siteId": site_id, "injectEnv": true, "replaceEnv": true}),
    )
    .await;

    // Build and start the PHP site
    f.call(
        "build_app",
        json!({"siteId": site_id, "trustAcknowledged": true}),
    )
    .await;
    loop {
        let st = f.call("get_build_status", json!({"siteId": site_id})).await;
        if st["state"] == "done" {
            break;
        }
        assert_ne!(st["state"], "failed", "PHP build failed: {st}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    f.call("start_site", json!({"id": site_id})).await;
    let local_port = f.call("list_sites", json!({})).await[0]["localPort"]
        .as_u64()
        .unwrap() as u16;

    // Test /health endpoint
    let health_resp = http_get(local_port, "/health").await;
    assert!(
        health_resp.contains("\"ok\":true"),
        "Health endpoint failed: {health_resp}"
    );
    assert!(health_resp.contains("php-mysql"));

    // POST a note to /notes
    let post_resp = http_post(local_port, "/notes", "body=Verified+by+Gemini+QA").await;
    assert!(
        post_resp.contains("201 Created"),
        "POST note failed: {post_resp}"
    );
    assert!(post_resp.contains("\"id\":1"));

    // GET /notes/1
    let get_resp = http_get(local_port, "/notes/1").await;
    assert!(
        get_resp.contains("Verified by Gemini QA"),
        "GET note failed: {get_resp}"
    );

    f.call("stop_site", json!({"id": site_id})).await;
    f.call("mysql_stop", json!({})).await;
}

use super::*;
use crate::verify::{HttpEvidence, PublicProbe, VerifyClass};
use crate::{db, secrets::MemoryStore};
use serde_json::{Value, json};
use std::net::IpAddr;
use std::time::Duration;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
struct Remote {
    tunnels: Vec<Value>,
    records: Vec<Value>,
    config: Value,
    config_reads_fail: bool,
    writes: Vec<String>,
    delayed_create: Option<&'static str>,
    add_concurrent: bool,
    add_after_restore: bool,
    fail: Option<&'static str>,
}
#[derive(Clone)]
struct Api {
    remote: Arc<Mutex<Remote>>,
    db: Arc<Mutex<Connection>>,
}
impl Respond for Api {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut r = self.remote.lock().unwrap();
        let path = req.url.path();
        let method = req.method.as_str();
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        if method != "GET" {
            r.writes.push(format!("{method} {path}"));
            // The actual Cloudflare POST is observed only after its intent is durable.
            if method == "POST" {
                let db = self.db.lock().unwrap();
                let raw:String=db.query_row("SELECT created_resources_json FROM deployments WHERE state IN ('running','validating','starting_local','provisioning_tunnel','configuring_dns','starting_connector') ORDER BY rowid DESC LIMIT 1",[],|r|r.get(0)).unwrap();
                let journal: Resources = serde_json::from_str(&raw).unwrap();
                if path.ends_with("cfd_tunnel") {
                    assert_eq!(journal.tunnel_intent.as_deref(), body["name"].as_str());
                } else {
                    assert!(journal.dns_intent);
                    assert!(body.get("tags").is_none());
                }
            }
            if method == "PUT" && path.ends_with("configurations") {
                let raw:String=self.db.lock().unwrap().query_row("SELECT created_resources_json FROM deployments ORDER BY rowid DESC LIMIT 1",[],|r|r.get(0)).unwrap();
                let journal: Resources = serde_json::from_str(&raw).unwrap();
                assert!(
                    journal.previous_config.is_some()
                        || journal.tunnel_created
                        || journal.ingress_pending
                );
            }
        }
        let result = match (method, path) {
            ("GET", "/accounts/a/cfd_tunnel") => json!(r.tunnels),
            ("POST", "/accounts/a/cfd_tunnel") => {
                let t = json!({"id":"t","name":body["name"]});
                r.tunnels.push(t.clone());
                t
            }
            ("GET", "/accounts/a/cfd_tunnel/t/token") => {
                let raw:String=self.db.lock().unwrap().query_row("SELECT created_resources_json FROM deployments ORDER BY rowid DESC LIMIT 1",[],|r|r.get(0)).unwrap();
                let journal: Resources = serde_json::from_str(&raw).unwrap();
                assert_eq!(journal.tunnel_id.as_deref(), Some("t"));
                if r.fail == Some("token") {
                    return ResponseTemplate::new(403);
                }
                json!("secret")
            }
            ("GET", "/accounts/a/cfd_tunnel/t/configurations") => {
                if r.config_reads_fail || r.fail == Some("config") {
                    return ResponseTemplate::new(500);
                }
                if r.config == json!("missing") {
                    json!({})
                } else {
                    json!({"config":r.config})
                }
            }
            ("PUT", "/accounts/a/cfd_tunnel/t/configurations") => {
                r.config = body["config"].clone();
                if r.add_concurrent
                    || (r.add_after_restore
                        && r.writes.iter().filter(|s| s.starts_with("PUT")).count() == 2)
                {
                    r.add_concurrent = false;
                    r.add_after_restore = false;
                    r.config["ingress"].as_array_mut().unwrap().insert(
                        0,
                        json!({"hostname":"concurrent.example.com","service":"http://another"}),
                    );
                }
                if r.fail == Some("ingress") {
                    r.fail = None;
                    return ResponseTemplate::new(500).set_body_json(json!({"success":false}));
                }
                json!({})
            }
            ("DELETE", "/accounts/a/cfd_tunnel/t") => {
                r.tunnels.retain(|t| t["id"] != "t");
                json!({})
            }
            ("GET", "/zones/z/dns_records") => {
                let name = req
                    .url
                    .query_pairs()
                    .find(|(k, _)| k == "name")
                    .map(|(_, v)| v.to_string())
                    .unwrap();
                json!(
                    r.records
                        .iter()
                        .filter(|v| v["name"] == name)
                        .cloned()
                        .collect::<Vec<_>>()
                )
            }
            ("POST", "/zones/z/dns_records") => {
                if r.fail == Some("dns") {
                    return ResponseTemplate::new(403);
                }
                let mut record = body;
                record["id"] = json!("dns");
                r.records.push(record.clone());
                record
            }
            ("PUT", "/zones/z/dns_records/dns") => {
                if r.fail == Some("restore_dns_once") && body["content"] == "old.cfargotunnel.com" {
                    r.fail = None;
                    return ResponseTemplate::new(500);
                }
                let idx = r.records.iter().position(|v| v["id"] == "dns").unwrap();
                let mut body = body;
                body["id"] = json!("dns");
                r.records[idx] = body.clone();
                body
            }
            ("GET", "/zones/z/dns_records/dns") => {
                r.records.iter().find(|v| v["id"] == "dns").unwrap().clone()
            }
            ("DELETE", "/zones/z/dns_records/dns") => {
                if r.fail == Some("delete_dns_once") {
                    r.fail = None;
                    return ResponseTemplate::new(403);
                }
                r.records.retain(|v| v["id"] != "dns");
                json!({})
            }
            ("DELETE", p) if p.ends_with("/orphan-1") || p.ends_with("/orphan-2") => {
                let id = p.rsplit('/').next().unwrap();
                r.tunnels.retain(|t| t["id"] != id);
                json!({})
            }
            _ => panic!("Unexpected Cloudflare request: {method} {path}"),
        };
        let response =
            ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":result}));
        if method == "POST"
            && ((r.delayed_create == Some("tunnel") && path.ends_with("cfd_tunnel"))
                || (r.delayed_create == Some("dns") && path.ends_with("dns_records")))
        {
            response.set_delay(Duration::from_secs(1))
        } else {
            response
        }
    }
}
struct Runtime {
    fail: usize,
    started: bool,
    stop_failure: Mutex<bool>,
    events: Mutex<Vec<&'static str>>,
}
#[async_trait]
impl SiteRuntime for Runtime {
    async fn start_site(&self, _: &str) -> Result<bool, String> {
        self.events.lock().unwrap().push("start_site");
        if self.fail == 1 {
            Err("start failed".into())
        } else {
            Ok(self.started)
        }
    }
    async fn stop_site(&self, _: &str) -> Result<(), String> {
        self.events.lock().unwrap().push("stop_site");
        Ok(())
    }
    async fn site_responds(&self, _: u16) -> Result<bool, String> {
        Ok(self.fail != 2)
    }
    async fn start_tunnel(&self, _: &str) -> Result<bool, String> {
        self.events.lock().unwrap().push("start_tunnel");
        if self.fail == 5 {
            Err("connector failed".into())
        } else {
            Ok(self.started)
        }
    }
    async fn stop_tunnel(&self) -> Result<(), String> {
        self.events.lock().unwrap().push("stop_tunnel");
        let mut fail = self.stop_failure.lock().unwrap();
        if *fail {
            *fail = false;
            Err("stop failed once".into())
        } else {
            Ok(())
        }
    }
}
struct Fixture {
    deployer: Deployer,
    q: DeployRequest,
    remote: Arc<Mutex<Remote>>,
    runtime: Arc<Runtime>,
    _dir: tempfile::TempDir,
    server: MockServer,
}
async fn setup(fail: usize, reused: bool, started: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "ok").unwrap();
    let mut conn = Connection::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    conn.execute("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','s','x',1234,'now','now')",[]).unwrap();
    let db = Arc::new(Mutex::new(conn));
    let mut tunnels = vec![json!({"id":"unrelated","name":"unrelated"})];
    let mut entries = vec![json!({"service":"http_status:404"})];
    if reused {
        tunnels.push(json!({"id":"t","name":"hostify-s"}));
        entries.insert(0,json!({"hostname":"other.example.com","service":"http://elsewhere","originRequest":{"connectTimeout":30}}));
    }
    let remote = Arc::new(Mutex::new(Remote {
        tunnels,
        records: vec![
            json!({"id":"unrelated","name":"other.example.com","type":"A","content":"192.0.2.1"}),
        ],
        config: if reused {
            json!({"ingress":entries,"originRequest":{"noTLSVerify":false}})
        } else {
            Value::Null
        },
        fail: match fail {
            3 => Some("token"),
            4 => Some("dns"),
            _ => None,
        },
        ..Default::default()
    }));
    let server = MockServer::start().await;
    Mock::given(|_: &Request| true)
        .respond_with(Api {
            remote: remote.clone(),
            db: db.clone(),
        })
        .mount(&server)
        .await;
    let runtime = Arc::new(Runtime {
        fail,
        started,
        stop_failure: Mutex::new(false),
        events: Mutex::new(Vec::new()),
    });
    let deployer = Deployer {
        probe: Arc::new(Probe {
            class: None,
            wrong: false,
        }),
        db,
        cf: Arc::new(CloudflareClient::new(server.uri(), "token")),
        supervisor: runtime.clone(),
        secrets: Arc::new(MemoryStore::default()),
    };
    let q = DeployRequest {
        zone_name: Some("example.com".into()),
        site_id: "s".into(),
        hostname: "www.example.com".into(),
        account_id: "a".into(),
        zone_id: "z".into(),
        tunnel_name: "site".into(),
        port: 1234,
        root: dir.path().to_owned(),
        consent_dns: true,
    };
    Fixture {
        deployer,
        q,
        remote,
        runtime,
        _dir: dir,
        server,
    }
}
async fn failure(step: usize) {
    let mut f = setup(step, false, true).await;
    f.remote.lock().unwrap().config_reads_fail = true;
    if step == 0 {
        f.q.port = 0;
    }
    let result = f.deployer.test_publish(f.q.clone(), &|_| {}).await.unwrap();
    assert_eq!(result.state, "failed");
    assert_eq!(result.steps[step].state, "failed");
    let r = f.remote.lock().unwrap();
    assert_eq!(
        r.tunnels,
        vec![json!({"id":"unrelated","name":"unrelated"})]
    );
    assert_eq!(
        r.records,
        vec![json!({"id":"unrelated","name":"other.example.com","type":"A","content":"192.0.2.1"})]
    );
    let deletes: Vec<_> = r
        .writes
        .iter()
        .filter(|s| s.starts_with("DELETE"))
        .cloned()
        .collect();
    let mut expected = Vec::new();
    if step >= 5 {
        expected.push("DELETE /zones/z/dns_records/dns".to_owned());
    }
    if step >= 3 {
        expected.push("DELETE /accounts/a/cfd_tunnel/t".to_owned());
    }
    assert_eq!(deletes, expected);
    let events = f.runtime.events.lock().unwrap();
    assert_eq!(events.contains(&"stop_site"), step >= 2);
    assert_eq!(events.contains(&"stop_tunnel"), step >= 3);
}
#[tokio::test]
async fn failure_step_1_validation() {
    failure(0).await;
}
#[tokio::test]
async fn failure_step_2_start_local() {
    failure(1).await;
}
#[tokio::test]
async fn failure_step_3_verify_local() {
    failure(2).await;
}
#[tokio::test]
async fn failure_step_4_tunnel() {
    failure(3).await;
}
#[tokio::test]
async fn failure_step_5_dns() {
    failure(4).await;
}
#[tokio::test]
async fn failure_step_6_connector() {
    failure(5).await;
}

#[tokio::test]
async fn success_and_idempotent_republish() {
    let f = setup(99, false, true).await;
    let first = f.deployer.test_publish(f.q.clone(), &|_| {}).await.unwrap();
    assert_eq!(first.state, "published");
    assert!(first.steps.iter().all(|s| s.state == "done"));
    let writes = f.remote.lock().unwrap().writes.clone();
    let second = f.deployer.test_publish(f.q.clone(), &|_| {}).await.unwrap();
    assert_eq!(second.state, "published");
    let remote = f.remote.lock().unwrap();
    assert_eq!(
        remote
            .writes
            .iter()
            .filter(|s| s.starts_with("POST"))
            .count(),
        writes.iter().filter(|s| s.starts_with("POST")).count()
    );
    assert_eq!(remote.tunnels.len(), 2);
    assert_eq!(remote.records.len(), 2);
}
#[tokio::test]
async fn no_consent_zero_cloudflare_calls() {
    let mut f = setup(99, false, true).await;
    f.q.consent_dns = false;
    assert_eq!(
        f.deployer.test_publish(f.q, &|_| {}).await.unwrap().state,
        "failed"
    );
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn dns_conflict_has_no_writes() {
    let f = setup(99, false, true).await;
    f.remote
        .lock()
        .unwrap()
        .records
        .push(json!({"id":"foreign","name":f.q.hostname,"type":"A","content":"192.0.2.3"}));
    let before = f.remote.lock().unwrap().records.clone();
    assert_eq!(
        f.deployer.test_publish(f.q, &|_| {}).await.unwrap().state,
        "failed"
    );
    let r = f.remote.lock().unwrap();
    assert!(r.writes.is_empty());
    assert_eq!(r.records, before);
    assert!(f.runtime.events.lock().unwrap().is_empty());
}
#[tokio::test]
async fn reused_tunnel_restores_full_ingress_and_preserves_running_processes() {
    let f = setup(5, true, false).await;
    let previous = f.remote.lock().unwrap().config.clone();
    assert_eq!(
        f.deployer
            .test_publish(f.q.clone(), &|_| {})
            .await
            .unwrap()
            .state,
        "failed"
    );
    let r = f.remote.lock().unwrap();
    assert_eq!(r.config, previous);
    assert_eq!(r.tunnels.len(), 2);
    assert!(
        !r.writes
            .iter()
            .any(|s| s == "DELETE /accounts/a/cfd_tunnel/t")
    );
    let events = f.runtime.events.lock().unwrap();
    assert!(!events.contains(&"stop_site"));
    assert!(!events.contains(&"stop_tunnel"));
}
#[tokio::test]
async fn ambiguous_ingress_write_is_restored() {
    let f = setup(99, true, true).await;
    let previous = f.remote.lock().unwrap().config.clone();
    f.remote.lock().unwrap().fail = Some("ingress");
    assert_eq!(
        f.deployer.test_publish(f.q, &|_| {}).await.unwrap().state,
        "failed"
    );
    assert_eq!(f.remote.lock().unwrap().config, previous);
}
// Stop at the journal boundary immediately after a remote create, before the ID is saved.
async fn interrupt_after_create(dns: bool) {
    let f = setup(99, false, true).await;
    let cf = f.deployer.cf.clone();
    let q = f.q.clone();
    // Seed the exact durable intent and remote result left by death before ID save.
    let d = Deployment {
        hostname: None,
        verification: None,
        id: "interrupted".into(),
        site_id: "s".into(),
        state: "running".into(),
        steps: Vec::new(),
        error: None,
        started_at: "now".into(),
        finished_at: None,
    };
    let mut r = Resources {
        request: Some(q.clone()),
        owner: d.id.clone(),
        tunnel_intent: Some("site-hostify-interrupted".into()),
        ..Default::default()
    };
    f.deployer.save(&d, &r).unwrap();
    let id = cf
        .create_tunnel("a", "site-hostify-interrupted")
        .await
        .unwrap();
    if dns {
        r.tunnel_id = Some(id.clone());
        r.tunnel_created = true;
        r.dns_intent = true;
        f.deployer.save(&d, &r).unwrap();
        cf.create_owned_dns("z", &q.hostname, &id, &d.id)
            .await
            .unwrap();
    }
    // Real legacy databases stored both the row and JSON state as running.
    f.deployer.db.lock().unwrap().execute("UPDATE deployments SET state='running',snapshot_json=json_set(snapshot_json,'$.state','running') WHERE id='interrupted'",[]).unwrap();
    let recovered = f.deployer.recover_interrupted(&|_| {}).await.unwrap();
    assert_eq!(recovered[0].state, "rolled_back");
    assert!(
        f.deployer
            .recover_interrupted(&|_| {})
            .await
            .unwrap()
            .is_empty()
    );
    let remote = f.remote.lock().unwrap();
    assert_eq!(remote.tunnels.len(), 1);
    assert_eq!(remote.records.len(), 1);
}
#[tokio::test]
async fn recovers_tunnel_create_before_id_save() {
    interrupt_after_create(false).await;
}
#[tokio::test]
async fn recovers_dns_create_before_id_save() {
    interrupt_after_create(true).await;
}

#[tokio::test]
async fn concurrent_ingress_is_preserved_on_reused_tunnel() {
    let f = setup(5, true, true).await;
    f.remote.lock().unwrap().add_concurrent = true;
    assert_eq!(
        f.deployer.test_publish(f.q, &|_| {}).await.unwrap().state,
        "failed"
    );
    let r = f.remote.lock().unwrap();
    assert_eq!(r.tunnels.len(), 2);
    assert_eq!(
        r.config["ingress"],
        json!([{"hostname":"concurrent.example.com","service":"http://another"},{"hostname":"other.example.com","service":"http://elsewhere","originRequest":{"connectTimeout":30}},{"service":"http_status:404"}])
    );
    assert!(
        !r.writes
            .iter()
            .any(|s| s == "DELETE /accounts/a/cfd_tunnel/t")
    );
}
#[tokio::test]
async fn incomplete_failed_rollback_is_retried() {
    let f = setup(5, true, true).await;
    f.remote.lock().unwrap().fail = Some("delete_dns_once");
    let failed = f.deployer.test_publish(f.q.clone(), &|_| {}).await.unwrap();
    assert_eq!(failed.state, "failed");
    assert!(failed.error.unwrap().contains("rollback incomplete"));
    assert_eq!(f.remote.lock().unwrap().records.len(), 2);
    let writes = f.remote.lock().unwrap().writes.clone();
    let blocked = f.deployer.test_publish(f.q.clone(), &|_| {}).await.unwrap();
    assert!(blocked.error.unwrap().contains("rollback pending"));
    assert_eq!(f.remote.lock().unwrap().writes, writes);
    let recovered = f.deployer.recover_interrupted(&|_| {}).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].state, "rolled_back");
    assert_eq!(f.remote.lock().unwrap().records.len(), 1);
    assert!(
        f.deployer
            .recover_interrupted(&|_| {})
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn tagged_dns_for_different_tunnel_repoints_and_rollback_restores() {
    let f = setup(5, false, true).await;
    let prior = Resources {
        request: Some(DeployRequest {
            hostname: "prior.example.com".into(),
            ..f.q.clone()
        }),
        owner: "prior".into(),
        tunnel_created: true,
        tunnel_id: Some("different".into()),
        ..Default::default()
    };
    f.deployer.db.lock().unwrap().execute("INSERT INTO deployments(id,site_id,state,snapshot_json,created_resources_json,started_at) VALUES('prior','s','published','{}',?1,'before')",[serde_json::to_string(&prior).unwrap()]).unwrap();
    let old = json!({"id":"dns","name":f.q.hostname,"type":"CNAME","content":"different.cfargotunnel.com","proxied":false,"comment":"managed-by=hostify","tags":["hostify-deployment:old"]});
    f.remote.lock().unwrap().records.push(old.clone());
    assert_eq!(
        f.deployer.test_publish(f.q, &|_| {}).await.unwrap().state,
        "failed"
    );
    let r = f.remote.lock().unwrap();
    let mut expected = old;
    expected.as_object_mut().unwrap().remove("tags");
    assert_eq!(r.records[1], expected);
    assert_eq!(
        r.writes
            .iter()
            .filter(|s| s.starts_with("PUT /zones"))
            .count(),
        2
    );
}
#[tokio::test]
async fn recovery_does_not_delete_another_deployments_tagged_dns() {
    let f = setup(99, true, false).await;
    let d = Deployment {
        hostname: None,
        verification: None,
        id: "interrupted".into(),
        site_id: "s".into(),
        state: "running".into(),
        steps: Vec::new(),
        error: None,
        started_at: "now".into(),
        finished_at: None,
    };
    let r = Resources {
        request: Some(f.q.clone()),
        owner: d.id.clone(),
        dns_intent: true,
        dns_id: Some("other".into()),
        ..Default::default()
    };
    f.deployer.save(&d, &r).unwrap();
    f.remote.lock().unwrap().records.push(json!({"id":"other","name":f.q.hostname,"type":"CNAME","content":"t.cfargotunnel.com","comment":"managed-by=hostify","tags":["hostify-deployment:someone-else"]}));
    assert_eq!(
        f.deployer.recover_interrupted(&|_| {}).await.unwrap()[0].state,
        "rolled_back"
    );
    assert!(f.remote.lock().unwrap().writes.is_empty());
    assert_eq!(f.remote.lock().unwrap().records.len(), 2);
}

async fn cancel_after_remote_create(kind: &'static str) {
    let f = setup(99, false, true).await;
    f.remote.lock().unwrap().delayed_create = Some(kind);
    {
        let run = f.deployer.test_publish(f.q.clone(), &|_| {});
        tokio::pin!(run);
        tokio::time::timeout(Duration::from_secs(5),async {
            loop {
                tokio::select! {
                    result=&mut run=>panic!("deployment completed before cancellation: {:?}",result),
                    _=tokio::time::sleep(Duration::from_millis(1))=>{
                        let r=f.remote.lock().unwrap();
                        if (kind=="tunnel" && r.tunnels.len()==2) || (kind=="dns" && r.records.len()==2) {break;}
                    }
                }
            }
        }).await.expect("create was observed");
        // The server committed the POST but withheld the response, so dropping the
        // future really loses the ID, leaving only the pre-create SQLite intent.
    }
    let raw: String = f
        .deployer
        .db
        .lock()
        .unwrap()
        .query_row("SELECT created_resources_json FROM deployments", [], |r| {
            r.get(0)
        })
        .unwrap();
    let journal: Resources = serde_json::from_str(&raw).unwrap();
    if kind == "tunnel" {
        assert!(journal.tunnel_id.is_none());
        assert!(journal.tunnel_intent.is_some());
    } else {
        assert!(journal.dns_id.is_none());
        assert!(journal.dns_intent);
    }
    assert_eq!(
        f.deployer.recover_interrupted(&|_| {}).await.unwrap()[0].state,
        "rolled_back"
    );
    let r = f.remote.lock().unwrap();
    assert_eq!(r.tunnels.len(), 1);
    assert_eq!(r.records.len(), 1);
    assert!(f.runtime.events.lock().unwrap().contains(&"stop_site"));
}
#[tokio::test]
async fn cancelled_tunnel_post_reconciles_remote_result() {
    cancel_after_remote_create("tunnel").await;
}
#[tokio::test]
async fn cancelled_dns_post_reconciles_remote_result() {
    cancel_after_remote_create("dns").await;
}
#[tokio::test]
async fn reused_dns_is_never_deleted_on_failure() {
    let f = setup(5, true, false).await;
    f.remote.lock().unwrap().records.push(json!({"id":"other-deployment","name":f.q.hostname,"type":"CNAME","content":"t.cfargotunnel.com","comment":"managed-by=hostify","tags":["hostify-deployment:previous"]}));
    let before = f.remote.lock().unwrap().records.clone();
    assert_eq!(
        f.deployer.test_publish(f.q, &|_| {}).await.unwrap().state,
        "failed"
    );
    let r = f.remote.lock().unwrap();
    assert_eq!(r.records, before);
    assert!(!r.writes.iter().any(|s| s.starts_with("DELETE")));
}

#[tokio::test]
async fn republish_reverifies_instead_of_returning_cached_live() {
    let mut f = setup(99, false, true).await;
    assert_eq!(
        f.deployer
            .test_publish(f.q.clone(), &|_| {})
            .await
            .unwrap()
            .state,
        "published"
    );
    let before = f.remote.lock().unwrap().config.clone();
    f.deployer.supervisor = Arc::new(Runtime {
        fail: 5,
        started: false,
        stop_failure: Mutex::new(false),
        events: Mutex::new(Vec::new()),
    });
    let result = f.deployer.test_publish(f.q.clone(), &|_| {}).await.unwrap();
    assert_eq!(result.state, "failed");
    assert_eq!(result.steps[5].state, "failed");
    let r = f.remote.lock().unwrap();
    assert_eq!(r.tunnels.len(), 2);
    assert_eq!(r.records.len(), 2);
    assert_eq!(r.config, before);
    assert!(!r.writes.iter().any(|s| s.starts_with("DELETE")));
}

#[tokio::test]
async fn created_tunnel_cleanup_never_reads_configuration() {
    let f = setup(5, false, true).await;
    f.remote.lock().unwrap().fail = Some("config");
    assert_eq!(
        f.deployer.test_publish(f.q, &|_| {}).await.unwrap().state,
        "failed"
    );
    let r = f.remote.lock().unwrap();
    assert_eq!(r.tunnels.len(), 1);
    assert!(
        r.writes
            .iter()
            .any(|s| s == "DELETE /accounts/a/cfd_tunnel/t")
    );
    drop(r);
    assert!(
        !f.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.method == "GET" && r.url.path().ends_with("configurations"))
    );
}

#[tokio::test]
async fn fresh_null_empty_and_missing_config_publish_apex_to_live() {
    for config in [Value::Null, json!({}), json!("missing")] {
        let mut f = setup(99, false, true).await;
        f.q.hostname = "example.com".into();
        f.remote.lock().unwrap().config = config;
        let d = f.deployer.test_publish(f.q.clone(), &|_| {}).await.unwrap();
        assert_eq!(d.state, "published", "{:?}", d.error);
        let requests = f.server.received_requests().await.unwrap();
        assert!(
            !requests
                .iter()
                .any(|r| r.method == "GET" && r.url.path().ends_with("configurations"))
        );
        let put = requests.iter().find(|r| r.method == "PUT").unwrap();
        let body: Value = serde_json::from_slice(&put.body).unwrap();
        assert_eq!(
            body,
            json!({"config":{"ingress":[{"hostname":"example.com","service":"http://127.0.0.1:1234"},{"service":"http_status:404"}]}})
        );
        let post = requests
            .iter()
            .find(|r| r.method == "POST" && r.url.path().ends_with("cfd_tunnel"))
            .unwrap();
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body, json!({"name":"hostify-s","config_src":"cloudflare"}));
        assert_eq!(
            f.remote.lock().unwrap().records[1]["content"],
            "t.cfargotunnel.com"
        );
    }
}
#[tokio::test]
async fn reused_unconfigured_tunnel_restores_without_deleting() {
    for config in [Value::Null, json!({}), json!("missing")] {
        let f = setup(5, true, true).await;
        f.remote.lock().unwrap().config = config;
        let d = f.deployer.test_publish(f.q, &|_| {}).await.unwrap();
        assert_eq!(d.state, "failed");
        assert!(!d.error.unwrap().contains("rollback incomplete"));
        let r = f.remote.lock().unwrap();
        assert_eq!(r.tunnels.len(), 2);
        assert_eq!(r.config["ingress"], json!([{"service":"http_status:404"}]));
    }
}
#[tokio::test]
async fn recovers_two_failed_orphans_with_null_snapshot_idempotently() {
    let f = setup(99, false, false).await;
    for (owner, id) in [("failed-1", "orphan-1"), ("failed-2", "orphan-2")] {
        let d = Deployment {
            hostname: None,
            verification: None,
            id: owner.into(),
            site_id: "s".into(),
            state: "failed".into(),
            steps: Vec::new(),
            error: Some(
                "Cloudflare network error: Missing tunnel ingress; rollback incomplete".into(),
            ),
            started_at: "now".into(),
            finished_at: Some("now".into()),
        };
        // Persist the same resource fields as the real rows, with fake identifiers.
        let r = Resources {
            request: Some(f.q.clone()),
            owner: owner.into(),
            tunnel_created: true,
            tunnel_id: Some(id.into()),
            previous_config: None,
            dns_id: None,
            rollback_pending: true,
            ..Default::default()
        };
        f.deployer.save(&d, &r).unwrap();
        f.remote
            .lock()
            .unwrap()
            .tunnels
            .push(json!({"id":id,"name":owner}));
    }
    f.remote.lock().unwrap().config_reads_fail = true;
    let recovered = f.deployer.recover_interrupted(&|_| {}).await.unwrap();
    assert_eq!(recovered.len(), 2);
    assert!(recovered.iter().all(|d| d.state == "rolled_back"));
    assert!(
        f.deployer
            .recover_interrupted(&|_| {})
            .await
            .unwrap()
            .is_empty()
    );
    let r = f.remote.lock().unwrap();
    assert_eq!(
        r.writes,
        vec![
            "DELETE /accounts/a/cfd_tunnel/orphan-1",
            "DELETE /accounts/a/cfd_tunnel/orphan-2"
        ]
    );
    assert_eq!(r.tunnels.len(), 1);
    assert_eq!(r.records.len(), 1);
}
#[tokio::test]
async fn apex_conflicts_all_record_types_and_no_consent_repoint_have_zero_writes() {
    for kind in ["A", "AAAA", "CNAME", "TXT"] {
        let mut f = setup(99, false, true).await;
        f.q.hostname = "example.com".into();
        f.remote
            .lock()
            .unwrap()
            .records
            .push(json!({"id":"foreign","name":"example.com","type":kind,"content":"foreign"}));
        assert_eq!(
            f.deployer.test_publish(f.q, &|_| {}).await.unwrap().state,
            "failed"
        );
        assert!(f.remote.lock().unwrap().writes.is_empty());
    }
    let mut f = setup(99, false, true).await;
    f.q.consent_dns = false;
    f.remote.lock().unwrap().records.push(json!({"id":"dns","name":f.q.hostname,"type":"CNAME","content":"old.cfargotunnel.com","comment":"managed-by=hostify"}));
    assert_eq!(
        f.deployer.test_publish(f.q, &|_| {}).await.unwrap().state,
        "failed"
    );
    assert!(f.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn failed_dns_restore_retries_after_owned_tunnel_has_been_deleted() {
    let f = setup(5, false, true).await;
    let prior = Resources {
        request: Some(DeployRequest {
            hostname: "prior.example.com".into(),
            ..f.q.clone()
        }),
        owner: "prior".into(),
        tunnel_created: true,
        tunnel_id: Some("old".into()),
        ..Default::default()
    };
    f.deployer.db.lock().unwrap().execute("INSERT INTO deployments(id,site_id,state,snapshot_json,created_resources_json,started_at) VALUES('prior','s','published','{}',?1,'before')",[serde_json::to_string(&prior).unwrap()]).unwrap();
    let old = json!({"id":"dns","name":f.q.hostname,"type":"CNAME","content":"old.cfargotunnel.com","proxied":true,"comment":"managed-by=hostify"});
    {
        let mut remote = f.remote.lock().unwrap();
        remote.records.push(old.clone());
        remote.fail = Some("restore_dns_once");
    }
    let failed = f.deployer.test_publish(f.q, &|_| {}).await.unwrap();
    assert!(failed.error.unwrap().contains("rollback incomplete"));
    {
        let remote = f.remote.lock().unwrap();
        assert_eq!(remote.tunnels.len(), 1);
        assert_eq!(remote.records[1]["content"], "t.cfargotunnel.com");
    }
    assert_eq!(
        f.deployer.recover_interrupted(&|_| {}).await.unwrap()[0].state,
        "rolled_back"
    );
    assert_eq!(f.remote.lock().unwrap().records[1], old);
    assert!(
        f.deployer
            .recover_interrupted(&|_| {})
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn tagged_stale_dns_success_repoints_then_republish_has_no_dns_write() {
    let f = setup(99, false, true).await;
    let prior = Resources {
        request: Some(DeployRequest {
            hostname: "prior.example.com".into(),
            ..f.q.clone()
        }),
        owner: "prior".into(),
        tunnel_created: true,
        tunnel_id: Some("old".into()),
        ..Default::default()
    };
    f.deployer.db.lock().unwrap().execute("INSERT INTO deployments(id,site_id,state,snapshot_json,created_resources_json,started_at) VALUES('prior','s','published','{}',?1,'before')",[serde_json::to_string(&prior).unwrap()]).unwrap();
    f.remote.lock().unwrap().records.push(json!({"id":"dns","name":f.q.hostname,"type":"CNAME","content":"old.cfargotunnel.com","proxied":false,"comment":"managed-by=hostify","tags":["hostify-deployment:old"]}));
    assert_eq!(
        f.deployer
            .test_publish(f.q.clone(), &|_| {})
            .await
            .unwrap()
            .state,
        "published"
    );
    let writes = f.remote.lock().unwrap().writes.clone();
    assert_eq!(
        writes
            .iter()
            .filter(|s| s.starts_with("PUT /zones"))
            .count(),
        1
    );
    assert_eq!(
        f.deployer.test_publish(f.q, &|_| {}).await.unwrap().state,
        "published"
    );
    let remote = f.remote.lock().unwrap();
    assert_eq!(remote.records.len(), 2);
    assert_eq!(remote.records[1]["content"], "t.cfargotunnel.com");
    assert_eq!(remote.records[1]["proxied"], true);
    assert_eq!(
        remote
            .writes
            .iter()
            .filter(|s| s.starts_with("PUT /zones") || s.starts_with("POST"))
            .count(),
        writes
            .iter()
            .filter(|s| s.starts_with("PUT /zones") || s.starts_with("POST"))
            .count()
    );
}

#[tokio::test]
async fn failed_connector_stop_keeps_owned_tunnel_pending_until_retry() {
    let f = setup(5, false, true).await;
    *f.runtime.stop_failure.lock().unwrap() = true;
    let failed = f.deployer.test_publish(f.q, &|_| {}).await.unwrap();
    assert!(failed.error.unwrap().contains("rollback incomplete"));
    assert_eq!(f.remote.lock().unwrap().tunnels.len(), 2);
    assert!(
        !f.remote
            .lock()
            .unwrap()
            .writes
            .iter()
            .any(|w| w == "DELETE /accounts/a/cfd_tunnel/t")
    );
    assert_eq!(
        f.deployer.recover_interrupted(&|_| {}).await.unwrap()[0].state,
        "rolled_back"
    );
    assert_eq!(f.remote.lock().unwrap().tunnels.len(), 1);
    assert!(
        f.deployer
            .recover_interrupted(&|_| {})
            .await
            .unwrap()
            .is_empty()
    );
}

struct Probe {
    class: Option<VerifyClass>,
    wrong: bool,
}
#[async_trait]
impl PublicProbe for Probe {
    async fn zone(&self, _: &str) -> Result<crate::cloudflare::Zone, String> {
        if self.class == Some(VerifyClass::CloudflareApi) {
            return Err("API denied".into());
        }
        Ok(crate::cloudflare::Zone {
            id: "z".into(),
            name: "example.com".into(),
            status: if self.class == Some(VerifyClass::ZoneNotActive) {
                "pending"
            } else {
                "active"
            }
            .into(),
            name_servers: vec!["ns.example".into()],
        })
    }
    async fn record(&self, _: &str, _: &str) -> Result<Option<Value>, String> {
        if self.class == Some(VerifyClass::DnsRecordMissing) {
            return Ok(None);
        }
        Ok(Some(
            json!({"id":"dns","type":"CNAME","proxied":true,"content":"t.cfargotunnel.com"}),
        ))
    }
    async fn connector(&self, _: &str) -> Result<(bool, u64), String> {
        Ok((self.class != Some(VerifyClass::ConnectorDown), 4))
    }
    async fn authoritative(&self, _: &str, _: &str) -> Result<Vec<IpAddr>, String> {
        if self.class == Some(VerifyClass::DnsNotVisible) {
            return Ok(vec![]);
        }
        Ok(vec!["192.0.2.1".parse().unwrap()])
    }
    async fn recursive(&self, _: &str, endpoint: &str) -> Result<Vec<IpAddr>, String> {
        if self.class == Some(VerifyClass::DnsNotVisible) || endpoint.contains("google") {
            Err("negative cached answer".into())
        } else {
            Ok(vec!["192.0.2.1".parse().unwrap()])
        }
    }
    async fn system(&self, _: &str) -> Result<Vec<IpAddr>, String> {
        Err("negative cached answer".into())
    }
    async fn https(&self, _: &str, _: IpAddr, nonce: &str) -> Result<HttpEvidence, VerifyClass> {
        if let Some(class) = self.class {
            return Err(class);
        }
        Ok(HttpEvidence {
            marker_status: 200,
            marker_body: if self.wrong {
                "unrelated identical index page".into()
            } else {
                nonce.into()
            },
            root_status: 200,
        })
    }
}
impl Deployer {
    async fn test_publish(
        &self,
        q: DeployRequest,
        callback: &(dyn Fn(&Deployment) + Send + Sync),
    ) -> Result<Deployment, DeployError> {
        let d = self.deploy(q, callback).await?;
        if verification_state(&d.state) {
            self.verify_once(&d.id, callback).await
        } else {
            Ok(d)
        }
    }
}
#[tokio::test]
async fn negative_pc_and_google_cache_authoritative_and_pinned_https_publish() {
    let f = setup(99, false, true).await;
    let d = f.deployer.test_publish(f.q.clone(), &|_| {}).await.unwrap();
    assert_eq!(d.state, "published");
    let v = d.verification.unwrap();
    assert_eq!(v.visible_from_this_pc, Some(false));
    assert!(v.message.unwrap().contains("clears by itself"));
    let (_, r) = f.deployer.load(&d.id).unwrap();
    assert_eq!(r.nonce.as_ref().unwrap().len(), 32);
    assert!(!r.marker_directory.unwrap().join(r.nonce.unwrap()).exists());
}
#[tokio::test]
async fn expired_verification_retains_every_resource_and_performs_no_writes() {
    let mut f = setup(99, false, true).await;
    f.deployer.probe = Arc::new(Probe {
        class: Some(VerifyClass::TlsPending),
        wrong: false,
    });
    let d = f.deployer.deploy(f.q.clone(), &|_| {}).await.unwrap();
    let (mut d, r) = f.deployer.load(&d.id).unwrap();
    d.verification.as_mut().unwrap().deadline_at =
        (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
    f.deployer.save(&d, &r).unwrap();
    let before = f.remote.lock().unwrap().writes.clone();
    let events = f.runtime.events.lock().unwrap().clone();
    let result = f.deployer.verify_once(&d.id, &|_| {}).await.unwrap();
    assert_eq!(result.state, "recovery_required");
    assert_eq!(f.remote.lock().unwrap().writes, before);
    assert_eq!(*f.runtime.events.lock().unwrap(), events);
    assert!(!events.contains(&"stop_tunnel"));
    assert!(!events.contains(&"stop_site"));
    assert!(
        f.deployer
            .recover_interrupted(&|_| {})
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.remote.lock().unwrap().records.len(), 2);
}
#[tokio::test]
async fn unrelated_http_200_never_publishes_and_cancel_retains_resources() {
    let mut f = setup(99, false, true).await;
    f.deployer.probe = Arc::new(Probe {
        class: None,
        wrong: true,
    });
    let d = f.deployer.test_publish(f.q.clone(), &|_| {}).await.unwrap();
    assert_eq!(d.state, "verifying_public");
    assert_eq!(
        d.verification.unwrap().error_class,
        Some(VerifyClass::RoutingMismatch)
    );
    let before = f.remote.lock().unwrap().writes.clone();
    let events = f.runtime.events.lock().unwrap().clone();
    assert_eq!(
        f.deployer.cancel(&d.id, &|_| {}).await.unwrap().state,
        "recovery_required"
    );
    assert_eq!(f.remote.lock().unwrap().writes, before);
    assert_eq!(*f.runtime.events.lock().unwrap(), events);
}
#[tokio::test]
async fn waiting_dns_restart_preserves_resources_and_unpublish_owns_only_its_record() {
    let f = setup(99, false, true).await;
    let d = f.deployer.deploy(f.q.clone(), &|_| {}).await.unwrap();
    let (mut d, r) = f.deployer.load(&d.id).unwrap();
    d.state = "waiting_dns".into();
    f.deployer.save(&d, &r).unwrap();
    let before = f.remote.lock().unwrap().writes.clone();
    assert!(
        f.deployer
            .recover_interrupted(&|_| {})
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.remote.lock().unwrap().writes, before);
    f.deployer.reset_verification(&d.id, false).unwrap();
    assert_eq!(
        f.deployer.verify_once(&d.id, &|_| {}).await.unwrap().state,
        "published"
    );
    // Explicit unpublish is also supported directly from a waiting state.
    d.state = "waiting_dns".into();
    f.deployer.save(&d, &r).unwrap();
    f.deployer.unpublish(&d.id, &|_| {}).await.unwrap();
    assert_eq!(
        f.remote.lock().unwrap().records,
        vec![json!({"id":"unrelated","name":"other.example.com","type":"A","content":"192.0.2.1"})]
    );
    assert!(!r.marker_directory.unwrap().join(r.nonce.unwrap()).exists());
}

#[tokio::test]
async fn all_evidence_failures_have_specific_classes_states_and_ordered_checks() {
    let f = setup(99, false, true).await;
    let d = f.deployer.deploy(f.q.clone(), &|_| {}).await.unwrap();
    let v = d.verification.as_ref().unwrap();
    for class in [
        VerifyClass::ZoneNotActive,
        VerifyClass::DnsRecordMissing,
        VerifyClass::DnsNotVisible,
        VerifyClass::TlsPending,
        VerifyClass::ConnectorDown,
        VerifyClass::OriginUnreachable,
        VerifyClass::RoutingMismatch,
        VerifyClass::NetworkError,
        VerifyClass::CloudflareApi,
    ] {
        let probe = Probe {
            class: Some(class),
            wrong: false,
        };
        let evidence =
            crate::verify::attempt(&probe, "z", &f.q.hostname, Some("dns"), "t", "nonce", v).await;
        assert_eq!(evidence.error_class, Some(class));
        assert_eq!(evidence.message.as_deref(), Some(class.message()));
        assert_eq!(
            evidence
                .checks
                .iter()
                .map(|c| c.key.as_str())
                .collect::<Vec<_>>(),
            [
                "zone",
                "dns_record",
                "connector",
                "dns_public",
                "tls",
                "http",
                "content"
            ]
        );
    }
}
#[tokio::test]
async fn rewritten_root_body_is_irrelevant_when_nonce_and_root_status_are_correct() {
    let f = setup(99, false, true).await;
    std::fs::write(f.q.root.join("index.html"), "<html>original site</html>").unwrap();
    // The fake public origin provides HTTP 200 for / independently of its body.
    // Only the marker body is evidence of deployment identity.
    let d = f.deployer.test_publish(f.q.clone(), &|_| {}).await.unwrap();
    assert_eq!(d.state, "published");
    assert!(
        d.verification
            .unwrap()
            .checks
            .iter()
            .all(|c| c.state == "passed")
    );
}
#[tokio::test]
async fn attempts_do_not_repeat_transition_activity_and_snapshots_surface_pending_domain() {
    let mut f = setup(99, false, true).await;
    f.deployer.probe = Arc::new(Probe {
        class: Some(VerifyClass::DnsNotVisible),
        wrong: false,
    });
    let d = f.deployer.test_publish(f.q.clone(), &|_| {}).await.unwrap();
    let count = || {
        f.deployer.db.lock().unwrap().query_row("SELECT count(*) FROM activity_logs WHERE message='Waiting for DNS for www.example.com'",[],|r|r.get::<_,i64>(0)).unwrap()
    };
    assert_eq!(count(), 1);
    f.deployer.verify_once(&d.id, &|_| {}).await.unwrap();
    assert_eq!(count(), 1);
    let domain: (String, bool) = f
        .deployer
        .db
        .lock()
        .unwrap()
        .query_row("SELECT dns_status,https_verified FROM domains", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(domain, ("pending".into(), false));
}

#[tokio::test]
async fn resume_preserves_future_deadline_and_renews_an_expired_budget() {
    let f = setup(99, false, true).await;
    let d = f.deployer.deploy(f.q.clone(), &|_| {}).await.unwrap();
    let deadline = d.verification.as_ref().unwrap().deadline_at.clone();
    assert_eq!(
        f.deployer
            .reset_verification(&d.id, false)
            .unwrap()
            .verification
            .unwrap()
            .deadline_at,
        deadline
    );
    let (mut d, r) = f.deployer.load(&d.id).unwrap();
    d.verification.as_mut().unwrap().deadline_at =
        (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
    f.deployer.save(&d, &r).unwrap();
    let renewed = f
        .deployer
        .reset_verification(&d.id, false)
        .unwrap()
        .verification
        .unwrap();
    let remaining = chrono::DateTime::parse_from_rfc3339(&renewed.deadline_at)
        .unwrap()
        .with_timezone(&Utc)
        - Utc::now();
    assert!(remaining.num_seconds() >= 1798 && remaining.num_seconds() <= 1800);
}

#[tokio::test]
async fn malformed_recovery_row_does_not_block_other_rows() {
    let f = setup(99, false, false).await;
    f.deployer.db.lock().unwrap().execute("INSERT INTO deployments(id,site_id,state,snapshot_json,created_resources_json,started_at) VALUES('bad','s','validating','{}','{}','now')",[]).unwrap();
    let d = Deployment {
        hostname: Some(f.q.hostname.clone()),
        verification: None,
        id: "good".into(),
        site_id: "s".into(),
        state: "validating".into(),
        steps: vec![],
        error: None,
        started_at: "now".into(),
        finished_at: None,
    };
    f.deployer
        .save(
            &d,
            &Resources {
                request: Some(f.q.clone()),
                owner: d.id.clone(),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(f.deployer.recover_interrupted(&|_| {}).await.is_err());
    assert_eq!(f.deployer.load("good").unwrap().0.state, "rolled_back");
    assert_eq!(
        f.deployer
            .db
            .lock()
            .unwrap()
            .query_row("SELECT state FROM deployments WHERE id='bad'", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "validating"
    );
}

#[tokio::test]
async fn truncated_deployment_steps_return_error_without_panicking() {
    let f = setup(99, false, true).await;
    let d = f.deployer.deploy(f.q.clone(), &|_| {}).await.unwrap();
    let (mut d, r) = f.deployer.load(&d.id).unwrap();
    d.steps.clear();
    f.deployer.save(&d, &r).unwrap();
    assert!(
        f.deployer
            .verify_once(&d.id, &|_| {})
            .await
            .unwrap_err()
            .to_string()
            .contains("missing its verification step")
    );
    assert_eq!(f.deployer.load(&d.id).unwrap().0.state, "verifying_public");
}

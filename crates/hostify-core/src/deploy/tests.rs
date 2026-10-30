use super::*;
use crate::{db, secrets::MemoryStore};
use serde_json::{json, Value};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
struct Remote {
    tunnels: Vec<Value>, records: Vec<Value>, config: Value,
    writes: Vec<String>, delayed_create:Option<&'static str>, add_concurrent: bool, add_after_restore:bool, fail: Option<&'static str>,
}
#[derive(Clone)]
struct Api { remote: Arc<Mutex<Remote>>, db: Arc<Mutex<Connection>> }
impl Respond for Api {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut r=self.remote.lock().unwrap();
        let path=req.url.path(); let method=req.method.as_str();
        let body:Value=serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        if method!="GET" {
            r.writes.push(format!("{method} {path}"));
            // The actual Cloudflare POST is observed only after its intent is durable.
            if method=="POST" {
                let db=self.db.lock().unwrap();
                let raw:String=db.query_row("SELECT created_resources_json FROM deployments WHERE state='running' ORDER BY rowid DESC LIMIT 1",[],|r|r.get(0)).unwrap();
                let journal:Resources=serde_json::from_str(&raw).unwrap();
                if path.ends_with("cfd_tunnel") {assert_eq!(journal.tunnel_intent.as_deref(),body["name"].as_str());}
                else {assert!(journal.dns_intent); assert_eq!(body["tags"][0],format!("hostify-deployment:{}",journal.owner));}
            }
            if method=="PUT" {
                let raw:String=self.db.lock().unwrap().query_row("SELECT created_resources_json FROM deployments ORDER BY rowid DESC LIMIT 1",[],|r|r.get(0)).unwrap();
                let journal:Resources=serde_json::from_str(&raw).unwrap();
                assert!(journal.previous_config.is_some());
            }
        }
        let result=match (method,path) {
            ("GET","/accounts/a/cfd_tunnel")=>json!(r.tunnels),
            ("POST","/accounts/a/cfd_tunnel")=>{let t=json!({"id":"t","name":body["name"]});r.tunnels.push(t.clone());t},
            ("GET","/accounts/a/cfd_tunnel/t/token")=>{
                let raw:String=self.db.lock().unwrap().query_row("SELECT created_resources_json FROM deployments ORDER BY rowid DESC LIMIT 1",[],|r|r.get(0)).unwrap();
                let journal:Resources=serde_json::from_str(&raw).unwrap();assert_eq!(journal.tunnel_id.as_deref(),Some("t"));
                if r.fail==Some("token") {return ResponseTemplate::new(403);}
                json!("secret")
            },
            ("GET","/accounts/a/cfd_tunnel/t/configurations")=>json!({"config":r.config}),
            ("PUT","/accounts/a/cfd_tunnel/t/configurations")=>{
                r.config=body["config"].clone();
                if r.add_concurrent || (r.add_after_restore && r.writes.iter().filter(|s|s.starts_with("PUT")).count()==2) {
                    r.add_concurrent=false;r.add_after_restore=false;
                    r.config["ingress"].as_array_mut().unwrap().insert(0,json!({"hostname":"concurrent.example.com","service":"http://another"}));
                }
                if r.fail==Some("ingress") {r.fail=None; return ResponseTemplate::new(500).set_body_json(json!({"success":false}));}
                json!({})
            },
            ("DELETE","/accounts/a/cfd_tunnel/t")=>{r.tunnels.retain(|t|t["id"]!="t"); json!({})},
            ("GET","/zones/z/dns_records")=>{
                let name=req.url.query_pairs().find(|(k,_)|k=="name").map(|(_,v)|v.to_string()).unwrap();
                json!(r.records.iter().filter(|v|v["name"]==name).cloned().collect::<Vec<_>>())
            },
            ("POST","/zones/z/dns_records")=>{
                if r.fail==Some("dns") {return ResponseTemplate::new(403);}
                let mut record=body;record["id"]=json!("dns");r.records.push(record.clone());record
            },
            ("GET","/zones/z/dns_records/dns")=>r.records.iter().find(|v|v["id"]=="dns").unwrap().clone(),
            ("DELETE","/zones/z/dns_records/dns")=>{if r.fail==Some("delete_dns_once") {r.fail=None;return ResponseTemplate::new(403);} r.records.retain(|v|v["id"]!="dns");json!({})},
            _=>panic!("Unexpected Cloudflare request: {method} {path}"),
        };
        let response=ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":result}));
        if method=="POST" && ((r.delayed_create==Some("tunnel") && path.ends_with("cfd_tunnel")) || (r.delayed_create==Some("dns") && path.ends_with("dns_records"))) {
            response.set_delay(Duration::from_secs(1))
        } else {response}
    }
}
struct Runtime { fail:usize, started:bool, events:Mutex<Vec<&'static str>> }
#[async_trait]
impl SiteRuntime for Runtime {
    async fn start_site(&self,_:&str)->Result<bool,String>{self.events.lock().unwrap().push("start_site");if self.fail==1 {Err("start failed".into())} else {Ok(self.started)}}
    async fn stop_site(&self,_:&str)->Result<(),String>{self.events.lock().unwrap().push("stop_site");Ok(())}
    async fn site_responds(&self,_:u16)->Result<bool,String>{Ok(self.fail!=2)}
    async fn start_tunnel(&self,_:&str)->Result<bool,String>{self.events.lock().unwrap().push("start_tunnel");if self.fail==5 {Err("connector failed".into())}else{Ok(self.started)}}
    async fn stop_tunnel(&self)->Result<(),String>{self.events.lock().unwrap().push("stop_tunnel");Ok(())}
    async fn verify_public(&self,_:&str,_:&std::path::Path)->Result<(),String>{if self.fail==6{Err("DNS has not propagated yet".into())}else{Ok(())}}
}
struct Fixture { deployer:Deployer, q:DeployRequest, remote:Arc<Mutex<Remote>>, runtime:Arc<Runtime>, _dir:tempfile::TempDir, server:MockServer }
async fn setup(fail:usize, reused:bool, started:bool)->Fixture {
    let dir=tempfile::tempdir().unwrap();std::fs::write(dir.path().join("index.html"),"ok").unwrap();
    let mut conn=Connection::open_in_memory().unwrap(); db::migrate(&mut conn).unwrap();
    conn.execute("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','s','x',1234,'now','now')",[]).unwrap();
    let db=Arc::new(Mutex::new(conn));
    let mut tunnels=vec![json!({"id":"unrelated","name":"unrelated"})];
    let mut entries=vec![json!({"service":"http_status:404"})];
    if reused {tunnels.push(json!({"id":"t","name":"site"}));entries.insert(0,json!({"hostname":"other.example.com","service":"http://elsewhere","originRequest":{"connectTimeout":30}}));}
    let remote=Arc::new(Mutex::new(Remote{tunnels,records:vec![json!({"id":"unrelated","name":"other.example.com","type":"A","content":"192.0.2.1"})],config:json!({"ingress":entries,"originRequest":{"noTLSVerify":false}}),fail:match fail{3=>Some("token"),4=>Some("dns"),_=>None},..Default::default()}));
    let server=MockServer::start().await;
    Mock::given(|_:&Request|true).respond_with(Api{remote:remote.clone(),db:db.clone()}).mount(&server).await;
    let runtime=Arc::new(Runtime{fail,started,events:Mutex::new(Vec::new())});
    let deployer=Deployer{db,cf:Arc::new(CloudflareClient::new(server.uri(),"token")),supervisor:runtime.clone(),secrets:Arc::new(MemoryStore::default())};
    let q=DeployRequest{site_id:"s".into(),hostname:"www.example.com".into(),account_id:"a".into(),zone_id:"z".into(),tunnel_name:"site".into(),port:1234,root:dir.path().to_owned(),consent_dns:true};
    Fixture{deployer,q,remote,runtime,_dir:dir,server}
}
async fn failure(step:usize) {
    let mut f=setup(step,false,true).await;
    if step==0 {f.q.port=0;}
    let result=f.deployer.deploy(f.q.clone(),&|_|{}).await.unwrap();
    assert_eq!(result.state,"failed"); assert_eq!(result.steps[step].state,"failed");
    let r=f.remote.lock().unwrap();
    assert_eq!(r.tunnels,vec![json!({"id":"unrelated","name":"unrelated"})]);
    assert_eq!(r.records,vec![json!({"id":"unrelated","name":"other.example.com","type":"A","content":"192.0.2.1"})]);
    let deletes:Vec<_>=r.writes.iter().filter(|s|s.starts_with("DELETE")).cloned().collect();
    let mut expected=Vec::new();
    if step>=5 {expected.push("DELETE /zones/z/dns_records/dns".to_owned());}
    if step>=3 {expected.push("DELETE /accounts/a/cfd_tunnel/t".to_owned());}
    assert_eq!(deletes,expected);
    let events=f.runtime.events.lock().unwrap();
    assert_eq!(events.contains(&"stop_site"),step>=2);
    assert_eq!(events.contains(&"stop_tunnel"),step==6);
    if step==6 {assert_eq!(result.error.as_deref(),Some("DNS has not propagated yet"));}
}
#[tokio::test] async fn failure_step_1_validation(){failure(0).await;}
#[tokio::test] async fn failure_step_2_start_local(){failure(1).await;}
#[tokio::test] async fn failure_step_3_verify_local(){failure(2).await;}
#[tokio::test] async fn failure_step_4_tunnel(){failure(3).await;}
#[tokio::test] async fn failure_step_5_dns(){failure(4).await;}
#[tokio::test] async fn failure_step_6_connector(){failure(5).await;}
#[tokio::test] async fn failure_step_7_propagation_never_live(){failure(6).await;}
#[tokio::test] async fn success_and_idempotent_republish() {
    let f=setup(99,false,true).await;
    let first=f.deployer.deploy(f.q.clone(),&|_|{}).await.unwrap();assert_eq!(first.state,"live");assert!(first.steps.iter().all(|s|s.state=="done"));
    let writes=f.remote.lock().unwrap().writes.clone();
    let second=f.deployer.deploy(f.q.clone(),&|_|{}).await.unwrap(); assert_eq!(second.state,"live");
    let remote=f.remote.lock().unwrap();
    assert_eq!(remote.writes.iter().filter(|s|s.starts_with("POST")).count(),writes.iter().filter(|s|s.starts_with("POST")).count());
    assert_eq!(remote.tunnels.len(),2);assert_eq!(remote.records.len(),2);
}
#[tokio::test] async fn no_consent_zero_cloudflare_calls() {
    let mut f=setup(99,false,true).await;f.q.consent_dns=false;
    assert_eq!(f.deployer.deploy(f.q,&|_|{}).await.unwrap().state,"failed");
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test] async fn dns_conflict_has_no_writes() {
    let f=setup(99,false,true).await;
    f.remote.lock().unwrap().records.push(json!({"id":"foreign","name":f.q.hostname,"type":"A","content":"192.0.2.3"}));
    let before=f.remote.lock().unwrap().records.clone();
    assert_eq!(f.deployer.deploy(f.q,&|_|{}).await.unwrap().state,"failed");
    let r=f.remote.lock().unwrap();assert!(r.writes.is_empty());assert_eq!(r.records,before);assert!(f.runtime.events.lock().unwrap().is_empty());
}
#[tokio::test] async fn reused_tunnel_restores_full_ingress_and_preserves_running_processes() {
    let f=setup(6,true,false).await;let previous=f.remote.lock().unwrap().config.clone();
    assert_eq!(f.deployer.deploy(f.q.clone(),&|_|{}).await.unwrap().state,"failed");
    let r=f.remote.lock().unwrap();assert_eq!(r.config,previous);assert_eq!(r.tunnels.len(),2);assert!(!r.writes.iter().any(|s|s=="DELETE /accounts/a/cfd_tunnel/t"));
    let events=f.runtime.events.lock().unwrap();assert!(!events.contains(&"stop_site"));assert!(!events.contains(&"stop_tunnel"));
}
#[tokio::test] async fn ambiguous_ingress_write_is_restored() {
    let f=setup(99,true,true).await;let previous=f.remote.lock().unwrap().config.clone();f.remote.lock().unwrap().fail=Some("ingress");
    assert_eq!(f.deployer.deploy(f.q,&|_|{}).await.unwrap().state,"failed");assert_eq!(f.remote.lock().unwrap().config,previous);
}
// Stop at the journal boundary immediately after a remote create, before the ID is saved.
async fn interrupt_after_create(dns:bool) {
    let f=setup(99,false,true).await;
    let cf=f.deployer.cf.clone(); let q=f.q.clone();
    // Seed the exact durable intent and remote result left by death before ID save.
    let d=Deployment{id:"interrupted".into(),site_id:"s".into(),state:"running".into(),steps:Vec::new(),error:None,started_at:"now".into(),finished_at:None};
    let mut r=Resources{request:Some(q.clone()),owner:d.id.clone(),tunnel_intent:Some("site-hostify-interrupted".into()),..Default::default()};
    f.deployer.save(&d,&r).unwrap();
    let id=cf.create_tunnel("a","site-hostify-interrupted").await.unwrap();
    if dns {
        r.tunnel_id=Some(id.clone());r.tunnel_created=true;r.dns_intent=true;f.deployer.save(&d,&r).unwrap();
        cf.create_owned_dns("z",&q.hostname,&id,&d.id).await.unwrap();
    }
    let recovered=f.deployer.recover_interrupted(&|_|{}).await.unwrap();assert_eq!(recovered[0].state,"rolled_back");
    assert!(f.deployer.recover_interrupted(&|_|{}).await.unwrap().is_empty());
    let remote=f.remote.lock().unwrap();assert_eq!(remote.tunnels.len(),1);assert_eq!(remote.records.len(),1);
}
#[tokio::test] async fn recovers_tunnel_create_before_id_save(){interrupt_after_create(false).await;}
#[tokio::test] async fn recovers_dns_create_before_id_save(){interrupt_after_create(true).await;}

#[tokio::test] async fn concurrent_ingress_is_preserved_and_created_tunnel_retained() {
    let f=setup(6,false,true).await;f.remote.lock().unwrap().add_concurrent=true;
    assert_eq!(f.deployer.deploy(f.q,&|_|{}).await.unwrap().state,"failed");
    let r=f.remote.lock().unwrap();assert_eq!(r.tunnels.len(),2);
    assert_eq!(r.config["ingress"],json!([{"hostname":"concurrent.example.com","service":"http://another"},{"service":"http_status:404"}]));
    assert!(!r.writes.iter().any(|s|s=="DELETE /accounts/a/cfd_tunnel/t"));
}
#[tokio::test] async fn incomplete_failed_rollback_is_retried() {
    let f=setup(6,true,true).await;f.remote.lock().unwrap().fail=Some("delete_dns_once");
    let failed=f.deployer.deploy(f.q,&|_|{}).await.unwrap();assert_eq!(failed.state,"failed");assert!(failed.error.unwrap().contains("rollback incomplete"));
    assert_eq!(f.remote.lock().unwrap().records.len(),2);
    let recovered=f.deployer.recover_interrupted(&|_|{}).await.unwrap();assert_eq!(recovered.len(),1);assert_eq!(recovered[0].state,"rolled_back");assert_eq!(f.remote.lock().unwrap().records.len(),1);
    assert!(f.deployer.recover_interrupted(&|_|{}).await.unwrap().is_empty());
}
#[tokio::test] async fn tagged_dns_for_different_tunnel_conflicts_before_writes() {
    let f=setup(99,false,true).await;
    f.remote.lock().unwrap().records.push(json!({"id":"old-hostify","name":f.q.hostname,"type":"CNAME","content":"different.cfargotunnel.com","comment":"managed-by=hostify"}));
    assert_eq!(f.deployer.deploy(f.q,&|_|{}).await.unwrap().state,"failed");assert!(f.remote.lock().unwrap().writes.is_empty());
}
#[tokio::test] async fn recovery_does_not_delete_another_deployments_tagged_dns() {
    let f=setup(99,true,false).await;
    let d=Deployment{id:"interrupted".into(),site_id:"s".into(),state:"running".into(),steps:Vec::new(),error:None,started_at:"now".into(),finished_at:None};
    let r=Resources{request:Some(f.q.clone()),owner:d.id.clone(),dns_intent:true,dns_id:Some("other".into()),..Default::default()};
    f.deployer.save(&d,&r).unwrap();
    f.remote.lock().unwrap().records.push(json!({"id":"other","name":f.q.hostname,"type":"CNAME","content":"t.cfargotunnel.com","comment":"managed-by=hostify","tags":["hostify-deployment:someone-else"]}));
    assert_eq!(f.deployer.recover_interrupted(&|_|{}).await.unwrap()[0].state,"rolled_back");assert!(f.remote.lock().unwrap().writes.is_empty());assert_eq!(f.remote.lock().unwrap().records.len(),2);
}

async fn cancel_after_remote_create(kind:&'static str) {
    let f=setup(99,false,true).await;f.remote.lock().unwrap().delayed_create=Some(kind);
    {
        let run=f.deployer.deploy(f.q.clone(),&|_|{});tokio::pin!(run);
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
    let raw:String=f.deployer.db.lock().unwrap().query_row("SELECT created_resources_json FROM deployments",[],|r|r.get(0)).unwrap();
    let journal:Resources=serde_json::from_str(&raw).unwrap();
    if kind=="tunnel" {assert!(journal.tunnel_id.is_none());assert!(journal.tunnel_intent.is_some());}
    else {assert!(journal.dns_id.is_none());assert!(journal.dns_intent);}
    assert_eq!(f.deployer.recover_interrupted(&|_|{}).await.unwrap()[0].state,"rolled_back");
    let r=f.remote.lock().unwrap();assert_eq!(r.tunnels.len(),1);assert_eq!(r.records.len(),1);
    assert!(f.runtime.events.lock().unwrap().contains(&"stop_site"));
}
#[tokio::test] async fn cancelled_tunnel_post_reconciles_remote_result(){cancel_after_remote_create("tunnel").await;}
#[tokio::test] async fn cancelled_dns_post_reconciles_remote_result(){cancel_after_remote_create("dns").await;}
#[tokio::test] async fn reused_dns_is_never_deleted_on_failure() {
    let f=setup(6,true,false).await;
    f.remote.lock().unwrap().records.push(json!({"id":"other-deployment","name":f.q.hostname,"type":"CNAME","content":"t.cfargotunnel.com","comment":"managed-by=hostify","tags":["hostify-deployment:previous"]}));
    let before=f.remote.lock().unwrap().records.clone();
    assert_eq!(f.deployer.deploy(f.q,&|_|{}).await.unwrap().state,"failed");
    let r=f.remote.lock().unwrap();assert_eq!(r.records,before);assert!(!r.writes.iter().any(|s|s.starts_with("DELETE")));
}

#[tokio::test] async fn republish_reverifies_instead_of_returning_cached_live() {
    let mut f=setup(99,false,true).await;
    assert_eq!(f.deployer.deploy(f.q.clone(),&|_|{}).await.unwrap().state,"live");
    let before=f.remote.lock().unwrap().config.clone();
    f.deployer.supervisor=Arc::new(Runtime{fail:6,started:false,events:Mutex::new(Vec::new())});
    let result=f.deployer.deploy(f.q.clone(),&|_|{}).await.unwrap();assert_eq!(result.state,"failed");assert_eq!(result.steps[6].state,"failed");
    let r=f.remote.lock().unwrap();assert_eq!(r.tunnels.len(),2);assert_eq!(r.records.len(),2);assert_eq!(r.config,before);assert!(!r.writes.iter().any(|s|s.starts_with("DELETE")));
}

#[tokio::test] async fn tunnel_deletion_rechecks_ingress_after_restoration() {
    let f=setup(6,false,true).await;f.remote.lock().unwrap().add_after_restore=true;
    assert_eq!(f.deployer.deploy(f.q,&|_|{}).await.unwrap().state,"failed");
    let r=f.remote.lock().unwrap();assert_eq!(r.tunnels.len(),2);assert!(r.config["ingress"].as_array().unwrap().iter().any(|e|e["hostname"]=="concurrent.example.com"));
    assert!(!r.writes.iter().any(|s|s=="DELETE /accounts/a/cfd_tunnel/t"));
}

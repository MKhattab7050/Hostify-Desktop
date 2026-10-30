use crate::{cloudflare::{CloudflareClient, CfError, DnsPlan, Tunnel}, secrets::{SecretError, SecretStore}};
use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::{Arc, Mutex}, time::Duration};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeployError { #[error("{0}")] Message(String), #[error(transparent)] Cloudflare(#[from] CfError), #[error(transparent)] Secret(#[from] SecretError), #[error(transparent)] Db(#[from] rusqlite::Error), #[error(transparent)] Json(#[from] serde_json::Error), #[error(transparent)] Io(#[from] std::io::Error) }
#[async_trait]
pub trait SiteRuntime: Send + Sync {
    /// Returns true only when this call started a previously stopped site.
    async fn start_site(&self, site_id: &str) -> Result<bool, String>;
    async fn stop_site(&self, site_id: &str) -> Result<(), String>;
    /// Returns true only when this call started a new connector.
    async fn start_tunnel(&self, token: &str) -> Result<bool, String>;
    async fn stop_tunnel(&self) -> Result<(), String>;
    async fn site_responds(&self, port: u16) -> Result<bool, String>;
}
#[async_trait]
pub trait Cloudflare: Send + Sync {
    async fn tunnel(&self, account: &str, name: &str) -> Result<Tunnel,CfError>;
    async fn ingress(&self, account: &str, id: &str, entries: &[(String,String)]) -> Result<(),CfError>;
    async fn plan(&self, zone: &str, hostname: &str, tunnel: &str) -> Result<DnsPlan,CfError>;
    async fn create_dns(&self, zone: &str, hostname: &str, tunnel: &str) -> Result<String,CfError>;
    async fn delete_dns(&self, zone: &str, id: &str) -> Result<bool,CfError>;
    async fn delete_tunnel(&self, account: &str, id: &str) -> Result<(),CfError>;
}
#[async_trait]
impl Cloudflare for CloudflareClient {
    async fn tunnel(&self,a:&str,n:&str)->Result<Tunnel,CfError>{ self.get_or_create_tunnel(a,n).await }
    async fn ingress(&self,a:&str,i:&str,e:&[(String,String)])->Result<(),CfError>{ self.put_tunnel_ingress(a,i,e).await }
    async fn plan(&self,z:&str,h:&str,t:&str)->Result<DnsPlan,CfError>{ self.plan_dns(z,h,t).await }
    async fn create_dns(&self,z:&str,h:&str,t:&str)->Result<String,CfError>{ self.create_dns(z,h,t).await }
    async fn delete_dns(&self,z:&str,i:&str)->Result<bool,CfError>{ self.delete_dns_if_ours(z,i).await }
    async fn delete_tunnel(&self,a:&str,i:&str)->Result<(),CfError>{ self.delete_tunnel(a,i).await }
}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct DeploymentStep { pub key:String, pub label:String, pub state:String, #[serde(skip_serializing_if="Option::is_none")] pub detail:Option<String> }
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Deployment { pub id:String, pub site_id:String, pub state:String, pub steps:Vec<DeploymentStep>, #[serde(skip_serializing_if="Option::is_none")] pub error:Option<String>, pub started_at:String, #[serde(skip_serializing_if="Option::is_none")] pub finished_at:Option<String> }
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct DeployRequest { pub site_id:String, pub hostname:String, pub account_id:String, pub zone_id:String, pub tunnel_name:String, pub port:u16, pub root:PathBuf, pub consent_dns:bool }
#[derive(Default,Serialize,Deserialize)]
struct Resources { request: Option<DeployRequest>, site_started:bool, connector_started:bool, tunnel_id:Option<String>, tunnel_created:bool, dns_id:Option<String> }
pub struct Deployer { pub db:Arc<Mutex<Connection>>, pub cf:Arc<dyn Cloudflare>, pub secrets:Arc<dyn SecretStore>, pub supervisor:Arc<dyn SiteRuntime> }
const LABELS: [&str;7]=["Validate website","Start local server","Verify local site","Create tunnel","Add DNS record","Start connector","Verify HTTPS"];
impl Deployer {
    fn save(&self,d:&Deployment,r:&Resources)->Result<(),DeployError> { let db=self.db.lock().map_err(|_| DeployError::Message("Database lock poisoned".into()))?; db.execute("INSERT INTO deployments (id,site_id,state,snapshot_json,created_resources_json,started_at,finished_at) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET state=excluded.state,snapshot_json=excluded.snapshot_json,created_resources_json=excluded.created_resources_json,finished_at=excluded.finished_at",params![d.id,d.site_id,d.state,serde_json::to_string(d)?,serde_json::to_string(r)?,d.started_at,d.finished_at])?; Ok(()) }
    fn emit(&self,d:&Deployment,r:&Resources,callback:&(dyn Fn(&Deployment)+Send+Sync))->Result<(),DeployError> { self.save(d,r)?; callback(d); Ok(()) }
    pub async fn deploy(&self, request:DeployRequest, callback: &(dyn Fn(&Deployment)+Send+Sync)) -> Result<Deployment,DeployError> {
        let mut d=Deployment { id:uuid::Uuid::new_v4().to_string(),site_id:request.site_id.clone(),state:"running".into(),steps:LABELS.iter().enumerate().map(|(i,s)| DeploymentStep{key:["validate_website","start_local_server","verify_local_site","create_tunnel","add_dns_record","start_connector","verify_https"][i].into(),label:(*s).into(),state:"pending".into(),detail:None}).collect(),error:None,started_at:Utc::now().to_rfc3339(),finished_at:None };
        let mut r=Resources{request:Some(request.clone()),..Default::default()}; self.emit(&d,&r,callback)?;
        for i in 0..LABELS.len() { d.steps[i].state="running".into(); self.emit(&d,&r,callback)?;
            let result=self.step(i,&request,&mut r,&d,callback).await;
            if let Err(e)=result { d.steps[i].state="failed".into(); d.steps[i].detail=Some(e.to_string()); d.error=Some(e.to_string()); self.save(&d,&r)?; let rollback=self.rollback(&request,&mut r).await; if let Err(err)=rollback { d.error=Some(format!("{e}; rollback incomplete: {err}")); } d.state="failed".into(); d.finished_at=Some(Utc::now().to_rfc3339()); self.emit(&d,&r,callback)?; return Ok(d); }
            d.steps[i].state="done".into(); self.emit(&d,&r,callback)?;
        } d.state="live".into(); d.finished_at=Some(Utc::now().to_rfc3339()); self.emit(&d,&r,callback)?; Ok(d)
    }
    async fn step(&self,i:usize,q:&DeployRequest,r:&mut Resources,d:&Deployment,callback:&(dyn Fn(&Deployment)+Send+Sync))->Result<(),DeployError> {
        match i {
            0=>{ if !q.consent_dns { return Err(DeployError::Message("DNS consent is required".into())); } if !q.root.join("index.html").is_file() || q.hostname.is_empty() || q.port==0 { return Err(DeployError::Message("Invalid website or hostname".into())); } },
            1=>{ r.site_started=self.supervisor.start_site(&q.site_id).await.map_err(DeployError::Message)?; self.emit(d,r,callback)?; },
            2=>{ if !self.supervisor.site_responds(q.port).await.map_err(DeployError::Message)? { return Err(DeployError::Message("Local site did not respond".into())); } },
            3=>{ let t=self.cf.tunnel(&q.account_id,&q.tunnel_name).await?; r.tunnel_id=Some(t.id.clone()); r.tunnel_created=t.created; self.emit(d,r,callback)?; self.secrets.set(&format!("tunnel:{}",t.id),&t.token)?; self.cf.ingress(&q.account_id,&t.id,&[(q.hostname.clone(),format!("http://127.0.0.1:{}",q.port))]).await?; },
            4=>{ let tunnel=r.tunnel_id.as_deref().ok_or_else(|| DeployError::Message("Missing tunnel".into()))?; let plan=self.cf.plan(&q.zone_id,&q.hostname,tunnel).await?; match plan.action.as_str() { "create"=>r.dns_id=Some(self.cf.create_dns(&q.zone_id,&q.hostname,tunnel).await?), "update_own"=>(), _=>return Err(CfError::Conflict.into()) } self.emit(d,r,callback)?; },
            5=>{ let tunnel=r.tunnel_id.as_deref().ok_or_else(|| DeployError::Message("Missing tunnel".into()))?; let token=self.secrets.get(&format!("tunnel:{tunnel}"))?.ok_or_else(|| DeployError::Message("Missing tunnel credential".into()))?; r.connector_started=self.supervisor.start_tunnel(&token).await.map_err(DeployError::Message)?; self.emit(d,r,callback)?; },
            6=>{ let expected=Sha256::digest(std::fs::read(q.root.join("index.html"))?); let client=reqwest::Client::builder().timeout(Duration::from_secs(10)).build().map_err(|e| DeployError::Message(e.to_string()))?; let deadline=tokio::time::Instant::now()+Duration::from_secs(180); loop { if let Ok(resp)=client.get(format!("https://{}/",q.hostname)).send().await { if resp.status().is_success() { if let Ok(body)=resp.bytes().await { if Sha256::digest(&body)==expected { break; } } } } if tokio::time::Instant::now()>=deadline { return Err(DeployError::Message("DNS has not propagated yet".into())); } tokio::time::sleep(Duration::from_secs(5)).await; } },
            _=>return Err(DeployError::Message("Invalid step".into()))
        } Ok(())
    }
    async fn rollback(&self,q:&DeployRequest,r:&mut Resources)->Result<(),DeployError> { let mut errors=Vec::new(); if r.connector_started { match self.supervisor.stop_tunnel().await { Ok(())=>r.connector_started=false,Err(e)=>errors.push(e) } } if let Some(id)=r.dns_id.clone() { match self.cf.delete_dns(&q.zone_id,&id).await { Ok(_)=>r.dns_id=None,Err(e)=>errors.push(e.to_string()) } } if r.tunnel_created { errors.push("Tunnel cleanup requires verifying that the tunnel is unused".into()); } if r.site_started { match self.supervisor.stop_site(&q.site_id).await { Ok(())=>r.site_started=false,Err(e)=>errors.push(e) } } if errors.is_empty(){Ok(())}else{Err(DeployError::Message(errors.join("; ")))} }
    pub async fn recover_interrupted(&self,callback:&(dyn Fn(&Deployment)+Send+Sync))->Result<Vec<Deployment>,DeployError> { let records={ let db=self.db.lock().map_err(|_| DeployError::Message("Database lock poisoned".into()))?; let mut stmt=db.prepare("SELECT snapshot_json,created_resources_json FROM deployments WHERE state='running'")?; let rows=stmt.query_map([],|row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?; rows.collect::<Result<Vec<_>,_>>()? }; let mut recovered=Vec::new(); for (snapshot,resources) in records { let mut d:Deployment=serde_json::from_str(&snapshot)?; let mut r:Resources=serde_json::from_str(&resources)?; let q=r.request.clone().ok_or_else(|| DeployError::Message("Missing recovery data".into()))?; self.rollback(&q,&mut r).await?; d.state="rolled_back".into(); d.error=Some("Deployment interrupted and rolled back".into()); d.finished_at=Some(Utc::now().to_rfc3339()); self.emit(&d,&r,callback)?; recovered.push(d); } Ok(recovered) }
}
#[cfg(test)] mod tests {
 use super::*; use crate::{db, secrets::MemoryStore};
 struct Fake;
 #[async_trait] impl SiteRuntime for Fake {
  async fn start_site(&self,_:&str)->Result<bool,String>{Err("start failed".into())} async fn stop_site(&self,_:&str)->Result<(),String>{Ok(())} async fn start_tunnel(&self,_:&str)->Result<bool,String>{Ok(true)} async fn stop_tunnel(&self)->Result<(),String>{Ok(())} async fn site_responds(&self,_:u16)->Result<bool,String>{Ok(true)}
 }
 #[async_trait] impl Cloudflare for Fake {
  async fn tunnel(&self,_:&str,_:&str)->Result<Tunnel,CfError>{panic!("unexpected cloudflare call")} async fn ingress(&self,_:&str,_:&str,_:&[(String,String)])->Result<(),CfError>{panic!("unexpected cloudflare call")} async fn plan(&self,_:&str,_:&str,_:&str)->Result<DnsPlan,CfError>{panic!("unexpected cloudflare call")} async fn create_dns(&self,_:&str,_:&str,_:&str)->Result<String,CfError>{panic!("unexpected cloudflare call")} async fn delete_dns(&self,_:&str,_:&str)->Result<bool,CfError>{panic!("unexpected cloudflare call")} async fn delete_tunnel(&self,_:&str,_:&str)->Result<(),CfError>{panic!("unexpected cloudflare call")}
 }
 fn setup() -> (Deployer,tempfile::TempDir,DeployRequest) { let dir=tempfile::tempdir().expect("dir"); std::fs::write(dir.path().join("index.html"),"ok").expect("index"); let mut conn=Connection::open_in_memory().expect("db"); db::migrate(&mut conn).expect("migrate"); conn.execute("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES('s','s','x',1234,'now','now')",[]).expect("site"); let d=Deployer{db:Arc::new(Mutex::new(conn)),cf:Arc::new(Fake),secrets:Arc::new(MemoryStore::default()),supervisor:Arc::new(Fake)}; let q=DeployRequest{site_id:"s".into(),hostname:"example.com".into(),account_id:"a".into(),zone_id:"z".into(),tunnel_name:"t".into(),port:1234,root:dir.path().to_owned(),consent_dns:false}; (d,dir,q) }
 #[tokio::test] async fn refuses_without_consent_before_network() { let (d,_dir,q)=setup(); let result=d.deploy(q,&|_|{}).await.expect("deployment"); assert_eq!(result.state,"failed"); assert_eq!(result.error.as_deref(),Some("DNS consent is required")); }
 #[tokio::test] async fn persists_failure() { let (d,_dir,mut q)=setup(); q.consent_dns=true; let result=d.deploy(q,&|_|{}).await.expect("deployment"); assert_eq!(result.steps[1].state,"failed"); let db=d.db.lock().expect("lock"); let state:String=db.query_row("SELECT state FROM deployments WHERE id=?1",[result.id],|r|r.get(0)).expect("row"); assert_eq!(state,"failed"); }
}

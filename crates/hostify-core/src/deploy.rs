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
    async fn stop_owned_tunnel(&self,_id:&str)->Result<(),String>{self.stop_tunnel().await}
    async fn site_responds(&self, port: u16) -> Result<bool, String>;
    async fn verify_public(&self, hostname: &str, root: &std::path::Path) -> Result<(), String> {
let expected=Sha256::digest(std::fs::read(root.join("index.html")).map_err(|e| e.to_string())?); let client=reqwest::Client::builder().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?; let deadline=tokio::time::Instant::now()+Duration::from_secs(180); loop { if let Ok(resp)=client.get(format!("https://{}/",hostname)).send().await { if resp.status().is_success() { if let Ok(body)=resp.bytes().await { if Sha256::digest(&body)==expected { break; } } } } if tokio::time::Instant::now()>=deadline { return Err("DNS has not propagated yet".into()); } tokio::time::sleep(Duration::from_secs(5)).await; } Ok(())
    }

}
#[async_trait]
pub trait Cloudflare: Send + Sync {
    async fn tunnel(&self, account: &str, name: &str) -> Result<Tunnel,CfError>;
    async fn ingress(&self, account: &str, id: &str, entries: &[(String,String)]) -> Result<(),CfError>;
    async fn plan(&self, zone: &str, hostname: &str, tunnel: &str) -> Result<DnsPlan,CfError>;
    async fn create_dns(&self, zone: &str, hostname: &str, tunnel: &str) -> Result<String,CfError>;
    async fn delete_dns(&self, zone: &str, id: &str) -> Result<bool,CfError>;
    async fn delete_owned_dns(&self,z:&str,i:&str,_o:&str)->Result<bool,CfError>{self.delete_dns(z,i).await}
    async fn delete_tunnel(&self, account: &str, id: &str) -> Result<(),CfError>;
    async fn find_tunnel(&self, _a: &str, _n: &str) -> Result<Option<String>,CfError> { Err(unsupported()) }
    async fn create_tunnel(&self, _a: &str, _n: &str) -> Result<String,CfError> { Err(unsupported()) }
    async fn token(&self, _a: &str, _i: &str) -> Result<String,CfError> { Err(unsupported()) }
    async fn config(&self, _a: &str, _i: &str) -> Result<Option<serde_json::Value>,CfError> { Err(unsupported()) }
    async fn put_config(&self, _a: &str, _i: &str, _v: serde_json::Value) -> Result<(),CfError> { Err(unsupported()) }
    async fn preflight(&self, _z: &str, _h: &str) -> Result<(),CfError> { Err(unsupported()) }
    async fn owned_dns(&self, _z: &str, _h: &str, _o: &str) -> Result<Option<String>,CfError> { Err(unsupported()) }
    async fn create_owned_dns(&self, _z: &str, _h: &str, _t: &str, _o: &str) -> Result<String,CfError> { Err(unsupported()) }

    async fn dns_record(&self,_z:&str,_h:&str)->Result<Option<serde_json::Value>,CfError>{Err(unsupported())}
    async fn write_dns(&self,_z:&str,_i:&str,_v:serde_json::Value)->Result<(),CfError>{Err(unsupported())}
    async fn verify_dns(&self,_z:&str,_i:&str,_t:&str)->Result<(),CfError>{Err(unsupported())}

}
fn unsupported() -> CfError { CfError::Unexpected("Cloudflare adapter lacks journaled deployment operations".into()) }
#[async_trait]
impl Cloudflare for CloudflareClient {
    async fn tunnel(&self,a:&str,n:&str)->Result<Tunnel,CfError>{ self.get_or_create_tunnel(a,n).await }
    async fn ingress(&self,a:&str,i:&str,e:&[(String,String)])->Result<(),CfError>{ self.put_tunnel_ingress(a,i,e).await }
    async fn plan(&self,z:&str,h:&str,t:&str)->Result<DnsPlan,CfError>{ self.plan_dns(z,h,t).await }
    async fn create_dns(&self,z:&str,h:&str,t:&str)->Result<String,CfError>{ self.create_dns(z,h,t).await }
    async fn delete_dns(&self,z:&str,i:&str)->Result<bool,CfError>{ self.delete_dns_if_ours(z,i).await }
    async fn delete_owned_dns(&self,z:&str,i:&str,o:&str)->Result<bool,CfError>{CloudflareClient::delete_owned_dns(self,z,i,o).await}
    async fn delete_tunnel(&self,a:&str,i:&str)->Result<(),CfError>{ self.delete_tunnel(a,i).await }
    async fn find_tunnel(&self,a:&str,n:&str)->Result<Option<String>,CfError>{ CloudflareClient::find_tunnel(self,a,n).await }
    async fn create_tunnel(&self,a:&str,n:&str)->Result<String,CfError>{ CloudflareClient::create_tunnel(self,a,n).await }
    async fn token(&self,a:&str,i:&str)->Result<String,CfError>{ self.tunnel_token(a,i).await }
    async fn config(&self,a:&str,i:&str)->Result<Option<serde_json::Value>,CfError>{ self.get_ingress(a,i).await }
    async fn put_config(&self,a:&str,i:&str,v:serde_json::Value)->Result<(),CfError>{ CloudflareClient::put_config(self,a,i,v).await }
    async fn preflight(&self,z:&str,h:&str)->Result<(),CfError>{ self.dns_preflight(z,h).await }
    async fn owned_dns(&self,z:&str,h:&str,o:&str)->Result<Option<String>,CfError>{ self.find_deployment_dns(z,h,o).await }
    async fn create_owned_dns(&self,z:&str,h:&str,t:&str,o:&str)->Result<String,CfError>{ self.create_deployment_dns(z,h,t,o).await }

    async fn dns_record(&self,z:&str,h:&str)->Result<Option<serde_json::Value>,CfError>{CloudflareClient::dns_record(self,z,h).await}
    async fn write_dns(&self,z:&str,i:&str,v:serde_json::Value)->Result<(),CfError>{CloudflareClient::write_dns(self,z,i,v).await}
    async fn verify_dns(&self,z:&str,i:&str,t:&str)->Result<(),CfError>{CloudflareClient::verify_dns(self,z,i,t).await}

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
#[serde(default)]
struct Resources {
    request: Option<DeployRequest>, site_started:bool, connector_started:bool,
    tunnel_id:Option<String>, tunnel_created:bool, dns_id:Option<String>,
    tunnel_intent:Option<String>, dns_intent:bool, owner:String,
    previous_config:Option<serde_json::Value>, desired_config:Option<serde_json::Value>,
    ingress_pending:bool, rollback_pending:bool, previous_dns:Option<serde_json::Value>, dns_update_pending:bool, dns_update_target:Option<String>,
}
pub struct Deployer { pub db:Arc<Mutex<Connection>>, pub cf:Arc<dyn Cloudflare>, pub secrets:Arc<dyn SecretStore>, pub supervisor:Arc<dyn SiteRuntime> }
const LABELS: [&str;7]=["Validate website","Start local server","Verify local site","Create tunnel","Add DNS record","Start connector","Verify HTTPS"];
impl Deployer {
    fn save(&self,d:&Deployment,r:&Resources)->Result<(),DeployError> { let db=self.db.lock().map_err(|_| DeployError::Message("Database lock poisoned".into()))?; db.execute("INSERT INTO deployments (id,site_id,state,snapshot_json,created_resources_json,started_at,finished_at) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET state=excluded.state,snapshot_json=excluded.snapshot_json,created_resources_json=excluded.created_resources_json,finished_at=excluded.finished_at",params![d.id,d.site_id,d.state,serde_json::to_string(d)?,serde_json::to_string(r)?,d.started_at,d.finished_at])?; Ok(()) }
    fn emit(&self,d:&Deployment,r:&Resources,callback:&(dyn Fn(&Deployment)+Send+Sync))->Result<(),DeployError> { self.save(d,r)?; callback(d); Ok(()) }
    pub async fn deploy(&self, mut request:DeployRequest, callback: &(dyn Fn(&Deployment)+Send+Sync)) -> Result<Deployment,DeployError> {
        request.tunnel_name=format!("hostify-{}",request.site_id);
        if request.consent_dns {
            let db = self.db.lock().map_err(|_| DeployError::Message("Database lock poisoned".into()))?;
            let mut stmt = db.prepare("SELECT snapshot_json,created_resources_json FROM deployments WHERE state='live' AND site_id=?1")?;
            let rows = stmt.query_map([&request.site_id], |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?)))?;
            for row in rows {
                let (snapshot, resources) = row?;
                let old: Resources = serde_json::from_str(&resources)?;
                if old.request.as_ref().is_some_and(|q| q.hostname == request.hostname && q.account_id == request.account_id && q.zone_id == request.zone_id && q.port == request.port && q.root == request.root) {
                    // Reuse the physical tunnel, but repeat validation and public verification.
                    // A cached live snapshot is not evidence that this publish is healthy.
                    let _:Deployment=serde_json::from_str(&snapshot)?;
                    if let Some(name)=old.tunnel_intent {request.tunnel_name=name;}
                    break;
                }
            }
        }
        let mut d=Deployment { id:uuid::Uuid::new_v4().to_string(),site_id:request.site_id.clone(),state:"running".into(),steps:LABELS.iter().enumerate().map(|(i,s)| DeploymentStep{key:["validate_website","start_local_server","verify_local_site","create_tunnel","add_dns_record","start_connector","verify_https"][i].into(),label:(*s).into(),state:"pending".into(),detail:None}).collect(),error:None,started_at:Utc::now().to_rfc3339(),finished_at:None };
        let mut r=Resources{owner:d.id.clone(),request:Some(request.clone()),..Default::default()}; self.emit(&d,&r,callback)?;
        for i in 0..LABELS.len() { d.steps[i].state="running".into(); self.emit(&d,&r,callback)?;
            let result=self.step(i,&request,&mut r,&d,callback).await;
            if let Err(e)=result { d.steps[i].state="failed".into(); d.steps[i].detail=Some(e.to_string()); d.error=Some(e.to_string()); r.rollback_pending=true; self.save(&d,&r)?; let rollback=self.rollback(&request,&mut r,&d).await; if let Err(err)=rollback { d.error=Some(format!("{e}; rollback incomplete: {err}")); } d.state="failed".into(); d.finished_at=Some(Utc::now().to_rfc3339()); self.emit(&d,&r,callback)?; return Ok(d); }
            d.steps[i].state="done".into(); self.emit(&d,&r,callback)?;
        } d.state="live".into(); d.finished_at=Some(Utc::now().to_rfc3339()); self.emit(&d,&r,callback)?; Ok(d)
    }
    async fn step(&self,i:usize,q:&DeployRequest,r:&mut Resources,d:&Deployment,callback:&(dyn Fn(&Deployment)+Send+Sync))->Result<(),DeployError> {
        match i {
            0=>{ if !q.consent_dns { return Err(DeployError::Message("DNS consent is required".into())); } if !q.root.join("index.html").is_file() || q.hostname.is_empty() || q.port==0 { return Err(DeployError::Message("Invalid website or hostname".into())); } {
                    let db=self.db.lock().map_err(|_|DeployError::Message("Database lock poisoned".into()))?;
                    let mut stmt=db.prepare("SELECT created_resources_json FROM deployments WHERE site_id=?1 AND id!=?2 AND state IN ('running','failed')")?;
                    let rows=stmt.query_map(params![q.site_id,d.id],|row|row.get::<_,String>(0))?;
                    for row in rows {let previous:Resources=serde_json::from_str(&row?)?;if previous.rollback_pending {return Err(DeployError::Message("Previous deployment rollback pending; cleanup will be retried by the engine before publishing again".into()));}}
                }
                self.cf.preflight(&q.zone_id,&q.hostname).await?;
                let existing=self.cf.find_tunnel(&q.account_id,&q.tunnel_name).await?;
                if self.cf.plan(&q.zone_id,&q.hostname,existing.as_deref().unwrap_or("")).await?.action=="conflict" { return Err(CfError::Conflict.into()); }
            },
            1=>{ r.site_started=self.supervisor.start_site(&q.site_id).await.map_err(DeployError::Message)?; self.emit(d,r,callback)?; },
            2=>{ if !self.supervisor.site_responds(q.port).await.map_err(DeployError::Message)? { return Err(DeployError::Message("Local site did not respond".into())); } },
            3=>{
                if let Some(id) = self.cf.find_tunnel(&q.account_id,&q.tunnel_name).await? {
                    r.tunnel_id=Some(id); self.save(d,r)?;
                } else {
                    // Stable site name is journaled before POST for ambiguous-create recovery.
                    // Callers serialize deployment and recovery for the same site.
                    let name=q.tunnel_name.clone();
                    r.tunnel_intent=Some(name.clone()); self.save(d,r)?;
                    r.tunnel_id=Some(self.cf.create_tunnel(&q.account_id,&name).await?);
                    r.tunnel_created=true; self.save(d,r)?;
                }
                let id=r.tunnel_id.clone().unwrap();
                if self.cf.plan(&q.zone_id,&q.hostname,&id).await?.action == "conflict" { return Err(CfError::Conflict.into()); }
                let token=self.cf.token(&q.account_id,&id).await?;
                self.secrets.set(&format!("tunnel:{id}"),&token)?;
                let previous=if r.tunnel_created {None} else {self.cf.config(&q.account_id,&id).await?};
                let mut desired=previous.clone().unwrap_or_else(||serde_json::json!({"ingress":[]}));
                let entries=desired["ingress"].as_array_mut().ok_or_else(unsupported)?;
                entries.retain(|e|e.get("hostname").is_some());
                let entry=serde_json::json!({"hostname":q.hostname,"service":format!("http://127.0.0.1:{}",q.port)});
                entries.retain(|e|e["hostname"]!=q.hostname);
                entries.insert(0,entry);
                entries.push(serde_json::json!({"service":"http_status:404"}));
                r.previous_config=previous; r.desired_config=Some(desired.clone()); r.ingress_pending=true;
                self.save(d,r)?; self.cf.put_config(&q.account_id,&id,desired).await?;
            },
            4=>{
                let tunnel=r.tunnel_id.as_deref().ok_or_else(|| DeployError::Message("Missing tunnel".into()))?;
                let plan=self.cf.plan(&q.zone_id,&q.hostname,tunnel).await?;
                match plan.action.as_str() {
                    "create"=>{ r.dns_intent=true; self.save(d,r)?; r.dns_id=Some(self.cf.create_owned_dns(&q.zone_id,&q.hostname,tunnel,&r.owner).await?); self.save(d,r)?; self.cf.verify_dns(&q.zone_id,r.dns_id.as_deref().ok_or_else(unsupported)?,tunnel).await?; },
                    "reuse"=>(),
                    "update_own"=>{
                        let old=self.cf.dns_record(&q.zone_id,&q.hostname).await?.ok_or(CfError::Conflict)?;
                        let id=old["id"].as_str().ok_or_else(unsupported)?.to_owned();
                        if old["content"].as_str().is_some_and(|content|content.eq_ignore_ascii_case(&format!("{tunnel}.cfargotunnel.com"))) {return Ok(());}
                        r.dns_update_target=Some(format!("{tunnel}.cfargotunnel.com"));
                        r.previous_dns=Some(old.clone());r.dns_update_pending=true;self.save(d,r)?;
                        let mut updated=old;updated["content"]=serde_json::json!(format!("{tunnel}.cfargotunnel.com"));updated["proxied"]=serde_json::json!(true);
                        self.cf.write_dns(&q.zone_id,&id,updated).await?;
                        self.cf.verify_dns(&q.zone_id,&id,tunnel).await?;
                    }, _=>return Err(CfError::Conflict.into())
                }
            },
            5=>{ let tunnel=r.tunnel_id.as_deref().ok_or_else(|| DeployError::Message("Missing tunnel".into()))?; let token=self.secrets.get(&format!("tunnel:{tunnel}"))?.ok_or_else(|| DeployError::Message("Missing tunnel credential".into()))?; r.connector_started=self.supervisor.start_tunnel(&token).await.map_err(DeployError::Message)?; self.emit(d,r,callback)?; },
            6=>self.supervisor.verify_public(&q.hostname, &q.root).await.map_err(DeployError::Message)?,
            _=>return Err(DeployError::Message("Invalid step".into()))
        } Ok(())
    }
    async fn rollback(&self,q:&DeployRequest,r:&mut Resources,d:&Deployment)->Result<(),DeployError> {
        // Resolve ambiguous creates before cleanup. Never infer ownership from a hostname alone.
        let mut errors=Vec::new();
        if r.tunnel_id.is_none() { if let Some(name)=&r.tunnel_intent {
            match self.cf.find_tunnel(&q.account_id,name).await {
                Ok(id)=>{r.tunnel_id=id;r.tunnel_created=r.tunnel_id.is_some();self.save(d,r)?;},
                Err(e)=>errors.push(e.to_string()),
            }
        } }
        let mut dns_reconciled=true;
        if r.dns_intent {
            // Re-check the deployment tag even for a saved ID (records can be replaced).
            match self.cf.owned_dns(&q.zone_id,&q.hostname,&r.owner).await {
                Ok(id)=>{r.dns_id=id;self.save(d,r)?;},
                Err(e)=>{dns_reconciled=false;errors.push(e.to_string());},
            }
        }
        let mut connector_stopped=true;
        if r.connector_started || (r.tunnel_created && r.owner==d.id) { match self.supervisor.stop_owned_tunnel(r.tunnel_id.as_deref().unwrap_or("")).await { Ok(())=>{r.connector_started=false; self.save(d,r)?;},Err(e)=>{connector_stopped=false;errors.push(e)} } }
        if let Some(id)=r.dns_id.clone().filter(|_|dns_reconciled) { match self.cf.delete_owned_dns(&q.zone_id,&id,&r.owner).await { Ok(_)=>{r.dns_id=None; r.dns_intent=false; self.save(d,r)?;},Err(e)=>errors.push(e.to_string()) } }
        if r.dns_update_pending {
            if let Some(old)=r.previous_dns.clone() {
                let restore=async {
                    let current=self.cf.dns_record(&q.zone_id,&q.hostname).await?;
                    if let Some(current)=current {
                        if current["id"]==old["id"] && current["content"]==r.dns_update_target.clone().unwrap_or_else(||format!("{}.cfargotunnel.com",r.tunnel_id.as_deref().unwrap_or(""))) {
                            self.cf.write_dns(&q.zone_id,old["id"].as_str().ok_or_else(unsupported)?,old.clone()).await?;
                        }
                    }
                    Ok::<(),CfError>(())
                }.await;
                match restore {Ok(())=>{r.dns_update_pending=false;self.save(d,r)?;},Err(e)=>errors.push(e.to_string())}
            }
        }
        if let Some(id)=r.tunnel_id.clone() {
            let cleanup = async {
                if r.tunnel_created && r.owner==d.id {
                    if !connector_stopped {return Err(CfError::Unexpected("Connector stop pending before tunnel deletion".into()));}
                    // Ownership is journaled before creation. Configuration is irrelevant to deletion.
                    match self.cf.delete_tunnel(&q.account_id,&id).await {Ok(())=>(),Err(e) if e.is_not_found()=>(),Err(e)=>return Err(e)};
                    r.tunnel_created=false;r.tunnel_intent=None;r.tunnel_id=None;r.ingress_pending=false;
                } else if r.ingress_pending {
                    let mut current=self.cf.config(&q.account_id,&id).await?.unwrap_or_else(||serde_json::json!({"ingress":[]}));
                    if r.desired_config.as_ref()==Some(&current) && r.previous_config.is_some() {
                        current=r.previous_config.clone().unwrap();
                    } else {
                        let previous=r.previous_config.clone().unwrap_or_else(||serde_json::json!({"ingress":[]}));
                        let desired=r.desired_config.clone().unwrap_or_else(||serde_json::json!({"ingress":[]}));
                        let added:Vec<_>=desired["ingress"].as_array().ok_or_else(unsupported)?.iter().filter(|e|e.get("hostname").is_some() && !previous["ingress"].as_array().is_some_and(|p|p.contains(e))).cloned().collect();
                        current["ingress"].as_array_mut().ok_or_else(unsupported)?.retain(|e|!added.contains(e));
                        // Put back any old hostname entry we replaced, without overwriting a concurrent edit.
                        for entry in previous["ingress"].as_array().ok_or_else(unsupported)? {
                            if entry["hostname"]==q.hostname && !current["ingress"].as_array().unwrap().iter().any(|e|e["hostname"]==q.hostname) {
                                current["ingress"].as_array_mut().unwrap().insert(0,entry.clone());
                            }
                        }
                        if !current["ingress"].as_array().unwrap().iter().any(|e|e.get("hostname").is_none()) {current["ingress"].as_array_mut().unwrap().push(serde_json::json!({"service":"http_status:404"}));}
                    }
                    self.cf.put_config(&q.account_id,&id,current).await?;r.ingress_pending=false;
                }
                Ok::<(),CfError>(())
            }.await;
            if let Err(e)=cleanup {errors.push(e.to_string());} else {self.save(d,r)?;}
        }
        if r.site_started {match self.supervisor.stop_site(&q.site_id).await {Ok(())=>{r.site_started=false; self.save(d,r)?;},Err(e)=>errors.push(e)}}
        r.rollback_pending=!errors.is_empty(); self.save(d,r)?;
        if errors.is_empty(){Ok(())}else{Err(DeployError::Message(errors.join("; ")))}
    }
    pub async fn recover_interrupted(&self,callback:&(dyn Fn(&Deployment)+Send+Sync))->Result<Vec<Deployment>,DeployError> {
        let records={let db=self.db.lock().map_err(|_| DeployError::Message("Database lock poisoned".into()))?;
            let mut stmt=db.prepare("SELECT snapshot_json,created_resources_json FROM deployments WHERE state IN ('running','failed')")?;
            let rows=stmt.query_map([],|row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?; rows.collect::<Result<Vec<_>,_>>()?};
        let mut recovered=Vec::new();
        for (snapshot,resources) in records {
            let mut d:Deployment=serde_json::from_str(&snapshot)?; let mut r:Resources=serde_json::from_str(&resources)?;
            if d.state=="failed" && !r.rollback_pending {continue;}
            let q=r.request.clone().ok_or_else(||DeployError::Message("Missing recovery data".into()))?;
            r.rollback_pending=true; self.save(&d,&r)?;
            match self.rollback(&q,&mut r,&d).await {
                Ok(())=>{d.state="rolled_back".into(); d.error=Some("Deployment interrupted and rolled back".into());},
                Err(e)=>{d.state="failed".into();d.error=Some(format!("Recovery rollback incomplete: {e}"));}
            }
            d.finished_at=Some(Utc::now().to_rfc3339()); self.emit(&d,&r,callback)?; recovered.push(d);
        } Ok(recovered)
    }
}
#[cfg(test)] mod tests;

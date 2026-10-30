use anyhow::{Result,anyhow,bail};
use chrono::Utc;
use hostify_core::{cloudflare::CloudflareClient,deploy::{Deployer,SiteRuntime,DeployRequest},secrets::{SecretStore,WindowsCredentialStore}};
use rusqlite::{Connection,params};
use serde::{Serialize,Deserialize};
use serde_json::{Value,json};
use std::{path::{Path,PathBuf},sync::{Arc,Mutex},collections::HashMap};
use tokio::sync::broadcast;
pub type Database=Arc<Mutex<Connection>>;
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Site{pub id:String,pub name:String,pub source_path:String,pub local_port:u16,pub local_url:String,pub status:String,#[serde(skip_serializing_if="Option::is_none")]pub status_detail:Option<String>,#[serde(skip_serializing_if="Option::is_none")]pub domain_id:Option<String>,#[serde(skip_serializing_if="Option::is_none")]pub public_url:Option<String>,pub auto_start:bool,pub file_count:u64,pub size_bytes:u64,pub created_at:String,pub updated_at:String}
#[derive(Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Settings{pub launch_on_login:bool,pub start_engine_on_boot:bool,pub close_to_tray:bool,pub theme:String,pub base_port_start:u16}
impl Default for Settings{fn default()->Self{Self{launch_on_login:true,start_engine_on_boot:false,close_to_tray:true,theme:"system".into(),base_port_start:9000}}}
pub struct Store{pub detection:hostify_core::components::Detection,pub db:Database,pub root:PathBuf,pub events:broadcast::Sender<Value>,pub statuses:Mutex<HashMap<String,(String,Option<String>)>>}
impl Store{
 pub fn open(root:PathBuf)->Result<Arc<Self>>{Self::open_with_probe(root,Arc::new(hostify_core::components::ProcessProbe))}
 pub fn open_with_probe(root:PathBuf,runner:Arc<dyn hostify_core::components::ProbeRunner>)->Result<Arc<Self>>{for p in ["sites","bin","logs","run"]{std::fs::create_dir_all(root.join(p))?}let mut db=Connection::open(root.join("hostify.db"))?;hostify_core::db::migrate(&mut db)?;let (events,_)=broadcast::channel(512);Ok(Arc::new(Self{detection:hostify_core::components::Detection::new(root.join("bin"),runner),db:Arc::new(Mutex::new(db)),root,events,statuses:Mutex::new(HashMap::new())}))}
 pub fn db(&self)->Result<std::sync::MutexGuard<'_,Connection>>{self.db.lock().map_err(|_|anyhow!("Database unavailable"))}
 pub fn emit(&self,event:&str,payload:Value){let _=self.events.send(json!({"event":event,"payload":payload}));}
 pub fn sites(&self)->Result<Vec<Site>>{
 let db=self.db()?;let mut st=db.prepare("SELECT w.id,w.name,w.source_path,w.local_port,w.auto_start,w.file_count,w.size_bytes,w.created_at,w.updated_at,d.id,d.hostname,d.https_verified FROM websites w LEFT JOIN domains d ON d.site_id=w.id ORDER BY w.created_at,w.id")?;
 let rows=st.query_map([],|r|{let port:u16=r.get(3)?;let hostname:Option<String>=r.get(10)?;let verified:Option<bool>=r.get(11)?;Ok(Site{id:r.get(0)?,name:r.get(1)?,source_path:r.get(2)?,local_port:port,local_url:format!("http://127.0.0.1:{port}"),status:"stopped".into(),status_detail:None,auto_start:r.get(4)?,file_count:r.get(5)?,size_bytes:r.get(6)?,created_at:r.get(7)?,updated_at:r.get(8)?,domain_id:r.get(9)?,public_url:hostname.filter(|_|verified==Some(true)).map(|h|format!("https://{h}"))})})?;
 let mut sites=rows.collect::<rusqlite::Result<Vec<_>>>()?;let statuses=self.statuses.lock().map_err(|_|anyhow!("Status unavailable"))?;for site in &mut sites{if let Some((status,detail))=statuses.get(&site.id){site.status=status.clone();site.status_detail=detail.clone();}}Ok(sites)
 }
 pub fn site(&self,id:&str)->Result<Site>{self.sites()?.into_iter().find(|s|s.id==id).ok_or_else(||anyhow!("Website not found"))}
 pub fn status(&self,id:&str,status:&str,detail:Option<String>)->Result<()>{self.statuses.lock().map_err(|_|anyhow!("Status unavailable"))?.insert(id.into(),(status.into(),detail));self.emit("site-status",serde_json::to_value(self.site(id)?)?);Ok(())}
 pub fn activity(&self,level:&str,id:Option<&str>,message:&str)->Result<()>{let db=self.db()?;let ts=Utc::now().to_rfc3339();db.execute("INSERT INTO activity_logs(ts,level,site_id,message) VALUES(?1,?2,?3,?4)",params![ts,level,id,message])?;let number=db.last_insert_rowid();self.emit("activity",json!({"id":number,"ts":ts,"level":level,"siteId":id,"message":message}));drop(db);crate::supervisor::append_log(&self.root.join("logs/engine.log"),&format!("{ts} {level} {message}"))?;Ok(())}
 pub fn settings(&self)->Result<Settings>{let db=self.db()?;let mut st=db.prepare("SELECT value FROM application_settings WHERE key='settings'")?;let mut rows=st.query([])?;match rows.next()?{Some(r)=>Ok(serde_json::from_str(&r.get::<_,String>(0)?)?),None=>Ok(Settings::default())}}
 pub fn saved(&self,key:&str)->Result<Option<Value>>{let db=self.db()?;let mut st=db.prepare("SELECT value FROM application_settings WHERE key=?1")?;let mut rows=st.query([key])?;Ok(match rows.next()?{Some(r)=>Some(serde_json::from_str(&r.get::<_,String>(0)?)?),None=>None})}
 pub fn save(&self,key:&str,value:&Value)->Result<()>{self.db()?.execute("INSERT INTO application_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value.to_string()])?;Ok(())}
}
pub struct Engine{tunnel_observations:Mutex<HashMap<String,(bool,String)>>,background:Mutex<Vec<tokio::task::JoinHandle<()>>>,pub supervisor:Option<Arc<crate::supervisor::Supervisor>>,pub store:Arc<Store>,pub runtime:Arc<dyn SiteRuntime>,pub secrets:Arc<dyn SecretStore>,pub service:bool,pub operations:tokio::sync::Mutex<()>,pub publishing:Arc<tokio::sync::Mutex<()>>}
impl Engine{
 pub fn new(store:Arc<Store>,runtime:Arc<dyn SiteRuntime>,service:bool)->Arc<Self>{Arc::new(Self{tunnel_observations:Mutex::new(HashMap::new()),background:Mutex::new(Vec::new()),supervisor:None,store,runtime,secrets:Arc::new(WindowsCredentialStore),service,operations:tokio::sync::Mutex::new(()),publishing:Arc::new(tokio::sync::Mutex::new(()))})}
 pub fn production(store:Arc<Store>,supervisor:Arc<crate::supervisor::Supervisor>,service:bool)->Arc<Self>{Arc::new(Self{tunnel_observations:Mutex::new(HashMap::new()),background:Mutex::new(Vec::new()),store,runtime:supervisor.clone(),supervisor:Some(supervisor),secrets:Arc::new(WindowsCredentialStore),service,operations:tokio::sync::Mutex::new(()),publishing:Arc::new(tokio::sync::Mutex::new(()))})}
 pub async fn shutdown(&self){let tasks=match self.background.lock(){Ok(mut tasks)=>std::mem::take(&mut *tasks),Err(_)=>Vec::new()};for task in tasks{task.abort();let _=task.await;}}
 fn cf(&self)->Result<CloudflareClient>{Ok(CloudflareClient::production(self.secrets.get("cloudflare-token")?.ok_or_else(||anyhow!("Connect your Cloudflare account first"))?))}
 fn deployer(&self,cf:CloudflareClient,site:Option<&str>)->Deployer{Deployer{db:self.store.db.clone(),cf:Arc::new(cf),secrets:self.secrets.clone(),supervisor:match (&self.supervisor,site){(Some(runtime),Some(id))=>runtime.for_site(id),_=>self.runtime.clone()}}}
 pub async fn engine_status(&self)->Value {
  let [caddy,cloudflared]=self.store.detection.statuses(false).await;
  let mut status=json!({"running":true,"version":env!("CARGO_PKG_VERSION"),"mode":if self.service{"service"}else{"user"},
   "componentsReady":caddy.ready&&cloudflared.ready,"localHostingReady":caddy.ready,
   "caddy":caddy,"cloudflared":cloudflared,"dataDir":self.store.root,"binDir":self.store.root.join("bin")});
  if let Some(version)=caddy.version {status["caddyVersion"]=json!(version);}
  if let Some(version)=cloudflared.version {status["cloudflaredVersion"]=json!(version);}
  status
 }
 async fn preflight_caddy(&self,id:&str)->Result<()> {
  if !self.store.detection.caddy_ready().await {
   self.store.status(id,"error",Some(hostify_core::components::CADDY_UNAVAILABLE.into()))?;
   bail!(hostify_core::components::CADDY_UNAVAILABLE);
  }
  Ok(())
 }
 pub async fn startup(&self)->Result<()>{// Recovery runs before desired state is reconciled.
 let cf=CloudflareClient::production(self.secrets.get("cloudflare-token")?.unwrap_or_default());let s=self.store.clone();if let Err(e)=self.deployer(cf,None).recover_interrupted(&move|d|s.emit("deployment-update",json!(d))).await{self.store.activity("error",None,&format!("Deployment recovery requires attention: {e}"))?;}
 for site in self.store.sites()?{if site.auto_start{if let Err(e)=self.runtime.start_site(&site.id).await{self.store.activity("error",Some(&site.id),&e)?;}}}
 let records=self.live_resources()?;for r in records{if let Some(id)=r["tunnel_id"].as_str(){if let Some(token)=self.secrets.get(&format!("tunnel:{id}"))?{if let Err(e)=self.runtime.start_tunnel(&token).await{self.store.activity("error",None,&e)?;}}}}Ok(())}
 fn live_resources(&self)->Result<Vec<Value>>{let db=self.store.db()?;let mut st=db.prepare("SELECT created_resources_json FROM deployments WHERE state='live'")?;let rows=st.query_map([],|r|r.get::<_,String>(0))?;rows.map(|r|Ok(serde_json::from_str(&r?)?)).collect()}
 pub async fn dispatch(self:&Arc<Self>,method:&str,p:Value)->Result<Value>{
 let s=&self.store;
 match method{
 "ping"=>Ok(json!("pong")),"subscribe"=>Ok(Value::Null),"engine_status"=>Ok(self.engine_status().await),
 "ensure_components"=>{
  let _guard=self.operations.lock().await;
  let names=match p.get("names").filter(|v| !v.is_null()) {
   None=>vec!["caddy","cloudflared"],
   Some(v)=>v.as_array().ok_or_else(||anyhow!("names must be an array"))?.iter().map(|v|v.as_str().filter(|n|matches!(*n,"caddy"|"cloudflared")).ok_or_else(||anyhow!("Unknown component; choose caddy or cloudflared"))).collect::<Result<Vec<_>>>()?
  };
  let c=hostify_core::components::Components::new(s.root.join("bin"))?;
  for name in names {
   let result=c.ensure(&[name],s.detection.runner(),|n,percent|s.emit("components-progress",json!({"name":n,"percent":percent}))).await;
   s.detection.invalidate().await;
   result?;
  }
  Ok(self.engine_status().await)
 },
 "system_stats"=>{let mut system=sysinfo::System::new_all();system.refresh_all();let disks=sysinfo::Disks::new_with_refreshed_list();let disk=disks.list().iter().filter(|d|s.root.starts_with(d.mount_point())).max_by_key(|d|d.mount_point().as_os_str().len());Ok(json!({"cpuPercent":system.global_cpu_usage(),"ramUsedBytes":system.used_memory(),"ramTotalBytes":system.total_memory(),"diskUsedBytes":disk.map(|d|d.total_space()-d.available_space()).unwrap_or(0),"diskTotalBytes":disk.map(|d|d.total_space()).unwrap_or(0),"hostifyDataBytes":directory_bytes(&s.root)}))},
 "list_sites"=>Ok(json!(s.sites()?)),"get_site"=>Ok(json!(s.site(arg(&p,"id")?)?)),"preview_url"=>Ok(json!(s.site(arg(&p,"id")?)?.local_url)),
 "validate_source"=>{let path=arg(&p,"path")?;let kind=kind(&p)?;let temp=s.root.join("run").join(uuid::Uuid::new_v4().to_string());let report=hostify_core::import::import(Path::new(path),kind,&temp);let _=std::fs::remove_dir_all(temp);match report{Ok(r)=>Ok(json!(r)),Err(e)=>Ok(json!({"ok":false,"fileCount":0,"sizeBytes":0,"hasIndex":false,"warnings":[],"errors":[e.to_string()]}))}},
 "add_site"|"update_site_files"=>{let _guard=self.operations.lock().await;let _publish_guard=self.publishing.try_lock().map_err(|_|anyhow!("Wait for the current deployment to finish"))?;if method=="add_site"{let name=arg(&p,"name")?;if name.trim().is_empty()||name.len()>200{bail!("Website name must contain 1 to 200 characters")}}let source=arg(&p,"sourcePath")?;let kind=kind(&p)?;let id=if method=="add_site"{uuid::Uuid::new_v4().to_string()}else{let id=arg(&p,"id")?;s.site(id)?;id.into()};let report=hostify_core::import::import(Path::new(source),kind,&s.root.join("sites").join(&id))?;let now=Utc::now().to_rfc3339();if method=="add_site"{let name=arg(&p,"name")?;if name.trim().is_empty()||name.len()>200{bail!("Website name must contain 1 to 200 characters")};let reserved=s.sites()?.iter().map(|s|s.local_port).collect::<Vec<_>>();let port=hostify_core::caddy::allocate_port(s.settings()?.base_port_start,&reserved)?;s.db()?.execute("INSERT INTO websites(id,name,source_path,local_port,file_count,size_bytes,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",params![id,name,source,port,report.file_count,report.size_bytes,now])?;}else{s.db()?.execute("UPDATE websites SET source_path=?1,file_count=?2,size_bytes=?3,updated_at=?4 WHERE id=?5",params![source,report.file_count,report.size_bytes,now,id])?;}s.activity("info",Some(&id),"Website files imported")?;Ok(json!(s.site(&id)?))},
 "start_site"|"stop_site"|"restart_site"=>{let _guard=self.operations.lock().await;let _publish_guard=self.publishing.try_lock().map_err(|_|anyhow!("Wait for the current deployment to finish"))?;let id=arg(&p,"id")?;s.site(id)?;if method!="stop_site"{self.preflight_caddy(id).await?;}if method!="start_site"{self.runtime.stop_site(id).await.map_err(|e|anyhow!(e))?;s.status(id,"stopped",None)?;}if method!="stop_site"{s.status(id,"starting",None)?;match self.runtime.start_site(id).await{Ok(_)=>s.status(id,"running",None)?,Err(e)=>{s.status(id,"error",Some(e.clone()))?;bail!("{e}")}}}s.activity("info",Some(id),method)?;Ok(json!(s.site(id)?))},
 "remove_site"=>{let _guard=self.operations.lock().await;let _publish_guard=self.publishing.try_lock().map_err(|_|anyhow!("Wait for the current deployment to finish"))?;let id=arg(&p,"id")?;s.site(id)?;let delete=boolean(&p,"deleteFiles")?;if self.live_resources()?.iter().any(|r|r["request"]["siteId"]==id){bail!("Unpublish this website before removing it")};self.runtime.stop_site(id).await.map_err(|e|anyhow!(e))?;{let mut db=s.db()?;let tx=db.transaction()?;tx.execute("UPDATE domains SET site_id=NULL WHERE site_id=?1",[id])?;tx.execute("DELETE FROM deployments WHERE site_id=?1",[id])?;tx.execute("DELETE FROM websites WHERE id=?1",[id])?;tx.commit()?;}if delete{std::fs::remove_dir_all(s.root.join("sites").join(id))?}s.activity("info",Some(id),"Website removed")?;Ok(Value::Null)},
 "get_settings"=>Ok(json!(s.settings()?)),"save_settings"=>{let settings:Settings=serde_json::from_value(p)?;if settings.base_port_start==0||!["system","light","dark"].contains(&settings.theme.as_str()){bail!("Invalid settings")};#[cfg(windows)]{let old=s.settings()?;if old.launch_on_login!=settings.launch_on_login{crate::cli::task(settings.launch_on_login,&s.root)?;}if old.start_engine_on_boot!=settings.start_engine_on_boot{crate::cli::service_install(settings.start_engine_on_boot,&s.root)?;}}s.save("settings",&json!(settings))?;Ok(json!(settings))},
 "cf_status"=>Ok(s.saved("account")?.unwrap_or(json!({"connected":false}))),
 "cf_connect"=>{let token=arg(&p,"token")?;let cf=CloudflareClient::production(token);cf.verify_token().await?;let accounts=cf.list_accounts().await?;let account=accounts.first().ok_or_else(||anyhow!("No accessible Cloudflare account"))?;self.secrets.set("cloudflare-token",token)?;let hint=token.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<String>();let a=json!({"connected":true,"accountId":account.id,"accountName":account.name,"tokenHint":hint});s.save("account",&a)?;Ok(a)},
 "cf_disconnect"=>{self.secrets.delete("cloudflare-token")?;s.save("account",&json!({"connected":false}))?;Ok(Value::Null)},
 "cf_check_domain"=>{let cf=self.cf()?;let z=cf.find_zone(hostname(&p)?).await?;let problem=cf.check_dns_permission(&z.id).await.err().map(|e|e.to_string());Ok(json!({"zoneName":z.name,"zoneId":z.id,"canManageDns":problem.is_none(),"problems":problem.into_iter().collect::<Vec<_>>()}))},
 "cf_plan_dns"=>{let id=arg(&p,"siteId")?;s.site(id)?;let cf=self.cf()?;let h=hostname(&p)?;let z=cf.find_zone(h).await?;let tunnel=self.live_resources()?.iter().find(|r|r["request"]["siteId"]==id).and_then(|r|r["tunnel_id"].as_str()).unwrap_or("").to_owned();Ok(json!(cf.plan_dns(&z.id,h,&tunnel).await?))},
 "publish_site"=>{let _guard=self.operations.lock().await;let permit=self.publishing.clone().try_lock_owned().map_err(|_|anyhow!("A publish or unpublish operation is already running"))?;let id=arg(&p,"siteId")?.to_owned();let site=s.site(&id)?;let h=hostname(&p)?.to_owned();let consent=boolean(&p,"consentDns")?;if !consent{bail!("DNS consent is required")};if self.live_resources()?.iter().any(|r|r["request"]["siteId"]==id){bail!("Unpublish this website before publishing it again")};let statuses=s.detection.statuses(true).await;let missing=["Caddy","cloudflared"].into_iter().zip(statuses.iter()).filter(|(_,s)|!s.ready).map(|(name,_)|name).collect::<Vec<_>>();
 if !missing.is_empty(){bail!("Cannot publish: {} is not installed or can't run. Open the Dashboard and choose \"Install components\", then publish again.",missing.join(" and "));}let cf=self.cf()?;let z=cf.find_zone(&h).await?;let account=s.saved("account")?.ok_or_else(||anyhow!("Connect Cloudflare first"))?;let request=DeployRequest{site_id:id.clone(),hostname:h.clone(),account_id:arg(&account,"accountId")?.into(),zone_id:z.id.clone(),tunnel_name:format!("hostify-{id}"),port:site.local_port,root:s.root.join("sites").join(&id).join("current"),consent_dns:consent};let d=self.deployer(cf,Some(&id));let store=s.clone();let (tx,rx)=tokio::sync::oneshot::channel();let task=tokio::spawn(async move{let _permit=permit;let sender=Mutex::new(Some(tx));let callback=|dep:&hostify_core::deploy::Deployment|{store.emit("deployment-update",json!(dep));if let Ok(mut tx)=sender.lock(){if let Some(tx)=tx.take(){let _=tx.send(json!(dep));}}};let _=store.status(&id,"deploying",None);match d.deploy(request,&callback).await{Ok(dep)=>{if dep.state=="live"{if let Ok(db)=store.db(){let _=db.execute("INSERT INTO domains(id,hostname,zone_name,zone_id,site_id,dns_status,https_verified) VALUES(?1,?2,?3,?4,?5,'configured',1) ON CONFLICT(hostname) DO UPDATE SET site_id=excluded.site_id,dns_status='configured',https_verified=1",params![uuid::Uuid::new_v4().to_string(),h,z.name,z.id,id]);}let _=store.status(&id,"running",None);}else{let _=store.status(&id,"error",dep.error);}},Err(e)=>{let _=store.status(&id,"error",Some(e.to_string()));}}});self.background.lock().map_err(|_|anyhow!("Task registry unavailable"))?.push(task);Ok(rx.await?)},
 "get_deployment"|"list_deployments"=>{let db=s.db()?;let single=method=="get_deployment";let key=arg(&p,if single{"id"}else{"siteId"})?;let mut st=db.prepare(if single{"SELECT snapshot_json FROM deployments WHERE id=?1"}else{"SELECT snapshot_json FROM deployments WHERE site_id=?1 ORDER BY started_at DESC"})?;let rows=st.query_map([key],|r|r.get::<_,String>(0))?;let v=rows.map(|r|Ok(serde_json::from_str::<Value>(&r?)?)).collect::<Result<Vec<_>>>()?;if single{v.into_iter().next().ok_or_else(||anyhow!("Deployment not found"))}else{Ok(json!(v))}},
 "unpublish_site"=>{let _permit=self.publishing.lock().await;let id=arg(&p,"siteId")?;s.site(id)?;let cf=self.cf()?;for mut r in self.live_resources()?.into_iter().filter(|r|r["request"]["siteId"]==id){let q:DeployRequest=serde_json::from_value(r["request"].clone())?;if let Some(dns)=r["dns_id"].as_str(){cf.delete_dns_if_ours(&q.zone_id,dns).await?;}if let Some(tunnel)=r["tunnel_id"].as_str(){cf.put_tunnel_ingress(&q.account_id,tunnel,&[]).await?;}if let Some(supervisor)=&self.supervisor{supervisor.stop_site_tunnel(id).await.map_err(|e|anyhow!(e))?;}else{self.runtime.stop_tunnel().await.map_err(|e|anyhow!(e))?;}r["dns_id"]=Value::Null;let db=s.db()?;db.execute("UPDATE deployments SET state='rolled_back',snapshot_json=json_set(snapshot_json,'$.state','rolled_back'),created_resources_json=?1 WHERE site_id=?2 AND state='live'",params![r.to_string(),id])?;}s.db()?.execute("UPDATE domains SET site_id=NULL,dns_status='unconfigured',https_verified=0 WHERE site_id=?1",[id])?;s.status(id,"running",None)?;Ok(Value::Null)},
 "list_domains"=>{let db=s.db()?;let mut st=db.prepare("SELECT id,hostname,zone_name,zone_id,site_id,dns_status,https_verified FROM domains")?;let rows=st.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"hostname":r.get::<_,String>(1)?,"zoneName":r.get::<_,String>(2)?,"zoneId":r.get::<_,String>(3)?,"siteId":r.get::<_,Option<String>>(4)?,"dnsStatus":r.get::<_,String>(5)?,"httpsVerified":r.get::<_,bool>(6)?})))?;Ok(json!(rows.collect::<rusqlite::Result<Vec<_>>>()?))},
 "tunnel_health"=>{let mut v=Vec::new();for r in self.live_resources()?{if let Some(id)=r["tunnel_id"].as_str(){let port=crate::supervisor::metrics_port(id);let connected=crate::supervisor::http_ok(port,"/ready").await;let connections=crate::supervisor::tunnel_connections(port).await;let last_change={let mut observations=self.tunnel_observations.lock().map_err(|_|anyhow!("Tunnel status unavailable"))?;let observation=observations.entry(id.into()).or_insert_with(||(connected,Utc::now().to_rfc3339()));if observation.0!=connected{*observation=(connected,Utc::now().to_rfc3339());}observation.1.clone()};v.push(json!({"tunnelId":id,"name":r["request"]["tunnelName"],"connected":connected,"connections":connections,"lastChange":last_change}));}}Ok(json!(v))},
 "list_activity"=>{let limit=number(&p,"limit",1000)?;let site=p.get("siteId").and_then(Value::as_str);let db=s.db()?;let mut st=db.prepare("SELECT id,ts,level,site_id,message FROM activity_logs WHERE (?1 IS NULL OR site_id=?1) ORDER BY id DESC LIMIT ?2")?;let rows=st.query_map(params![site,limit],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"ts":r.get::<_,String>(1)?,"level":r.get::<_,String>(2)?,"siteId":r.get::<_,Option<String>>(3)?,"message":r.get::<_,String>(4)?})))?;Ok(json!(rows.collect::<rusqlite::Result<Vec<_>>>()?))},
 "site_logs"=>{let id=arg(&p,"siteId")?;s.site(id)?;let lines=number(&p,"lines",10000)? as usize;Ok(json!(crate::supervisor::tail_logs(&s.root.join("logs").join(format!("{id}.log")),lines)?))},
 "pick_path"=>{kind(&p)?;bail!("pick_path must be handled by the desktop shell's native dialog")},
 _=>bail!("Unknown method: {method}")
 }
 }
}
fn arg<'a>(p:&'a Value,k:&str)->Result<&'a str>{p.get(k).and_then(Value::as_str).filter(|s|!s.is_empty()).ok_or_else(||anyhow!("Missing or invalid {k}"))}
fn boolean(p:&Value,k:&str)->Result<bool>{p.get(k).and_then(Value::as_bool).ok_or_else(||anyhow!("Missing or invalid {k}"))}
fn number(p:&Value,k:&str,max:u64)->Result<u64>{p.get(k).and_then(Value::as_u64).filter(|n|*n<=max).ok_or_else(||anyhow!("{k} must be an integer from 0 to {max}"))}
fn kind(p:&Value)->Result<&str>{let k=arg(p,"kind")?;if !["directory","zip"].contains(&k){bail!("kind must be directory or zip")};Ok(k)}
fn hostname(p:&Value)->Result<&str>{let h=arg(p,"hostname")?;if h.len()>253||!h.contains('.')||h.split('.').any(|l|l.is_empty()||l.len()>63||l.starts_with('-')||l.ends_with('-')||!l.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-')){bail!("Enter a valid DNS hostname")};Ok(h)}
fn directory_bytes(path:&Path)->u64{std::fs::read_dir(path).ok().into_iter().flatten().filter_map(|e|e.ok()).map(|e|match e.file_type(){Ok(t)if t.is_dir()=>directory_bytes(&e.path()),Ok(t)if t.is_file()=>e.metadata().map(|m|m.len()).unwrap_or(0),_=>0}).sum()}





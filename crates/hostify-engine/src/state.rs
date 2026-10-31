use anyhow::{Result, anyhow, bail};
use chrono::Utc;
use hostify_core::{
    cloudflare::CloudflareClient,
    deploy::{DeployRequest, Deployer, SiteRuntime, VerifierControl},
    secrets::{SecretStore, WindowsCredentialStore},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;
pub type Database = Arc<Mutex<Connection>>;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub kind: String,
    pub runtime_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_deployment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_hostname: Option<String>,
    pub id: String,
    pub name: String,
    pub source_path: String,
    pub local_port: u16,
    pub local_url: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    pub auto_start: bool,
    pub file_count: u64,
    pub size_bytes: u64,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub launch_on_login: bool,
    pub start_engine_on_boot: bool,
    pub close_to_tray: bool,
    pub theme: String,
    pub base_port_start: u16,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            launch_on_login: true,
            start_engine_on_boot: false,
            close_to_tray: true,
            theme: "system".into(),
            base_port_start: 9000,
        }
    }
}
type SiteProcessHealth = HashMap<String, HashMap<String, (String, Option<String>)>>;
pub struct Store {
    pub process_health: Mutex<SiteProcessHealth>,
    pub runtimes: hostify_core::runtimes::Runtimes,
    pub detection: hostify_core::components::Detection,
    pub db: Database,
    pub root: PathBuf,
    pub events: broadcast::Sender<Value>,
    pub statuses: Mutex<HashMap<String, (String, Option<String>)>>,
}
impl Store {
    pub fn open(root: PathBuf) -> Result<Arc<Self>> {
        Self::open_with_probe(root, Arc::new(hostify_core::components::ProcessProbe))
    }
    pub fn open_with_probe(
        root: PathBuf,
        runner: Arc<dyn hostify_core::components::ProbeRunner>,
    ) -> Result<Arc<Self>> {
        for p in ["sites", "bin", "logs", "run"] {
            std::fs::create_dir_all(root.join(p))?
        }
        let mut db = Connection::open(root.join("hostify.db"))?;
        hostify_core::db::migrate(&mut db)?;
        {
            let interrupted: Vec<(String, String)> = {
                let mut q = db.prepare(
                    "SELECT key,value FROM application_settings WHERE key LIKE 'build:%'",
                )?;
                q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for (key, text) in interrupted {
                if let Ok(mut v) = serde_json::from_str::<Value>(&text) {
                    if v["state"] == "running" {
                        v["state"] = json!("failed");
                        v["detail"] = json!(
                            "The build was interrupted by an engine restart. Build the release again."
                        );
                        db.execute(
                            "UPDATE application_settings SET value=?1 WHERE key=?2",
                            params![v.to_string(), key],
                        )?;
                        if let Some(id) = key.strip_prefix("build:") {
                            let prior: Option<String> = db
                                .query_row(
                                    "SELECT value FROM application_settings WHERE key=?1",
                                    [format!("build-prior:{id}")],
                                    |r| r.get(0),
                                )
                                .optional()?;
                            if let Some(prior) = prior {
                                let previous: Value = serde_json::from_str(&prior)?;
                                let raw: Option<String> = db
                                    .query_row(
                                        "SELECT app_config_json FROM websites WHERE id=?1",
                                        [id],
                                        |r| r.get(0),
                                    )
                                    .optional()?
                                    .flatten();
                                if let Some(raw) = raw {
                                    let mut config: hostify_core::app_config::AppConfig =
                                        serde_json::from_str(&raw)?;
                                    config.built_release =
                                        serde_json::from_value(previous["builtRelease"].clone())?;
                                    config.trusted_release =
                                        serde_json::from_value(previous["trustedRelease"].clone())?;
                                    db.execute(
                                        "UPDATE websites SET app_config_json=?1 WHERE id=?2",
                                        params![serde_json::to_string(&config)?, id],
                                    )?;
                                }
                            }
                        }
                    }
                }
            }
        }
        let (events, _) = broadcast::channel(512);
        let runtimes = hostify_core::runtimes::Runtimes::new(&root).map_err(|e| anyhow!(e))?;
        runtimes.clean_partial().map_err(|e| anyhow!(e))?;
        Ok(Arc::new(Self {
            runtimes,
            detection: hostify_core::components::Detection::new(root.join("bin"), runner),
            db: Arc::new(Mutex::new(db)),
            root,
            events,
            statuses: Mutex::new(HashMap::new()),
            process_health: Mutex::new(HashMap::new()),
        }))
    }
    pub fn app_config(&self, id: &str) -> Result<hostify_core::app_config::AppConfig> {
        let text: Option<String> = self.db()?.query_row(
            "SELECT app_config_json FROM websites WHERE id=?1",
            [id],
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str(text.as_deref().unwrap_or("{}"))?)
    }
    pub fn save_app_config(
        &self,
        id: &str,
        config: &hostify_core::app_config::AppConfig,
    ) -> Result<()> {
        self.db()?.execute(
            "UPDATE websites SET app_config_json=?1,updated_at=?2 WHERE id=?3",
            params![serde_json::to_string(config)?, Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }
    pub fn reserved_ports(&self) -> Result<Vec<u16>> {
        let mut ports = Vec::new();
        for site in self.sites()? {
            ports.push(site.local_port);
            ports.extend(self.app_config(&site.id)?.php_ports);
            if let Some(p) = self.app_config(&site.id)?.internal_port {
                ports.push(p);
            }
        }
        Ok(ports)
    }
    pub fn process_status(
        &self,
        id: &str,
        key: &str,
        status: &str,
        detail: Option<String>,
    ) -> Result<()> {
        let mut all = self
            .process_health
            .lock()
            .map_err(|_| anyhow!("Process health unavailable"))?;
        let processes = all.entry(id.into()).or_default();
        processes.insert(key.into(), (status.into(), detail));
        let (status, detail) = aggregate_health(processes.values());
        // Serialize the aggregate write too, so an older healthy snapshot cannot
        // overwrite a concurrent application's failure.
        self.status(id, &status, detail)
    }

    pub fn db(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.db.lock().map_err(|_| anyhow!("Database unavailable"))
    }
    pub fn emit(&self, event: &str, payload: Value) {
        let _ = self.events.send(json!({"event":event,"payload":payload}));
    }
    pub fn sites(&self) -> Result<Vec<Site>> {
        let db = self.db()?;
        let mut st=db.prepare("SELECT w.id,w.name,w.source_path,w.local_port,w.auto_start,w.file_count,w.size_bytes,w.created_at,w.updated_at,d.id,d.hostname,d.https_verified,w.kind,w.runtime_version FROM websites w LEFT JOIN domains d ON d.site_id=w.id ORDER BY w.created_at,w.id")?;
        let rows = st.query_map([], |r| {
            let port: u16 = r.get(3)?;
            let hostname: Option<String> = r.get(10)?;
            let verified: Option<bool> = r.get(11)?;
            Ok(Site {
                kind: r.get(12)?,
                runtime_version: r.get(13)?,
                publish_state: None,
                publish_deployment_id: None,
                publish_hostname: None,
                id: r.get(0)?,
                name: r.get(1)?,
                source_path: r.get(2)?,
                local_port: port,
                local_url: format!("http://127.0.0.1:{port}"),
                status: "stopped".into(),
                status_detail: None,
                auto_start: r.get(4)?,
                file_count: r.get(5)?,
                size_bytes: r.get(6)?,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
                domain_id: r.get(9)?,
                public_url: hostname
                    .filter(|_| verified == Some(true))
                    .map(|h| format!("https://{h}")),
            })
        })?;
        let mut sites = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let statuses = self
            .statuses
            .lock()
            .map_err(|_| anyhow!("Status unavailable"))?;
        for site in &mut sites {
            let latest:Option<(String,String,String)>=db.query_row("SELECT id,state,snapshot_json FROM deployments WHERE site_id=?1 ORDER BY rowid DESC LIMIT 1",[&site.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).ok();
            // Only the newest attempt counts: an unpublished site must not show an older failure.
            if let Some((id, state, snapshot)) =
                latest.filter(|(_, state, _)| state != "rolled_back")
            {
                site.publish_state = Some(state);
                site.publish_deployment_id = Some(id);
                site.publish_hostname = serde_json::from_str::<Value>(&snapshot)
                    .ok()
                    .and_then(|v| v["hostname"].as_str().map(str::to_owned));
            }
            if let Some((status, detail)) = statuses.get(&site.id) {
                site.status = status.clone();
                site.status_detail = detail.clone();
            }
        }
        Ok(sites)
    }
    pub fn site(&self, id: &str) -> Result<Site> {
        self.sites()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow!("Website not found"))
    }
    pub fn status(&self, id: &str, status: &str, detail: Option<String>) -> Result<()> {
        self.statuses
            .lock()
            .map_err(|_| anyhow!("Status unavailable"))?
            .insert(id.into(), (status.into(), detail));
        self.emit("site-status", serde_json::to_value(self.site(id)?)?);
        Ok(())
    }
    pub fn activity(&self, level: &str, id: Option<&str>, message: &str) -> Result<()> {
        let db = self.db()?;
        let ts = Utc::now().to_rfc3339();
        db.execute(
            "INSERT INTO activity_logs(ts,level,site_id,message) VALUES(?1,?2,?3,?4)",
            params![ts, level, id, message],
        )?;
        let number = db.last_insert_rowid();
        db.execute("DELETE FROM activity_logs WHERE id NOT IN (SELECT id FROM activity_logs ORDER BY id DESC LIMIT 5000)",[])?;
        self.emit(
            "activity",
            json!({"id":number,"ts":ts,"level":level,"siteId":id,"message":message}),
        );
        drop(db);
        crate::supervisor::append_log(
            &self.root.join("logs/engine.log"),
            &format!("{ts} {level} {message}"),
        )?;
        Ok(())
    }
    pub fn deployment_update(&self, dep: &hostify_core::deploy::Deployment) {
        self.emit("deployment-update", json!(dep));
        if let Ok(site) = self.site(&dep.site_id) {
            self.emit("site-status", json!(site));
        }
    }
    pub fn settings(&self) -> Result<Settings> {
        let db = self.db()?;
        let mut st = db.prepare("SELECT value FROM application_settings WHERE key='settings'")?;
        let mut rows = st.query([])?;
        match rows.next()? {
            Some(r) => Ok(serde_json::from_str(&r.get::<_, String>(0)?)?),
            None => Ok(Settings::default()),
        }
    }
    pub fn saved(&self, key: &str) -> Result<Option<Value>> {
        let db = self.db()?;
        let mut st = db.prepare("SELECT value FROM application_settings WHERE key=?1")?;
        let mut rows = st.query([key])?;
        Ok(match rows.next()? {
            Some(r) => Some(serde_json::from_str(&r.get::<_, String>(0)?)?),
            None => None,
        })
    }
    pub fn save(&self, key: &str, value: &Value) -> Result<()> {
        self.db()?.execute("INSERT INTO application_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value.to_string()])?;
        Ok(())
    }
}
struct DeploymentTask {
    wake: tokio::sync::watch::Sender<VerifierControl>,
    task: tokio::task::JoinHandle<()>,
}
pub struct Engine {
    pub(crate) builds: Mutex<HashMap<String, crate::build::BuildTask>>,
    stats: Arc<Mutex<Stats>>,
    pub public_probe: Option<Arc<dyn hostify_core::verify::PublicProbe>>,
    verifiers: Mutex<HashMap<String, DeploymentTask>>,
    tunnel_observations: Mutex<HashMap<String, (bool, String)>>,
    background: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    pub supervisor: Option<Arc<crate::supervisor::Supervisor>>,
    pub store: Arc<Store>,
    pub runtime: Arc<dyn SiteRuntime>,
    pub secrets: Arc<dyn SecretStore>,
    pub service: bool,
    pub operations: tokio::sync::Mutex<()>,
    pub publishing: Arc<tokio::sync::Mutex<()>>,
}
impl Engine {
    pub fn new(store: Arc<Store>, runtime: Arc<dyn SiteRuntime>, service: bool) -> Arc<Self> {
        Arc::new(Self {
            stats: Arc::new(Mutex::new(Stats::new())),
            public_probe: None,
            verifiers: Mutex::new(HashMap::new()),
            tunnel_observations: Mutex::new(HashMap::new()),
            background: Mutex::new(Vec::new()),
            builds: Mutex::new(HashMap::new()),
            supervisor: None,
            store,
            runtime,
            secrets: Arc::new(WindowsCredentialStore),
            service,
            operations: tokio::sync::Mutex::new(()),
            publishing: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
    pub fn production(
        store: Arc<Store>,
        supervisor: Arc<crate::supervisor::Supervisor>,
        service: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            stats: Arc::new(Mutex::new(Stats::new())),
            public_probe: None,
            verifiers: Mutex::new(HashMap::new()),
            tunnel_observations: Mutex::new(HashMap::new()),
            background: Mutex::new(Vec::new()),
            builds: Mutex::new(HashMap::new()),
            store,
            runtime: supervisor.clone(),
            supervisor: Some(supervisor),
            secrets: Arc::new(WindowsCredentialStore),
            service,
            operations: tokio::sync::Mutex::new(()),
            publishing: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
    pub async fn shutdown(&self) {
        crate::build::shutdown(self).await;
        let tasks = self
            .verifiers
            .lock()
            .map(|mut t| std::mem::take(&mut *t))
            .unwrap_or_default();
        for (_, t) in tasks {
            t.task.abort();
            let _ = t.task.await;
        }
        let tasks = match self.background.lock() {
            Ok(mut tasks) => std::mem::take(&mut *tasks),
            Err(_) => Vec::new(),
        };
        for task in tasks {
            task.abort();
            let _ = task.await;
        }
    }
    fn logged_cf(&self, token: impl Into<String>) -> CloudflareClient {
        let path = self.store.root.join("logs/cloudflare.log");
        let cf = CloudflareClient::production(token).with_logger(move |line| {
            let _ = crate::supervisor::append_log(&path, line);
        });
        remember_tunnel_tokens(&cf, &self.store, self.secrets.as_ref());
        cf
    }
    fn cf(&self) -> Result<CloudflareClient> {
        Ok(self.logged_cf(
            self.secrets
                .get("cloudflare-token")?
                .ok_or_else(|| anyhow!("Connect your Cloudflare account first"))?,
        ))
    }
    fn deployer(&self, cf: CloudflareClient, site: Option<&str>) -> Deployer {
        {
            let runtime: Arc<dyn SiteRuntime> = match (&self.supervisor, site) {
                (Some(runtime), Some(id)) => runtime.for_site(id),
                _ => self.runtime.clone(),
            };
            Deployer {
                probe: self.public_probe.clone().unwrap_or_else(|| {
                    Arc::new(hostify_core::verify::NetworkProbe {
                        cf: cf.clone(),
                        runtime: runtime.clone(),
                    })
                }),
                db: self.store.db.clone(),
                cf: Arc::new(cf),
                secrets: self.secrets.clone(),
                supervisor: runtime,
            }
        }
    }
    pub async fn engine_status(&self) -> Value {
        let [caddy, cloudflared] = self.store.detection.statuses(false).await;
        let mut status = json!({"running":true,"version":env!("CARGO_PKG_VERSION"),"mode":if self.service{"service"}else{"user"},
   "componentsReady":caddy.ready&&cloudflared.ready,"localHostingReady":caddy.ready,
   "caddy":caddy,"cloudflared":cloudflared,"dataDir":self.store.root,"binDir":self.store.root.join("bin")});
        if let Some(version) = caddy.version {
            status["caddyVersion"] = json!(version);
        }
        if let Some(version) = cloudflared.version {
            status["cloudflaredVersion"] = json!(version);
        }
        status
    }
    async fn preflight_caddy(&self, id: &str) -> Result<()> {
        if !self.store.detection.caddy_ready().await {
            self.store.status(
                id,
                "error",
                Some(hostify_core::components::CADDY_UNAVAILABLE.into()),
            )?;
            bail!(hostify_core::components::CADDY_UNAVAILABLE);
        }
        Ok(())
    }
    pub async fn startup(self: &Arc<Self>) -> Result<()> {
        if let Some(supervisor) = &self.supervisor {
            if supervisor.mysql.should_start()? {
                if let Err(error) = supervisor.mysql_start().await {
                    self.store.activity("error", None, &error)?;
                }
            }
        }
        let _operations = self.operations.lock().await;
        let _publishing = self.publishing.lock().await;
        let pruning = {
            let db = self.store.db()?;
            hostify_core::db::prune(&db)
        };
        if let Err(e) = pruning {
            let _ = self.store.activity(
                "error",
                None,
                &format!("History cleanup needs attention: {e}"),
            );
        }
        let records = self.live_resources()?;
        // Start every local server before doing any connector or remote recovery work.
        for site in self.store.sites()? {
            if site.auto_start || records.iter().any(|r| r["request"]["siteId"] == site.id) {
                if let Err(e) = self.runtime.start_site(&site.id).await {
                    let _ = self.store.activity("error", Some(&site.id), &e);
                }
            }
        }
        // Retry durable cleanup failures while the engine remains running, serialized with publish.
        let store = self.store.clone();
        let secrets = self.secrets.clone();
        let runtime = self.runtime.clone();
        let publishing = self.publishing.clone();
        let retry = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                let Ok(_guard) = publishing.try_lock() else {
                    continue;
                };
                let pending = (|| -> Result<bool> {
                    let db = store.db()?;
                    let mut stmt=db.prepare("SELECT created_resources_json FROM deployments WHERE state IN ('running','validating','starting_local','provisioning_tunnel','configuring_dns','starting_connector','failed')")?;
                    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                    for row in rows {
                        let Ok(v) = serde_json::from_str::<Value>(&row?) else {
                            continue;
                        };
                        if v["rollback_pending"] == true {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                })();
                if !matches!(pending, Ok(true)) {
                    continue;
                }
                let token = match secrets.get("cloudflare-token") {
                    Ok(Some(token)) => token,
                    _ => continue,
                };
                let path = store.root.join("logs/cloudflare.log");
                let cf = CloudflareClient::production(token).with_logger(move |line| {
                    let _ = crate::supervisor::append_log(&path, line);
                });
                remember_tunnel_tokens(&cf, &store, secrets.as_ref());
                let deployer = Deployer {
                    probe: Arc::new(hostify_core::verify::NetworkProbe {
                        cf: cf.clone(),
                        runtime: runtime.clone(),
                    }),
                    db: store.db.clone(),
                    cf: Arc::new(cf),
                    secrets: secrets.clone(),
                    supervisor: runtime.clone(),
                };
                let events = store.clone();
                if let Err(e) = deployer
                    .recover_interrupted(&move |d| events.deployment_update(d))
                    .await
                {
                    let _ = store.activity(
                        "error",
                        None,
                        &format!("Deployment recovery requires attention: {e}"),
                    );
                }
            }
        });
        self.background
            .lock()
            .map_err(|_| anyhow!("Background tasks unavailable"))?
            .push(retry);
        for r in records {
            if let Some(id) = r["tunnel_id"].as_str() {
                let result = async {
                    let token = self
                        .secrets
                        .get(&format!("tunnel:{id}"))?
                        .ok_or_else(|| anyhow!("Saved connector credential is missing"))?;
                    self.runtime
                        .resume_tunnel(id, &token)
                        .await
                        .map_err(|e| anyhow!(e))
                }
                .await;
                if let Err(e) = result {
                    let _ = self.store.activity(
                        "error",
                        r["request"]["siteId"].as_str(),
                        &format!("Could not resume the Cloudflare connector: {e}"),
                    );
                }
            }
        }
        let waiting = {
            let db = self.store.db()?;
            let mut st=db.prepare("SELECT id,site_id FROM deployments WHERE state IN ('waiting_dns','waiting_tls','verifying_public')")?;
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, site) in waiting {
            if let Err(e) = self.start_verifier(&id, &site, false) {
                let _=self.store.activity("error",Some(&site),&format!("Public access checks could not resume. Connect your Cloudflare account and choose Check again. {e}"));
            }
        }
        let cf = self.logged_cf(
            self.secrets
                .get("cloudflare-token")
                .ok()
                .flatten()
                .unwrap_or_default(),
        );
        let store = self.store.clone();
        if let Err(e) = self
            .deployer(cf, None)
            .recover_interrupted(&move |d| store.deployment_update(d))
            .await
        {
            let _ = self.store.activity(
                "error",
                None,
                &format!("Deployment recovery requires attention: {e}"),
            );
        }
        // Rollback may have stopped a local server started by an interrupted deployment.
        for site in self.store.sites()? {
            if site.auto_start {
                if let Err(e) = self.runtime.start_site(&site.id).await {
                    let _ = self.store.activity("error", Some(&site.id), &e);
                }
            }
        }
        Ok(())
    }

    fn live_resources(&self) -> Result<Vec<Value>> {
        let db = self.store.db()?;
        let mut st=db.prepare("SELECT created_resources_json FROM deployments WHERE state IN ('waiting_dns','waiting_tls','verifying_public','recovery_required','published')")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut records = Vec::new();
        let mut malformed = false;
        for row in rows {
            match serde_json::from_str(&row?) {
                Ok(v) => records.push(v),
                Err(_) => malformed = true,
            }
        }
        drop(st);
        drop(db);
        if malformed {
            let _=self.store.activity("error",None,"A saved deployment has invalid recovery data and needs attention; its resources have been kept");
        }
        Ok(records)
    }
    fn start_verifier(&self, id: &str, site: &str, force: bool) -> Result<Value> {
        let mut tasks = self
            .verifiers
            .lock()
            .map_err(|_| anyhow!("Verifier registry unavailable"))?;
        let d = self.deployer(self.cf()?, Some(site));
        if let Some(t) = tasks.get(id).filter(|t| !t.task.is_finished()) {
            let dep = d.reset_verification(id, force)?;
            t.wake.send_replace(VerifierControl::Retry);
            return Ok(json!(dep));
        }
        let dep = d.reset_verification(id, force)?;
        let (wake, rx) = tokio::sync::watch::channel(VerifierControl::Retry);
        let store = self.store.clone();
        let key = id.to_owned();
        let task = tokio::spawn(async move {
            if let Err(e) = d
                .run_verifier(&key, rx, &|dep| {
                    store.deployment_update(dep);
                })
                .await
            {
                let _ = store.activity("error", None, &e.to_string());
            }
        });
        tasks.insert(id.into(), DeploymentTask { wake, task });
        Ok(json!(dep))
    }
    async fn stop_verifier(&self, id: &str) -> Result<()> {
        let task = self
            .verifiers
            .lock()
            .map_err(|_| anyhow!("Verifier registry unavailable"))?
            .remove(id);
        if let Some(t) = task {
            let state: Option<String> = self
                .store
                .db()?
                .query_row("SELECT state FROM deployments WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .ok();
            if state
                .as_deref()
                .is_some_and(hostify_core::deploy::verification_state)
            {
                t.wake.send_replace(VerifierControl::Cancel);
            } else {
                t.task.abort();
            }
            let _ = t.task.await;
        }
        Ok(())
    }
    fn runtime_usage(&self) -> Result<HashMap<(String, String), Vec<String>>> {
        let db = self.store.db()?;
        let mut st = db.prepare(
            "SELECT id,kind,runtime_version FROM websites WHERE runtime_version IS NOT NULL",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut usage: HashMap<(String, String), Vec<String>> = HashMap::new();
        for row in rows {
            let (id, kind, version) = row?;
            usage.entry((kind, version)).or_default().push(id);
        }
        Ok(usage)
    }
    pub async fn dispatch(self: &Arc<Self>, method: &str, p: Value) -> Result<Value> {
        if crate::mysql::is_command(method) {
            return crate::mysql::dispatch(self, method, p).await;
        }
        let s = &self.store;
        match method {
            "inspect_project" => {
                let path = PathBuf::from(arg(&p, "path")?);
                let kind = kind(&p)?.to_owned();
                Ok(json!(
                    tokio::task::spawn_blocking(move || hostify_core::project::inspect(
                        &path, &kind
                    ))
                    .await?
                    .map_err(|e| anyhow!(e))?
                ))
            }
            "get_app_config" => {
                let id = arg(&p, "siteId")?;
                s.site(id)?;
                Ok(json!(s.app_config(id)?))
            }
            "set_app_config" => {
                let _guard = self.operations.lock().await;
                let id = arg(&p, "siteId")?;
                let site = s.site(id)?;
                if !matches!(site.kind.as_str(), "node" | "php") {
                    bail!("Application configuration is available for Node and PHP websites.");
                }
                crate::build::refuse_busy(self, id)?;
                let old = s.app_config(id)?;
                let mut config: hostify_core::app_config::AppConfig = serde_json::from_value(
                    p.get("config")
                        .cloned()
                        .ok_or_else(|| anyhow!("Application configuration is required."))?,
                )?;
                config.internal_port = old.internal_port;
                config.php_ports = old.php_ports.clone();
                config.active_release = old.active_release.clone();
                config.previous_release = old.previous_release.clone();
                config.pending_release = old.pending_release.clone();
                config.built_release = old.built_release.clone();
                config.trusted_release = old.trusted_release.clone();
                let release = config
                    .pending_release
                    .as_ref()
                    .or(config.active_release.as_ref())
                    .ok_or_else(|| anyhow!("Import a release first."))?;
                config
                    .validate_kind(
                        &s.root.join("sites").join(id).join("app").join(release),
                        &site.kind,
                    )
                    .map_err(|e| anyhow!(e))?;
                let version = p
                    .get(if site.kind == "php" {
                        "phpVersion"
                    } else {
                        "nodeVersion"
                    })
                    .and_then(Value::as_str)
                    .or(site.runtime_version.as_deref())
                    .ok_or_else(|| anyhow!("Choose a runtime version."))?;
                if !hostify_core::runtimes::is_pinned(&site.kind, version) {
                    bail!("Choose a pinned runtime version.");
                }
                let rebuild = config.install != old.install
                    || config.build_script != old.build_script
                    || config.allow_install_scripts != old.allow_install_scripts
                    || config.working_dir != old.working_dir
                    || site.runtime_version.as_deref() != Some(version);
                let changed = config != old || site.runtime_version.as_deref() != Some(version);
                if rebuild {
                    config.built_release = None;
                }
                s.save_app_config(id, &config)?;
                s.db()?.execute(
                    "UPDATE websites SET runtime_version=?1 WHERE id=?2",
                    params![version, id],
                )?;
                Ok(
                    json!({"restartRequired":changed && site.status!="stopped","rebuildRequired":rebuild}),
                )
            }
            "build_app" => crate::build::begin(self, &p).await,
            "cancel_build" => {
                crate::build::cancel(self, arg(&p, "siteId")?).await?;
                Ok(Value::Null)
            }
            "get_build_status" => {
                let id = arg(&p, "siteId")?;
                s.site(id)?;
                Ok(s.saved(&format!("build:{id}"))?.unwrap_or(Value::Null))
            }
            "app_status" => {
                let id = arg(&p, "siteId")?;
                s.site(id)?;
                match &self.supervisor {
                    Some(supervisor) => {
                        let supervisor = supervisor.clone();
                        let id = id.to_owned();
                        tokio::task::spawn_blocking(move || supervisor.app_status(&id))
                            .await?
                            .map_err(|e| anyhow!(e))
                    }
                    None => Ok(Value::Null),
                }
            }
            "app_logs" => {
                let id = arg(&p, "siteId")?;
                s.site(id)?;
                let stream = arg(&p, "stream")?;
                let suffix = match stream {
                    "app" => "app.log",
                    "build" => "build.log",
                    "access" => "log",
                    _ => bail!("Choose app, build or access logs."),
                };
                let lines = p
                    .get("lines")
                    .and_then(Value::as_u64)
                    .unwrap_or(100)
                    .min(5000) as usize;
                Ok(json!(crate::supervisor::tail_logs(
                    &s.root.join("logs").join(format!("{id}.{suffix}")),
                    lines
                )?))
            }
            "list_runtimes" => {
                let usage = self.runtime_usage()?;
                Ok(json!(s.runtimes.list(usage).await.map_err(|e| anyhow!(e))?))
            }
            "install_runtime" => {
                let kind = arg(&p, "kind")?.to_owned();
                let version = arg(&p, "version")?.to_owned();
                let store = s.clone();
                let (reply, result) = tokio::sync::oneshot::channel();
                // A disconnected client must not interrupt extraction or release the
                // runtime lock while a blocking worker still writes its staging tree.
                let task = tokio::spawn(async move {
                    let events = store.clone();
                    let k = kind.clone();
                    let v = version.clone();
                    let result = store.runtimes.install(&kind, &version, Arc::new(move |phase,done,total| {
                        events.emit("runtime-progress", json!({"kind":k,"version":v,"phase":phase,"done":done,"total":total}));
                    })).await;
                    if let Err(error) = &result {
                        let _ = store.activity(
                            "error",
                            None,
                            &format!("Runtime installation failed: {error}"),
                        );
                    }
                    let _ = reply.send(result);
                });
                {
                    let mut tasks = self
                        .background
                        .lock()
                        .map_err(|_| anyhow!("Background tasks unavailable"))?;
                    tasks.retain(|task| !task.is_finished());
                    tasks.push(task);
                }
                result
                    .await
                    .map_err(|_| anyhow!("Runtime installation stopped. Retry the installation."))?
                    .map_err(|e| anyhow!(e))?;
                Ok(Value::Null)
            }
            "remove_runtime" => {
                let kind = arg(&p, "kind")?;
                let version = arg(&p, "version")?;
                let usage = self.runtime_usage()?;
                let used = usage
                    .get(&(kind.into(), version.into()))
                    .cloned()
                    .unwrap_or_default();
                s.runtimes
                    .remove(kind, version, &used)
                    .await
                    .map_err(|e| anyhow!(e))?;
                Ok(Value::Null)
            }
            "list_env_vars" | "set_env_var" | "delete_env_var" | "reveal_env_var"
            | "import_env" | "export_env" => {
                use hostify_core::env_vars;
                let id = arg(&p, "siteId")?;
                let site = s.site(id)?;
                let db = s.db()?;
                let restart = site.kind != "static"
                    && matches!(site.status.as_str(), "running" | "starting" | "error");
                let result: std::result::Result<Value, String> = (|| match method {
                    "list_env_vars" => Ok(json!(env_vars::list(&db, id)?)),
                    "reveal_env_var" => Ok(json!(env_vars::reveal(
                        &db,
                        id,
                        arg(&p, "name").map_err(|_| "Variable name is required.")?
                    )?)),
                    "export_env" => Ok(json!(env_vars::export_nonsecret(&db, id)?)),
                    "set_env_var" => {
                        let name = arg(&p, "name").map_err(|_| "Variable name is required.")?;
                        let value = arg(&p, "value").map_err(|_| "Variable value must be text.")?;
                        let secret =
                            boolean(&p, "secret").map_err(|_| "Secret must be a boolean.")?;
                        env_vars::set(&db, id, name, value, secret)?;
                        Ok(json!({"restartRequired":restart}))
                    }
                    "delete_env_var" => {
                        env_vars::delete(
                            &db,
                            id,
                            arg(&p, "name").map_err(|_| "Variable name is required.")?,
                        )?;
                        Ok(json!({"restartRequired":restart}))
                    }
                    _ => {
                        let text =
                            arg(&p, "text").map_err(|_| "Environment file text is required.")?;
                        let imported = env_vars::import_dotenv(&db, id, text, &[])?;
                        Ok(
                            json!({"imported":imported.imported,"errors":imported.errors,"restartRequired":restart && imported.imported>0}),
                        )
                    }
                })();
                result.map_err(|e| anyhow!(e))
            }
            "ping" => Ok(json!("pong")),
            "subscribe" => Ok(Value::Null),
            "engine_status" => Ok(self.engine_status().await),
            "ensure_components" => {
                let _guard = self.operations.lock().await;
                let names = match p.get("names").filter(|v| !v.is_null()) {
                    None => vec!["caddy", "cloudflared"],
                    Some(v) => v
                        .as_array()
                        .ok_or_else(|| anyhow!("names must be an array"))?
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .filter(|n| matches!(*n, "caddy" | "cloudflared"))
                                .ok_or_else(|| {
                                    anyhow!("Unknown component; choose caddy or cloudflared")
                                })
                        })
                        .collect::<Result<Vec<_>>>()?,
                };
                let c = hostify_core::components::Components::new(s.root.join("bin"))?;
                for name in names {
                    let result = c
                        .ensure(&[name], s.detection.runner(), |n, percent| {
                            s.emit("components-progress", json!({"name":n,"percent":percent}))
                        })
                        .await;
                    s.detection.invalidate().await;
                    result?;
                }
                Ok(self.engine_status().await)
            }
            "system_stats" => {
                let stats = self.stats.clone();
                let root = s.root.clone();
                tokio::task::spawn_blocking(move || {
                    stats
                        .lock()
                        .map_err(|_| anyhow!("System statistics unavailable"))?
                        .read(&root)
                })
                .await?
            }
            "list_sites" => Ok(json!(s.sites()?)),
            "get_site" => Ok(json!(s.site(arg(&p, "id")?)?)),
            "preview_url" => Ok(json!(s.site(arg(&p, "id")?)?.local_url)),
            "validate_source" => {
                let path = arg(&p, "path")?;
                let kind = kind(&p)?;
                let temp = s.root.join("run").join(uuid::Uuid::new_v4().to_string());
                let report = hostify_core::import::import(Path::new(path), kind, &temp);
                let _ = std::fs::remove_dir_all(temp);
                match report {
                    Ok(r) => Ok(json!(r)),
                    Err(e) => Ok(
                        json!({"ok":false,"fileCount":0,"sizeBytes":0,"hasIndex":false,"warnings":[],"errors":[e.to_string()]}),
                    ),
                }
            }
            "add_site" | "update_site_files" => {
                let _guard = self.operations.lock().await;
                let _publish_guard = self
                    .publishing
                    .try_lock()
                    .map_err(|_| anyhow!("Wait for the current deployment to finish"))?;
                if method == "add_site" {
                    let name = arg(&p, "name")?;
                    if name.trim().is_empty() || name.len() > 200 {
                        bail!("Website name must contain 1 to 200 characters")
                    }
                }
                let source = arg(&p, "sourcePath")?;
                let kind = kind(&p)?;
                let id = if method == "add_site" {
                    uuid::Uuid::new_v4().to_string()
                } else {
                    let id = arg(&p, "id")?;
                    s.site(id)?;
                    id.into()
                };
                let site_kind = if method == "add_site" {
                    p.get("siteKind")
                        .and_then(Value::as_str)
                        .unwrap_or("static")
                        .to_owned()
                } else {
                    s.site(&id)?.kind
                };
                if !["static", "node", "php"].contains(&site_kind.as_str()) {
                    bail!("Choose static, node or php as the website kind.");
                }
                if matches!(site_kind.as_str(), "node" | "php") {
                    crate::build::refuse_busy(self, &id)?;
                    let inspection = hostify_core::project::inspect(Path::new(source), kind)
                        .map_err(|e| anyhow!(e))?;
                    let mut config = if method == "add_site" {
                        match p.get("config").filter(|v| !v.is_null()) {
                            Some(v) => serde_json::from_value(v.clone())?,
                            None => inspection.suggested,
                        }
                    } else {
                        s.app_config(&id)?
                    };
                    if method == "add_site" {
                        config.internal_port = None;
                        config.php_ports.clear();
                        config.active_release = None;
                        config.previous_release = None;
                        config.pending_release = None;
                        config.built_release = None;
                        config.trusted_release = None;
                    }
                    let site_dir = s.root.join("sites").join(&id);
                    let (report, release) =
                        hostify_core::import::import_release(Path::new(source), kind, &site_dir)?;
                    if let Err(e) =
                        config.validate_kind(&site_dir.join("app").join(&release), &site_kind)
                    {
                        // Leave no refused import behind; add_site's folder is brand new.
                        let _ = std::fs::remove_dir_all(if method == "add_site" {
                            site_dir.clone()
                        } else {
                            site_dir.join("app").join(&release)
                        });
                        bail!(e);
                    }
                    let recommended_runtime = if hostify_core::runtimes::is_pinned(
                        &site_kind,
                        &inspection.recommended_runtime,
                    ) {
                        inspection.recommended_runtime
                    } else {
                        hostify_core::runtimes::pinned_versions(&site_kind)
                            .first()
                            .ok_or_else(|| anyhow!("No pinned runtime for this website kind."))?
                            .to_string()
                    };
                    let now = Utc::now().to_rfc3339();
                    if method == "add_site" {
                        config.active_release = None;
                        config.previous_release = None;
                        config.pending_release = Some(release);
                        config.built_release = None;
                        config.trusted_release = None;
                        let mut reserved = s.reserved_ports()?;
                        let port = hostify_core::caddy::allocate_port(
                            s.settings()?.base_port_start,
                            &reserved,
                        )?;
                        reserved.push(port);
                        if site_kind == "node" {
                            config.internal_port = Some(hostify_core::caddy::allocate_port(
                                s.settings()?.base_port_start,
                                &reserved,
                            )?);
                        }
                        s.db()?.execute("INSERT INTO websites(id,name,source_path,local_port,file_count,size_bytes,created_at,updated_at,kind,runtime_version,app_config_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?7,?10,?8,?9)",params![id,arg(&p,"name")?,source,port,report.file_count,report.size_bytes,now,recommended_runtime,serde_json::to_string(&config)?,site_kind])?;
                    } else {
                        config.pending_release = Some(release);
                        s.save_app_config(&id, &config)?;
                        s.db()?.execute("UPDATE websites SET source_path=?1,file_count=?2,size_bytes=?3,updated_at=?4 WHERE id=?5",params![source,report.file_count,report.size_bytes,now,id])?;
                    }
                    s.activity("info",Some(&id),"Application release imported; acknowledge trust and build before starting.")?;
                    return Ok(json!(s.site(&id)?));
                }
                let report = hostify_core::import::import(
                    Path::new(source),
                    kind,
                    &s.root.join("sites").join(&id),
                )?;
                let now = Utc::now().to_rfc3339();
                if method == "add_site" {
                    let name = arg(&p, "name")?;
                    if name.trim().is_empty() || name.len() > 200 {
                        bail!("Website name must contain 1 to 200 characters")
                    };
                    let reserved = s.reserved_ports()?;
                    let port = hostify_core::caddy::allocate_port(
                        s.settings()?.base_port_start,
                        &reserved,
                    )?;
                    s.db()?.execute("INSERT INTO websites(id,name,source_path,local_port,file_count,size_bytes,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",params![id,name,source,port,report.file_count,report.size_bytes,now])?;
                } else {
                    s.db()?.execute("UPDATE websites SET source_path=?1,file_count=?2,size_bytes=?3,updated_at=?4 WHERE id=?5",params![source,report.file_count,report.size_bytes,now,id])?;
                }
                s.activity("info", Some(&id), "Website files imported")?;
                Ok(json!(s.site(&id)?))
            }
            "start_site" | "stop_site" | "restart_site" => {
                let _guard = self.operations.lock().await;
                let _publish_guard = self
                    .publishing
                    .try_lock()
                    .map_err(|_| anyhow!("Wait for the current deployment to finish"))?;
                let id = arg(&p, "id")?;
                s.site(id)?;
                crate::build::refuse_busy(self, id)?;
                if method != "stop_site" {
                    self.preflight_caddy(id).await?;
                }
                if method != "start_site" {
                    self.runtime.stop_site(id).await.map_err(|e| anyhow!(e))?;
                    s.status(id, "stopped", None)?;
                }
                if method != "stop_site" {
                    s.status(id, "starting", None)?;
                    match self.runtime.start_site(id).await {
                        Ok(_) => {
                            if matches!(s.site(id)?.kind.as_str(), "node" | "php") {
                                s.process_status(id, &format!("site:{id}"), "running", None)?;
                            } else {
                                s.status(id, "running", None)?;
                            }
                        }
                        Err(e) => {
                            if !matches!(s.site(id)?.kind.as_str(), "node" | "php")
                                || s.site(id)?.status != "running"
                            {
                                s.status(id, "error", Some(e.clone()))?;
                            }
                            bail!("{e}")
                        }
                    }
                }
                s.activity(
                    "info",
                    Some(id),
                    match method {
                        "start_site" => "Website started",
                        "stop_site" => "Website stopped",
                        _ => "Website restarted",
                    },
                )?;
                Ok(json!(s.site(id)?))
            }
            "remove_site" => {
                let _guard = self.operations.lock().await;
                let _publish_guard = self
                    .publishing
                    .try_lock()
                    .map_err(|_| anyhow!("Wait for the current deployment to finish"))?;
                let id = arg(&p, "id")?;
                s.site(id)?;
                crate::build::refuse_busy(self, id)?;
                let delete = boolean(&p, "deleteFiles")?;
                if delete {
                    uuid::Uuid::parse_str(id).map_err(|_| anyhow!("Invalid website ID"))?;
                }
                {
                    let db = s.db()?;
                    let mut st = db.prepare(
                        "SELECT state,created_resources_json FROM deployments WHERE site_id=?1",
                    )?;
                    let rows = st.query_map([id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?;
                    for row in rows {
                        let (state, resources) = row?;
                        if !matches!(state.as_str(), "failed" | "rolled_back")
                            || hostify_core::db::resources_needed(&resources)
                        {
                            bail!("Unpublish this website before removing it")
                        }
                    }
                };
                self.runtime.stop_site(id).await.map_err(|e| anyhow!(e))?;
                {
                    let mut db = s.db()?;
                    let tx = db.transaction()?;
                    tx.execute("UPDATE domains SET site_id=NULL WHERE site_id=?1", [id])?;
                    tx.execute("DELETE FROM deployments WHERE site_id=?1", [id])?;
                    tx.execute(
                        "UPDATE managed_databases SET site_id=NULL WHERE site_id=?1",
                        [id],
                    )?;
                    tx.execute("DELETE FROM application_settings WHERE key LIKE 'mysql-env:%' AND json_extract(value,'$.site')=?1",[id])?;
                    tx.execute("DELETE FROM env_vars WHERE site_id=?1", [id])?;
                    tx.execute("DELETE FROM websites WHERE id=?1", [id])?;
                    tx.commit()?;
                }
                if delete {
                    std::fs::remove_dir_all(s.root.join("sites").join(id))?
                }
                s.activity("info", Some(id), "Website removed")?;
                Ok(Value::Null)
            }
            "get_settings" => Ok(json!(s.settings()?)),
            "save_settings" => {
                let settings: Settings = serde_json::from_value(p)?;
                if settings.base_port_start == 0
                    || !["system", "light", "dark"].contains(&settings.theme.as_str())
                {
                    bail!("Invalid settings")
                };
                #[cfg(windows)]
                {
                    let old = s.settings()?;
                    if old.launch_on_login != settings.launch_on_login {
                        crate::cli::task(settings.launch_on_login, &s.root)?;
                    }
                    if old.start_engine_on_boot != settings.start_engine_on_boot {
                        crate::cli::service_install(settings.start_engine_on_boot, &s.root)?;
                    }
                }
                s.save("settings", &json!(settings))?;
                Ok(json!(settings))
            }
            "cf_status" => Ok(s.saved("account")?.unwrap_or(json!({"connected":false}))),
            "cf_connect" => {
                let token = arg(&p, "token")?;
                let cf = self.logged_cf(token);
                cf.verify_token().await?;
                let accounts = cf.list_accounts().await?;
                let account = accounts
                    .first()
                    .ok_or_else(|| anyhow!("No accessible Cloudflare account"))?;
                self.secrets.set("cloudflare-token", token)?;
                let hint = token
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>();
                let a = json!({"connected":true,"accountId":account.id,"accountName":account.name,"tokenHint":hint});
                s.save("account", &a)?;
                Ok(a)
            }
            "cf_disconnect" => {
                self.secrets.delete("cloudflare-token")?;
                s.save("account", &json!({"connected":false}))?;
                Ok(Value::Null)
            }
            "cf_check_domain" => {
                let cf = self.cf()?;
                let z = cf.find_zone(hostname(&p)?).await?;
                let problem = cf
                    .check_dns_permission(&z.id)
                    .await
                    .err()
                    .map(|e| e.to_string());
                Ok(
                    json!({"zoneName":z.name,"zoneId":z.id,"canManageDns":problem.is_none(),"problems":problem.into_iter().collect::<Vec<_>>()}),
                )
            }
            "cf_plan_dns" => {
                let id = arg(&p, "siteId")?;
                s.site(id)?;
                let cf = self.cf()?;
                let h = hostname(&p)?;
                let z = cf.find_zone(h).await?;
                let tunnel = self
                    .live_resources()?
                    .iter()
                    .find(|r| r["request"]["siteId"] == id)
                    .and_then(|r| r["tunnel_id"].as_str())
                    .unwrap_or("")
                    .to_owned();
                let owned = self.deployer(cf.clone(), None).owned_tunnel_ids(arg(
                    &s.saved("account")?
                        .ok_or_else(|| anyhow!("Connect Cloudflare first"))?,
                    "accountId",
                )?)?;
                Ok(json!(
                    cf.plan_dns_with_owned_tunnels(&z.id, h, &tunnel, &owned)
                        .await?
                ))
            }
            "publish_site" => {
                let _guard = self.operations.lock().await;
                let permit =
                    self.publishing.clone().try_lock_owned().map_err(|_| {
                        anyhow!("A publish or unpublish operation is already running")
                    })?;
                let id = arg(&p, "siteId")?.to_owned();
                let site = s.site(&id)?;
                crate::build::refuse_busy(self, &id)?;
                let h = hostname(&p)?.to_owned();
                let consent = boolean(&p, "consentDns")?;
                if !consent {
                    bail!("DNS consent is required")
                };
                let existing:Option<(String,String)>=s.db()?.query_row("SELECT id,state FROM deployments WHERE site_id=?1 AND state NOT IN ('failed','rolled_back') ORDER BY rowid DESC LIMIT 1",[&id],|r|Ok((r.get(0)?,r.get(1)?))).ok();
                if let Some((dep, state)) = existing {
                    let host:Option<String>=s.db()?.query_row("SELECT json_extract(snapshot_json,'$.hostname') FROM deployments WHERE id=?1",[dep],|r|r.get(0))?;
                    bail!(
                        "{}. Choose Check again while waiting, or unpublish the website before publishing again.",
                        hostify_core::verify::state_message(&state, host.as_deref().unwrap_or(&h))
                    )
                };
                let statuses = s.detection.statuses(true).await;
                let missing = ["Caddy", "cloudflared"]
                    .into_iter()
                    .zip(statuses.iter())
                    .filter(|(_, s)| !s.ready)
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    bail!(
                        "Cannot publish: {} is not installed or can't run. Open the Dashboard and choose \"Install components\", then publish again.",
                        missing.join(" and ")
                    );
                }
                let cf = self.cf()?;
                let z = cf.find_zone(&h).await?;
                let account = s
                    .saved("account")?
                    .ok_or_else(|| anyhow!("Connect Cloudflare first"))?;
                let request = DeployRequest {
                    kind: site.kind.clone(),
                    zone_name: Some(z.name.clone()),
                    site_id: id.clone(),
                    hostname: h.clone(),
                    account_id: arg(&account, "accountId")?.into(),
                    zone_id: z.id.clone(),
                    tunnel_name: format!("hostify-{id}"),
                    port: site.local_port,
                    root: s.root.join("sites").join(&id).join("current"),
                    consent_dns: consent,
                };
                let d = self.deployer(cf, Some(&id));
                let store = s.clone();
                let (tx, rx) = tokio::sync::oneshot::channel();
                let (wake, control) = tokio::sync::watch::channel(VerifierControl::Retry);
                let task = tokio::spawn(async move {
                    let sender = Mutex::new(Some(tx));
                    let local_started = std::sync::atomic::AtomicBool::new(false);
                    let callback = |dep: &hostify_core::deploy::Deployment| {
                        store.deployment_update(dep);
                        if hostify_core::deploy::verification_state(&dep.state)
                            && !local_started.swap(true, std::sync::atomic::Ordering::SeqCst)
                        {
                            let _ = store.status(&dep.site_id, "running", None);
                        }
                        if let Ok(mut tx) = sender.lock() {
                            if let Some(tx) = tx.take() {
                                let _ = tx.send(json!(dep));
                            }
                        }
                    };
                    let result = d.deploy(request, &callback).await;
                    drop(permit); // Public checks never hold the engine's provisioning lock.
                    match result {
                        Ok(dep) if hostify_core::deploy::verification_state(&dep.state) => {
                            if let Err(e) = d.run_verifier(&dep.id, control, &callback).await {
                                let _ = store.activity("error", Some(&id), &e.to_string());
                            }
                        }
                        Ok(dep) => {
                            if dep.state == "failed" {
                                let _ = store.status(&id, "error", dep.error);
                            }
                        }
                        Err(e) => {
                            let _ = store.status(&id, "error", Some(e.to_string()));
                        }
                    }
                });
                let first: Value = rx.await?;
                let key = arg(&first, "id")?.to_owned();
                self.verifiers
                    .lock()
                    .map_err(|_| anyhow!("Verifier registry unavailable"))?
                    .insert(key, DeploymentTask { wake, task });
                Ok(first)
            }
            "retry_verification" => {
                let _guard = self.operations.lock().await;
                let id = arg(&p, "id")?;
                let site: String = s.db()?.query_row(
                    "SELECT site_id FROM deployments WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )?;
                self.start_verifier(id, &site, true)
            }
            "cancel_deployment" => {
                let _guard = self.operations.lock().await;
                let id = arg(&p, "id")?;
                self.stop_verifier(id).await?;
                let _permit = self.publishing.lock().await;
                let d = self.deployer(
                    self.logged_cf(self.secrets.get("cloudflare-token")?.unwrap_or_default()),
                    None,
                );
                Ok(json!(d.cancel(id, &|dep| s.deployment_update(dep)).await?))
            }

            "get_deployment" | "list_deployments" => {
                let db = s.db()?;
                let single = method == "get_deployment";
                let key = arg(&p, if single { "id" } else { "siteId" })?;
                let mut st=db.prepare(if single{"SELECT snapshot_json FROM deployments WHERE id=?1"}else{"SELECT snapshot_json FROM deployments WHERE site_id=?1 ORDER BY started_at DESC"})?;
                let rows = st.query_map([key], |r| r.get::<_, String>(0))?;
                let v = rows
                    .map(|r| Ok(serde_json::from_str::<Value>(&r?)?))
                    .collect::<Result<Vec<_>>>()?;
                if single {
                    v.into_iter()
                        .next()
                        .ok_or_else(|| anyhow!("Deployment not found"))
                } else {
                    Ok(json!(v))
                }
            }
            "unpublish_site" => {
                let _guard = self.operations.lock().await;
                let id = arg(&p, "siteId")?;
                s.site(id)?;
                let _permit = self
                    .publishing
                    .try_lock()
                    .map_err(|_| anyhow!("Wait for the current deployment to finish"))?;
                let rows = {
                    let db = s.db()?;
                    let mut st=db.prepare("SELECT id FROM deployments WHERE site_id=?1 AND state IN ('waiting_dns','waiting_tls','verifying_public','recovery_required','published')")?;
                    st.query_map([id], |r| r.get::<_, String>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                };
                for dep in &rows {
                    self.stop_verifier(dep).await?;
                }
                let d = self.deployer(self.cf()?, Some(id));
                for dep in rows {
                    d.unpublish(&dep, &|dep| s.deployment_update(dep)).await?;
                }
                s.db()?.execute("UPDATE domains SET site_id=NULL,dns_status='unconfigured',https_verified=0 WHERE site_id=?1",[id])?;
                Ok(Value::Null)
            }

            "list_domains" => {
                let db = s.db()?;
                let mut st=db.prepare("SELECT id,hostname,zone_name,zone_id,site_id,dns_status,https_verified FROM domains")?;
                let rows=st.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"hostname":r.get::<_,String>(1)?,"zoneName":r.get::<_,String>(2)?,"zoneId":r.get::<_,String>(3)?,"siteId":r.get::<_,Option<String>>(4)?,"dnsStatus":r.get::<_,String>(5)?,"httpsVerified":r.get::<_,bool>(6)?})))?;
                Ok(json!(rows.collect::<rusqlite::Result<Vec<_>>>()?))
            }
            "tunnel_health" => {
                let mut v = Vec::new();
                for r in self.live_resources()? {
                    if let Some(id) = r["tunnel_id"].as_str() {
                        let port = crate::supervisor::metrics_port(id);
                        let connected = crate::supervisor::http_ok(port, "/ready").await;
                        let connections = crate::supervisor::tunnel_connections(port).await;
                        let last_change = {
                            let mut observations = self
                                .tunnel_observations
                                .lock()
                                .map_err(|_| anyhow!("Tunnel status unavailable"))?;
                            let observation = observations
                                .entry(id.into())
                                .or_insert_with(|| (connected, Utc::now().to_rfc3339()));
                            if observation.0 != connected {
                                *observation = (connected, Utc::now().to_rfc3339());
                            }
                            observation.1.clone()
                        };
                        v.push(json!({"tunnelId":id,"name":r["request"]["tunnelName"],"connected":connected,"connections":connections,"lastChange":last_change}));
                    }
                }
                Ok(json!(v))
            }
            "list_activity" => {
                let limit = number(&p, "limit", 1000)?;
                let site = p.get("siteId").and_then(Value::as_str);
                let db = s.db()?;
                let mut st=db.prepare("SELECT id,ts,level,site_id,message FROM activity_logs WHERE (?1 IS NULL OR site_id=?1) ORDER BY id DESC LIMIT ?2")?;
                let rows=st.query_map(params![site,limit],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"ts":r.get::<_,String>(1)?,"level":r.get::<_,String>(2)?,"siteId":r.get::<_,Option<String>>(3)?,"message":r.get::<_,String>(4)?})))?;
                Ok(json!(rows.collect::<rusqlite::Result<Vec<_>>>()?))
            }
            "site_logs" => {
                let id = arg(&p, "siteId")?;
                s.site(id)?;
                let lines = number(&p, "lines", 10000)? as usize;
                Ok(json!(crate::supervisor::tail_logs(
                    &s.root.join("logs").join(format!("{id}.log")),
                    lines
                )?))
            }
            "pick_path" => {
                kind(&p)?;
                bail!("pick_path must be handled by the desktop shell's native dialog")
            }
            _ => bail!("Unknown method: {method}"),
        }
    }
}
fn arg<'a>(p: &'a Value, k: &str) -> Result<&'a str> {
    p.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("Missing or invalid {k}"))
}
fn boolean(p: &Value, k: &str) -> Result<bool> {
    p.get(k)
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow!("Missing or invalid {k}"))
}
fn number(p: &Value, k: &str, max: u64) -> Result<u64> {
    p.get(k)
        .and_then(Value::as_u64)
        .filter(|n| *n <= max)
        .ok_or_else(|| anyhow!("{k} must be an integer from 0 to {max}"))
}
fn kind(p: &Value) -> Result<&str> {
    let k = arg(p, "kind")?;
    if !["directory", "zip"].contains(&k) {
        bail!("kind must be directory or zip")
    };
    Ok(k)
}
fn hostname(p: &Value) -> Result<&str> {
    let h = arg(p, "hostname")?;
    if h.len() > 253
        || !h.contains('.')
        || h.split('.').any(|l| {
            l.is_empty()
                || l.len() > 63
                || l.starts_with('-')
                || l.ends_with('-')
                || !l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        bail!("Enter a valid DNS hostname")
    };
    Ok(h)
}
fn directory_bytes(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => directory_bytes(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod publishing_tests {
    use super::*;
    use async_trait::async_trait;
    use hostify_core::{
        deploy::{Deployment, DeploymentStep},
        verify::{HttpEvidence, PublicProbe, Verification, VerifyClass},
    };
    use std::{
        net::IpAddr,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };
    #[derive(Default)]
    pub(super) struct MemoryStore(Mutex<HashMap<String, String>>);
    impl SecretStore for MemoryStore {
        fn set(&self, key: &str, value: &str) -> Result<(), hostify_core::secrets::SecretError> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        fn get(&self, key: &str) -> Result<Option<String>, hostify_core::secrets::SecretError> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn delete(&self, key: &str) -> Result<(), hostify_core::secrets::SecretError> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }
    #[derive(Default)]
    pub(super) struct Runtime {
        pub(super) starts: Mutex<Vec<String>>,
        stops: AtomicUsize,
    }
    #[async_trait]
    impl SiteRuntime for Runtime {
        async fn start_site(&self, id: &str) -> Result<bool, String> {
            self.starts.lock().unwrap().push(id.into());
            Ok(true)
        }
        async fn stop_site(&self, _: &str) -> Result<(), String> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn start_tunnel(&self, _: &str) -> Result<bool, String> {
            self.starts.lock().unwrap().push("connector".into());
            Ok(true)
        }
        async fn stop_tunnel(&self) -> Result<(), String> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn site_responds(&self, _: u16) -> Result<bool, String> {
            Ok(true)
        }
    }
    struct Evidence {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl PublicProbe for Evidence {
        async fn zone(&self, _: &str) -> Result<hostify_core::cloudflare::Zone, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(hostify_core::cloudflare::Zone {
                id: "z".into(),
                name: "example.com".into(),
                status: "pending".into(),
                name_servers: vec!["ns.example".into()],
            })
        }
        async fn record(&self, _: &str, _: &str) -> Result<Option<Value>, String> {
            panic!("inactive zone stops the attempt")
        }
        async fn connector(&self, _: &str) -> Result<(bool, u64), String> {
            panic!("inactive zone")
        }
        async fn authoritative(&self, _: &str, _: &str) -> Result<Vec<IpAddr>, String> {
            panic!("inactive zone")
        }
        async fn recursive(&self, _: &str, _: &str) -> Result<Vec<IpAddr>, String> {
            panic!("inactive zone")
        }
        async fn system(&self, _: &str) -> Result<Vec<IpAddr>, String> {
            panic!("inactive zone")
        }
        async fn https(&self, _: &str, _: IpAddr, _: &str) -> Result<HttpEvidence, VerifyClass> {
            panic!("inactive zone")
        }
    }
    pub(super) fn seed(store: &Store, id: &str, state: &str, auto: bool) {
        let root = store.root.join("sites").join(id).join("current");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.html"), "site").unwrap();
        let port = if id == "s" { 9000 } else { 9001 };
        store.db().unwrap().execute("INSERT INTO websites(id,name,source_path,local_port,auto_start,created_at,updated_at) VALUES(?1,?1,'source',?2,?3,'now','now')",params![id,port,auto]).unwrap();
        let q = DeployRequest {
            kind: "static".into(),
            zone_name: Some("example.com".into()),
            site_id: id.into(),
            hostname: format!("{id}.example.com"),
            zone_id: "z".into(),
            account_id: "a".into(),
            tunnel_name: format!("hostify-{id}"),
            port,
            root,
            consent_dns: true,
        };
        let d = Deployment {
            id: format!("dep-{id}"),
            site_id: id.into(),
            hostname: Some(q.hostname.clone()),
            state: state.into(),
            steps: (0..7)
                .map(|_| DeploymentStep {
                    key: "step".into(),
                    label: "step".into(),
                    state: "done".into(),
                    detail: None,
                })
                .collect(),
            verification: if hostify_core::deploy::verification_state(state) {
                Some(Verification::new(
                    Utc::now() + chrono::Duration::seconds(1800),
                ))
            } else {
                None
            },
            error: None,
            started_at: "now".into(),
            finished_at: None,
        };
        let r = if hostify_core::deploy::verification_state(state) {
            json!({"request":q,"tunnel_id":"t","tunnel_created":true,"dns_id":"dns","dns_intent":true,"owner":d.id,"nonce":"0123456789abcdef0123456789abcdef","marker_directory":store.root.join("run/verify").join(id)})
        } else {
            json!({"request":q,"site_started":true,"owner":d.id})
        };
        store.db().unwrap().execute("INSERT INTO deployments(id,site_id,state,snapshot_json,created_resources_json,started_at) VALUES(?1,?2,?3,?4,?5,'now')",params![d.id,id,state,json!(d).to_string(),r.to_string()]).unwrap();
    }
    async fn observed(evidence: &Evidence, minimum: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while evidence.calls.load(Ordering::SeqCst) < minimum {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn startup_resumes_waiting_without_rollback_and_retry_wakes_one_task() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("data")).unwrap();
        seed(&store, "s", "waiting_dns", false);
        seed(&store, "p", "validating", false);
        let runtime = Arc::new(Runtime::default());
        let evidence = Arc::new(Evidence {
            calls: AtomicUsize::new(0),
        });
        let mut engine = Engine::new(store.clone(), runtime.clone(), false);
        let secrets = Arc::new(MemoryStore::default());
        secrets.set("cloudflare-token", "offline-test").unwrap();
        secrets.set("tunnel:t", "connector-token").unwrap();
        let inner = Arc::get_mut(&mut engine).unwrap();
        inner.secrets = secrets;
        inner.public_probe = Some(evidence.clone());
        engine.startup().await.unwrap();
        observed(&evidence, 1).await;
        assert!(runtime.starts.lock().unwrap().contains(&"s".into()));
        assert!(runtime.starts.lock().unwrap().contains(&"connector".into()));
        assert_eq!(
            runtime.stops.load(Ordering::SeqCst),
            1,
            "only interrupted provisioning was stopped"
        );
        let state: String = store
            .db()
            .unwrap()
            .query_row("SELECT state FROM deployments WHERE id='dep-p'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(state, "rolled_back");
        let task_id = engine.verifiers.lock().unwrap()["dep-s"].task.id();
        assert!(engine.operations.try_lock().is_ok());
        assert!(engine.publishing.try_lock().is_ok());
        let before = evidence.calls.load(Ordering::SeqCst);
        for _ in 0..2 {
            engine
                .dispatch("retry_verification", json!({"id":"dep-s"}))
                .await
                .unwrap();
        }
        observed(&evidence, before + 1).await;
        {
            let tasks = engine.verifiers.lock().unwrap();
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks["dep-s"].task.id(), task_id);
        }
        let site = store.site("s").unwrap();
        assert_eq!(site.publish_state.as_deref(), Some("waiting_dns"));
        assert_eq!(site.publish_deployment_id.as_deref(), Some("dep-s"));
        assert_eq!(site.publish_hostname.as_deref(), Some("s.example.com"));
        assert!(site.public_url.is_none());
        let error = engine
            .dispatch(
                "publish_site",
                json!({"siteId":"s","hostname":"s.example.com","consentDns":true}),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Check again"));
        assert!(error.contains("s.example.com"));
        assert!(!error.contains("dep-s"));
        engine
            .dispatch("stop_site", json!({"id":"s"}))
            .await
            .unwrap();
        assert_eq!(store.site("s").unwrap().status, "stopped");
        engine
            .dispatch("retry_verification", json!({"id":"dep-s"}))
            .await
            .unwrap();
        observed(&evidence, before + 2).await;
        assert_eq!(
            store.site("s").unwrap().status,
            "stopped",
            "verifier cannot override local server status"
        );
        let stops = runtime.stops.load(Ordering::SeqCst);
        let cancelled = engine
            .dispatch("cancel_deployment", json!({"id":"dep-s"}))
            .await
            .unwrap();
        assert_eq!(cancelled["state"], "recovery_required");
        assert!(engine.verifiers.lock().unwrap().is_empty());
        assert_eq!(runtime.stops.load(Ordering::SeqCst), stops);
        let error = engine
            .dispatch("remove_site", json!({"id":"s","deleteFiles":false}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Unpublish"));
        assert_eq!(engine.live_resources().unwrap().len(), 1);
        engine.shutdown().await;
    }
    #[tokio::test]
    async fn cancelling_provisioning_aborts_task_then_rolls_back_its_local_start() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("data")).unwrap();
        seed(&store, "s", "starting_local", false);
        let runtime = Arc::new(Runtime::default());
        let mut engine = Engine::new(store.clone(), runtime.clone(), false);
        let secrets = Arc::new(MemoryStore::default());
        secrets.set("cloudflare-token", "offline-test").unwrap();
        Arc::get_mut(&mut engine).unwrap().secrets = secrets;
        let (wake, _) = tokio::sync::watch::channel(VerifierControl::Retry);
        let task = tokio::spawn(std::future::pending());
        engine
            .verifiers
            .lock()
            .unwrap()
            .insert("dep-s".into(), DeploymentTask { wake, task });
        let result = engine
            .dispatch("cancel_deployment", json!({"id":"dep-s"}))
            .await
            .unwrap();
        assert_eq!(result["state"], "rolled_back");
        assert_eq!(runtime.stops.load(Ordering::SeqCst), 1);
        assert!(engine.verifiers.lock().unwrap().is_empty());
    }
}

struct Stats {
    system: sysinfo::System,
    cpu_at: std::time::Instant,
    disk: Option<(std::time::Instant, u64, u64, u64)>,
}
impl Stats {
    fn new() -> Self {
        let mut system = sysinfo::System::new();
        system.refresh_cpu_usage();
        Self {
            system,
            cpu_at: std::time::Instant::now(),
            disk: None,
        }
    }
    fn read(&mut self, root: &Path) -> Result<Value> {
        let elapsed = self.cpu_at.elapsed();
        if elapsed < sysinfo::MINIMUM_CPU_UPDATE_INTERVAL {
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL - elapsed);
        }
        self.system.refresh_cpu_usage();
        self.cpu_at = std::time::Instant::now();
        self.system.refresh_memory();
        if self
            .disk
            .as_ref()
            .is_none_or(|(at, _, _, _)| at.elapsed() >= std::time::Duration::from_secs(60))
        {
            let disks = sysinfo::Disks::new_with_refreshed_list();
            let disk = disks
                .list()
                .iter()
                .filter(|d| root.starts_with(d.mount_point()))
                .max_by_key(|d| d.mount_point().as_os_str().len());
            self.disk = Some((
                std::time::Instant::now(),
                disk.map(|d| d.total_space().saturating_sub(d.available_space()))
                    .unwrap_or(0),
                disk.map(|d| d.total_space()).unwrap_or(0),
                directory_bytes(root),
            ));
        }
        let (_, used, total, data) = self
            .disk
            .as_ref()
            .ok_or_else(|| anyhow!("Disk statistics unavailable"))?;
        Ok(
            json!({"cpuPercent":self.system.global_cpu_usage(),"ramUsedBytes":self.system.used_memory(),"ramTotalBytes":self.system.total_memory(),"diskUsedBytes":used,"diskTotalBytes":total,"hostifyDataBytes":data}),
        )
    }
}

#[cfg(test)]
mod audit_tests {
    use super::*;
    #[tokio::test]
    async fn startup_without_account_preserves_waiting_and_starts_other_sites() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("data")).unwrap();
        super::publishing_tests::seed(&store, "s", "waiting_dns", false);
        super::publishing_tests::seed(&store, "p", "waiting_dns", true);
        let runtime = Arc::new(super::publishing_tests::Runtime::default());
        let mut engine = Engine::new(store.clone(), runtime.clone(), false);
        Arc::get_mut(&mut engine).unwrap().secrets =
            Arc::new(super::publishing_tests::MemoryStore::default());
        let before: String = store
            .db()
            .unwrap()
            .query_row(
                "SELECT snapshot_json FROM deployments WHERE id='dep-s'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        engine.startup().await.unwrap();
        assert_eq!(
            store
                .db()
                .unwrap()
                .query_row(
                    "SELECT snapshot_json FROM deployments WHERE id='dep-s'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            before
        );
        let logs = engine
            .dispatch("list_activity", json!({"limit":100}))
            .await
            .unwrap()
            .to_string();
        assert!(logs.contains("Connect your Cloudflare account"));
        assert!(runtime.starts.lock().unwrap().contains(&"p".into()));
        engine.shutdown().await;
    }
    #[test]
    fn stats_cache_disk_and_prime_cpu() {
        let dir = tempfile::tempdir().unwrap();
        let mut stats = Stats::new();
        let start = stats.cpu_at;
        let first = stats.read(dir.path()).unwrap();
        assert!(start.elapsed() >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        assert!(first["ramTotalBytes"].as_u64().unwrap() > 0);
        std::fs::write(dir.path().join("file"), "new").unwrap();
        assert_eq!(
            stats.read(dir.path()).unwrap()["hostifyDataBytes"],
            first["hostifyDataBytes"]
        );
        stats.disk.as_mut().unwrap().0 =
            std::time::Instant::now() - std::time::Duration::from_secs(61);
        assert_eq!(stats.read(dir.path()).unwrap()["hostifyDataBytes"], 3);
    }
}

#[cfg(test)]
mod path_safety_tests {
    use super::*;
    #[tokio::test]
    async fn remove_rejects_non_uuid_before_deleting_row_or_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("data")).unwrap();
        let id = "../../outside";
        store.db().unwrap().execute("INSERT INTO websites(id,name,source_path,local_port,created_at,updated_at) VALUES(?1,'unsafe','source',9000,'now','now')",[id]).unwrap();
        let engine = Engine::new(
            store.clone(),
            Arc::new(super::publishing_tests::Runtime::default()),
            false,
        );
        assert!(
            engine
                .dispatch("remove_site", json!({"id":id,"deleteFiles":true}))
                .await
                .unwrap_err()
                .to_string()
                .contains("Invalid website ID")
        );
        assert!(store.site(id).is_ok());
    }
    #[test]
    fn unpublished_site_does_not_show_an_older_failed_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("data")).unwrap();
        super::publishing_tests::seed(&store, "s", "failed", false);
        assert_eq!(
            store.site("s").unwrap().publish_state.as_deref(),
            Some("failed")
        );
        store
            .db()
            .unwrap()
            .execute(
                "INSERT INTO deployments(id,site_id,state,snapshot_json,started_at) VALUES('new','s','rolled_back','{}','now')",
                [],
            )
            .unwrap();
        assert_eq!(store.site("s").unwrap().publish_state, None);
    }
    #[tokio::test]
    async fn unpublish_during_provisioning_leaves_rows_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("data")).unwrap();
        super::publishing_tests::seed(&store, "s", "starting_connector", false);
        let engine = Engine::new(
            store.clone(),
            Arc::new(super::publishing_tests::Runtime::default()),
            false,
        );
        let _busy = engine.publishing.lock().await;
        assert!(
            engine
                .dispatch("unpublish_site", json!({"siteId":"s"}))
                .await
                .unwrap_err()
                .to_string()
                .contains("Wait")
        );
        assert_eq!(
            store
                .db()
                .unwrap()
                .query_row("SELECT state FROM deployments", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "starting_connector"
        );
    }
}

#[cfg(test)]
mod cleanup_safety_tests {
    use super::*;
    #[tokio::test]
    async fn remove_keeps_failed_cleanup_and_malformed_resource_journals() {
        for resources in [r#"{"rollback_pending":true}"#, "invalid"] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(dir.path().join("data")).unwrap();
            super::publishing_tests::seed(&store, "s", "failed", false);
            store
                .db()
                .unwrap()
                .execute(
                    "UPDATE deployments SET created_resources_json=?1",
                    [resources],
                )
                .unwrap();
            let engine = Engine::new(
                store.clone(),
                Arc::new(super::publishing_tests::Runtime::default()),
                false,
            );
            assert!(
                engine
                    .dispatch("remove_site", json!({"id":"s","deleteFiles":false}))
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("Unpublish")
            );
            assert!(store.site("s").is_ok());
        }
    }
}

fn remember_tunnel_tokens(cf: &CloudflareClient, store: &Store, secrets: &dyn SecretStore) {
    let records = (|| -> Result<Vec<String>> {
        let db = store.db()?;
        let mut st = db.prepare("SELECT created_resources_json FROM deployments")?;
        Ok(st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    })();
    if let Ok(records) = records {
        for raw in records {
            if let Ok(r) = serde_json::from_str::<Value>(&raw) {
                if let Some(id) = r["tunnel_id"].as_str() {
                    if let Ok(Some(token)) = secrets.get(&format!("tunnel:{id}")) {
                        cf.remember_secret(token);
                    }
                }
            }
        }
    }
}

pub fn aggregate_health<'a>(
    processes: impl Iterator<Item = &'a (String, Option<String>)>,
) -> (String, Option<String>) {
    let values = processes.collect::<Vec<_>>();
    for state in ["error", "starting", "stopped"] {
        if let Some(v) = values.iter().find(|v| v.0 == state) {
            return (*v).clone();
        }
    }
    if values.is_empty() {
        ("stopped".into(), None)
    } else {
        ("running".into(), None)
    }
}
#[cfg(test)]
mod aggregation_tests {
    use super::*;
    #[test]
    fn failed_app_is_not_hidden_by_caddy() {
        let p = [
            ("running".into(), None),
            ("error".into(), Some("The application crashed".into())),
        ];
        assert_eq!(aggregate_health(p.iter()), p[1]);
        assert_eq!(aggregate_health(p[..1].iter()), p[0]);
    }
}

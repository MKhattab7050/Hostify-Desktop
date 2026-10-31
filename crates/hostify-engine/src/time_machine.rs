//! Local code snapshots: integrity-checked, never silently restoring database or secrets.
use crate::state::Engine;
use anyhow::{Result, anyhow, bail};
use chrono::Utc;
use serde_json::{json, Value};
use std::{path::{Path,PathBuf},sync::Arc};
use hostify_core::{import, snapshots};

fn snapshot_directory(root: &Path, id: &str) -> Result<PathBuf> {
    Ok(snapshots::directory(&root.join("backups/site-snapshots"), id)?)
}
fn backup_root(root: &Path) -> PathBuf { root.join("backups/site-snapshots") }

pub async fn create(engine: &Arc<Engine>, id: &str) -> Result<Value> {
    let _guard = engine.operations.lock().await;
    let _publishing = engine.publishing.try_lock()
        .map_err(|_| anyhow!("Wait for the current deployment to finish before backing up."))?;
    crate::build::refuse_busy(engine, id)?;
    let site = engine.store.site(id)?;
    let site_dir = engine.store.root.join("sites").join(id);
    let (source, release) = if site.kind == "static" {
        (site_dir.join("current"), None)
    } else {
        let config = engine.store.app_config(id)?;
        let release = config.active_release
            .or(config.built_release)
            .ok_or_else(|| anyhow!("No active or successfully built release is available to snapshot."))?;
        (site_dir.join("app").join(&release), Some(release))
    };
    let base = backup_root(&engine.store.root);
    let dir = snapshot_directory(&engine.store.root,id)?;
    std::fs::create_dir_all(&dir)?;
    #[cfg(windows)]
    crate::mysql::restrict(&dir, true)?;
    let kind = site.kind.clone();
    let owned = id.to_owned();
    let record = tokio::task::spawn_blocking(move ||
        snapshots::create(&source, &base, &owned, &kind, release.as_deref())
    ).await??;
    engine.store.activity("info",Some(id),
        "Time Machine: checksum-protected website files snapshot created. MySQL and credentials are NOT included.")?;
    Ok(json!(record))
}

pub async fn list(engine: &Arc<Engine>, id: &str) -> Result<Value> {
    engine.store.site(id)?;
    let dir=backup_root(&engine.store.root);
    let id=id.to_owned();
    Ok(json!(tokio::task::spawn_blocking(move ||
        snapshots::list(&dir,&id)).await??))
}

pub async fn verify(engine: &Arc<Engine>, id: &str, snapshot_id: &str) -> Result<Value> {
    engine.store.site(id)?;
    let base=backup_root(&engine.store.root);
    let site=id.to_owned();let sid=snapshot_id.to_owned();
    let record=tokio::task::spawn_blocking(move ||
        snapshots::read(&base,&site,&sid)).await??;
    Ok(json!({"verified":true,"snapshot":record}))
}

/// A single-purpose filesystem operation for an explicit user-selected
/// export. Does not replace an existing file or automatically upload content.
fn copy_verified_archive(
    base:&Path, root:&Path, site_id:&str, record_id:&str, dest:&Path,
)->Result<Value>{
    if !dest.is_absolute() || dest.extension().and_then(|s|s.to_str())!=Some("zip"){
        bail!("Choose an absolute .zip destination using the native Save dialog.");
    }
    let record=snapshots::read(base,site_id,record_id)?;
    let parent=dest.parent().ok_or_else(||anyhow!("Invalid export destination."))?;
    if std::fs::canonicalize(parent)?.starts_with(std::fs::canonicalize(root)?) {
        bail!("Export outside the Hostify managed data directory.");
    }
    let archive=snapshots::archive(base,site_id,record_id)?;
    let mut from=std::fs::File::open(archive)?;
    // create_new prevents a rename or disk backup operation from overwriting
    // existing files, including existing symlinks, at the chosen destination.
    let mut to=std::fs::OpenOptions::new().write(true).create_new(true).open(dest)?;
    let written=std::io::copy(&mut from,&mut to)
        .and_then(|_|to.sync_all());
    // Close before cleanup on Windows: open file handles prevent deletion.
    drop(to);
    if let Err(error)=written {
        let _=std::fs::remove_file(dest);
        return Err(error.into());
    }
    if snapshots::checksum(dest)? != record.sha256 {
        let _=std::fs::remove_file(dest);
        bail!("Export integrity validation failed.");
    }
    Ok(json!({"exported":true,"archiveBytes":record.archive_bytes,
        "sha256":record.sha256,"mayContainSecrets":record.may_contain_secrets}))
}
/// Copy a chosen snapshot outside Hostify without overwriting existing files.
/// The destination is selected by a native Save dialog and the bytes are
/// checked against the immutable manifest before the export is accepted.
pub async fn export(engine:&Arc<Engine>,id:&str,snapshot_id:&str,destination:&str)->Result<Value>{
    let _guard=engine.operations.lock().await;
    let _publishing=engine.publishing.try_lock()
        .map_err(|_|anyhow!("Wait for the current deployment to finish before exporting."))?;
    engine.store.site(id)?;
    let root=engine.store.root.clone();
    let base=backup_root(&root);
    let site_id=id.to_owned();
    let record_id=snapshot_id.to_owned();
    let dest=PathBuf::from(destination);
    let result=tokio::task::spawn_blocking(move ||
        copy_verified_archive(&base,&root,&site_id,&record_id,&dest)).await??;
    engine.store.activity("info",Some(id),
        "Time Machine: a verified website snapshot was exported to a user-selected destination.")?;
    Ok(result)
}

pub async fn restore(engine: &Arc<Engine>, id: &str, snapshot_id: &str, confirm_name: &str) -> Result<Value> {
    let _guard=engine.operations.lock().await;
    let _publishing=engine.publishing.try_lock()
        .map_err(|_| anyhow!("Wait for the current deployment to finish before restoring."))?;
    crate::build::refuse_busy(engine,id)?;
    let site=engine.store.site(id)?;
    if site.name != confirm_name { bail!("Type the exact website name to authorize the restore."); }
    if site.status != "stopped" { bail!("Stop this website before restoring its files."); }
    let base=backup_root(&engine.store.root);
    let owned=id.to_owned();let snapshot_id=snapshot_id.to_owned();
    let (record,archive)=tokio::task::spawn_blocking(move || -> Result<_> {
        let record=snapshots::read(&base,&owned,&snapshot_id)?;
        let archive=snapshots::archive(&base,&owned,&snapshot_id)?;
        Ok((record,archive))
    }).await??;
    if site.kind != record.site_kind { bail!("Snapshot project type does not match this website."); }
    let site_dir=engine.store.root.join("sites").join(id);
    if site.kind == "static" {
        let archive_clone=archive.clone();
        let dest=site_dir.clone();
        let report=tokio::task::spawn_blocking(move ||
            import::import(&archive_clone,"zip",&dest)).await??;
        engine.store.db()?.execute(
            "UPDATE websites SET file_count=?1,size_bytes=?2,updated_at=?3 WHERE id=?4",
            rusqlite::params![report.file_count,report.size_bytes,Utc::now().to_rfc3339(),id]
        )?;
        engine.store.activity("info",Some(id),
            "Time Machine: static site file snapshot restored while stopped; start it explicitly.")?;
        Ok(json!({"restored":true,"requiresBuild":false,"includesDatabase":false}))
    } else {
        let mut config=engine.store.app_config(id)?;
        if config.pending_release.as_ref().is_some_and(|pending|
            config.active_release.as_ref() != Some(pending)) {
            bail!("Resolve the existing pending release before restoring another snapshot.");
        }
        let dest=site_dir.clone();
        let (_report,new_release)=tokio::task::spawn_blocking(move ||
            import::import_release(&archive,"zip",&dest)).await??;
        // Never overwrite the trusted, built active release. The restored copy
        // is staged as an untrusted pending release until explicit approval.
        config.pending_release=Some(new_release.clone());
        engine.store.save_app_config(id,&config)?;
        engine.store.activity("info",Some(id),
            "Time Machine: application snapshot staged as pending release. Trust and rebuild before activating it.")?;
        Ok(json!({"restored":true,"requiresBuild":true,"pendingRelease":new_release,
            "includesDatabase":false}))
    }
}


#[derive(Clone,Debug,serde::Serialize,serde::Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct Schedule {
    pub enabled: bool,
    pub every_hours: u32,
    pub last_run_unix: Option<i64>,
    pub last_attempt_unix: Option<i64>,
    pub paused_for_capacity: bool,
}
impl Default for Schedule {
    fn default() -> Self {
        Self { enabled:false,every_hours:24,last_run_unix:None,
            last_attempt_unix:None,paused_for_capacity:false }
    }
}
fn schedule_key(id:&str)->String { format!("snapshot-schedule:{id}") }
pub fn schedule(engine:&Engine,id:&str)->Result<Schedule>{
    engine.store.site(id)?;
    Ok(match engine.store.saved(&schedule_key(id))? {
        Some(v)=>serde_json::from_value(v)?, None=>Schedule::default()
    })
}
pub fn configure_schedule(engine:&Engine,id:&str,enabled:bool,hours:u32)->Result<Value>{
    engine.store.site(id)?;
    if ![24,72,168].contains(&hours) { bail!("Choose 24, 72 or 168 hours."); }
    let mut state=schedule(engine,id)?;
    state.enabled=enabled;
    state.every_hours=hours;
    state.paused_for_capacity=false;
    if enabled && state.last_attempt_unix.is_none() {
        // Enabling does not silently take a potentially large first backup.
        // The owner can create one immediately using Create Snapshot.
        state.last_attempt_unix=Some(Utc::now().timestamp());
    }
    engine.store.save(&schedule_key(id),&json!(state))?;
    engine.store.activity("info",Some(id),if enabled{
        "Time Machine: periodic local file snapshots enabled. Database and off-site copies remain separate."
    }else{"Time Machine: periodic file snapshots disabled."})?;
    Ok(json!(state))
}
fn scheduled_due(s:&Schedule,now:i64,snapshot_count:usize)->bool{
    if !s.enabled || s.paused_for_capacity || snapshot_count>=5{return false;}
    let interval=(s.every_hours as i64).saturating_mul(3600);
    let last=s.last_run_unix.unwrap_or(0).max(s.last_attempt_unix.unwrap_or(0));
    now.saturating_sub(last)>=interval
}
pub fn spawn(engine:&Arc<Engine>)->tokio::task::JoinHandle<()>{
    let weak=Arc::downgrade(engine);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30*60)).await;
            let Some(engine)=weak.upgrade() else {return};
            let Ok(sites)=engine.store.sites() else {continue};
            for site in sites {
                let Ok(mut schedule)=schedule(&engine,&site.id) else {continue};
                if !schedule.enabled {continue;}
                let Ok(records)=list(&engine,&site.id).await else {continue};
                if records.as_array().is_some_and(|a|a.len()>=5) {
                    if !schedule.paused_for_capacity {
                        schedule.paused_for_capacity=true;
                        let _=engine.store.save(&schedule_key(&site.id),&json!(schedule));
                        let _=engine.store.activity("warn",Some(&site.id),
                            "Time Machine: five retained snapshots reached; scheduling paused. Export and manage backups manually.");
                    }
                    continue;
                }
                if !scheduled_due(&schedule,Utc::now().timestamp(),
                    records.as_array().map_or(0,Vec::len)){continue;}
                // At most one attempt per schedule interval, including failure;
                // never delete old archives automatically.
                schedule.last_attempt_unix=Some(Utc::now().timestamp());
                if engine.store.save(&schedule_key(&site.id),&json!(schedule)).is_err(){continue;}
                match create(&engine,&site.id).await {
                    Ok(_)=> {
                        schedule.last_run_unix=Some(Utc::now().timestamp());
                        let _=engine.store.save(&schedule_key(&site.id),&json!(schedule));
                    }
                    Err(_)=> {
                        let _=engine.store.activity("warn",Some(&site.id),
                            "Time Machine: scheduled snapshot postponed due to local resource or deployment issue.");
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_exports_have_verified_digest_and_never_overwrite() {
        let dir=tempfile::tempdir().unwrap();
        let root=dir.path().join("hostify");
        let src=dir.path().join("source");
        let export_dir=dir.path().join("usb-export");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&export_dir).unwrap();
        std::fs::write(src.join("index.html"),"<h1>Hello</h1>").unwrap();
        let id=uuid::Uuid::new_v4().to_string();
        let base=backup_root(&root);
        let snapshot=snapshots::create(&src,&base,&id,"static",None).unwrap();
        let dest=export_dir.join("website.zip");
        let outcome=copy_verified_archive(&base,&root,&id,&snapshot.id,&dest).unwrap();
        assert_eq!(outcome["sha256"],snapshot.sha256);
        let original=std::fs::read(&dest).unwrap();
        assert!(copy_verified_archive(&base,&root,&id,&snapshot.id,&dest).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(),original);
        assert!(copy_verified_archive(&base,&root,&id,&snapshot.id,&root.join("backup.zip")).is_err());
        assert!(!root.join("backup.zip").exists());
    }
    #[test]
    fn scheduling_is_opt_in_bounded_and_capacity_limited() {
        let mut s=Schedule::default();
        assert!(!scheduled_due(&s,500_000,0));
        s.enabled=true;s.last_attempt_unix=Some(1000);
        assert!(!scheduled_due(&s,1000+23*3600,0));
        assert!(scheduled_due(&s,1000+24*3600,0));
        assert!(!scheduled_due(&s,1000+48*3600,5));
        s.paused_for_capacity=true;
        assert!(!scheduled_due(&s,1000+168*3600,1));
    }

    #[test]
    fn invalid_identifiers_cannot_escape_site_backup_directory() {
        let base=std::env::temp_dir();
        assert!(snapshot_directory(&base,"../other").is_err());
        assert!(snapshots::archive(&base,&uuid::Uuid::new_v4().to_string(),"../../x").is_err());
    }
}

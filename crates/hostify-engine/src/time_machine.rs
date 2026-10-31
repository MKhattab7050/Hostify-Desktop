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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_identifiers_cannot_escape_site_backup_directory() {
        let base=std::env::temp_dir();
        assert!(snapshot_directory(&base,"../other").is_err());
        assert!(snapshots::archive(&base,&uuid::Uuid::new_v4().to_string(),"../../x").is_err());
    }
}

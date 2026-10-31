//! Hostify Guardian: explicitly opted-in, bounded, non-destructive local recovery.
//! Never writes Cloudflare DNS, application source, schema, or runtime selection.
use crate::state::Engine;
use anyhow::{Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{sync::{Arc, Weak}, time::Duration};

const CHECK_EVERY: u64 = 30;
const FAILURE_THRESHOLD: u32 = 3;
const RESTART_COOLDOWN: i64 = 15 * 60;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GuardianSnapshot {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub recovery_attempts: u64,
    #[serde(default)]
    pub last_restart_unix: Option<i64>,
    #[serde(default)]
    pub last_checked_at: Option<String>,
    #[serde(default)]
    pub last_result: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Decision { Skip, Observe, Cooldown, Recover }

/// Restrict automatic intervention to opted-in, formerly-running services.
/// A manually stopped or disabled site must never be restarted by Guardian.
fn decision(s: &GuardianSnapshot, site_status: &str, auto_start: bool, now: i64) -> Decision {
    if !s.enabled || !auto_start || site_status != "running" { return Decision::Skip; }
    if s.consecutive_failures < FAILURE_THRESHOLD { return Decision::Observe; }
    if s.last_restart_unix.is_some_and(|previous| now.saturating_sub(previous) < RESTART_COOLDOWN) {
        return Decision::Cooldown;
    }
    Decision::Recover
}

fn key(id: &str) -> String { format!("guardian:{id}") }

pub fn load(engine: &Engine, id: &str) -> Result<GuardianSnapshot> {
    engine.store.site(id)?;
    Ok(match engine.store.saved(&key(id))? {
        Some(v) => serde_json::from_value(v)?,
        None => GuardianSnapshot::default(),
    })
}

pub fn enable(engine: &Engine, id: &str, enabled: bool) -> Result<Value> {
    let site = engine.store.site(id)?;
    if enabled && !site.auto_start {
        bail!("Enable automatic start for this site before enabling Guardian.");
    }
    let mut snapshot = load(engine, id)?;
    snapshot.enabled = enabled;
    snapshot.consecutive_failures = 0;
    snapshot.last_result = Some(if enabled { "Monitoring enabled" } else { "Monitoring disabled" }.into());
    engine.store.save(&key(id), &json!(snapshot))?;
    engine.store.activity("info", Some(id), if enabled {
        "Guardian monitoring enabled; site recovery waits for three failing checks and enforces a cooldown."
    } else { "Guardian monitoring disabled; existing processes are unchanged." })?;
    Ok(json!(snapshot))
}

pub async fn check(engine: &Arc<Engine>, id: &str) -> Result<Value> {
    let site = engine.store.site(id)?;
    let mut snapshot = load(engine, id)?;
    snapshot.last_checked_at = Some(Utc::now().to_rfc3339());
    if !snapshot.enabled || !site.auto_start || site.status != "running" {
        snapshot.last_result = Some("Monitoring paused (disabled, manually stopped or not running)".into());
        snapshot.consecutive_failures = 0;
        engine.store.save(&key(id), &json!(snapshot))?;
        return Ok(json!(snapshot));
    }
    // The site runtime already checks the real HTTP path and Caddy Host routing;
    // a TCP-open listener alone is not considered healthy for dynamic websites.
    let healthy = engine.runtime.site_responds(site.local_port).await.unwrap_or(false);
    if healthy {
        snapshot.consecutive_failures = 0;
        snapshot.last_result = Some("Healthy".into());
    } else {
        snapshot.consecutive_failures = snapshot.consecutive_failures.saturating_add(1);
        let now = Utc::now().timestamp();
        snapshot.last_result = Some("Local HTTP health check failed".into());
        if decision(&snapshot, &site.status, site.auto_start, now) == Decision::Recover {
            // Serialize with user-initiated changes and DNS deployment. A busy
            // publisher always wins; the next interval will check again.
            if let (Ok(_operations), Ok(_publishing)) =
                (engine.operations.try_lock(), engine.publishing.try_lock())
            {
                if crate::build::refuse_busy(engine, id).is_ok() {
                    // Set cooldown *before* restart, including failed attempts,
                    // so a crash cannot trigger an unbounded restart storm.
                    snapshot.last_restart_unix = Some(now);
                    snapshot.recovery_attempts = snapshot.recovery_attempts.saturating_add(1);
                    snapshot.last_result = Some("Safe local restart requested".into());
                    engine.store.save(&key(id), &json!(snapshot))?;
                    let _ = engine.store.activity("warn", Some(id), "Guardian: three failed HTTP health checks; attempting one local restart.");
                    let result = match engine.runtime.stop_site(id).await {
                        Ok(()) => engine.runtime.start_site(id).await.map(|_| ()),
                        Err(e) => Err(e),
                    };
                    match result {
                        Ok(()) => {
                            snapshot.consecutive_failures = 0;
                            snapshot.last_result = Some("Local restart completed; awaiting next health check".into());
                            let _ = engine.store.activity("info", Some(id), "Guardian: local restart completed; health will be rechecked.");
                        }
                        Err(_) => {
                            snapshot.last_result = Some("Local restart failed; manual diagnosis required".into());
                            let _ = engine.store.activity("error", Some(id), "Guardian: local restart failed; cooldown active. Review application logs.");
                        }
                    }
                }
            }
        } else if decision(&snapshot, &site.status, site.auto_start, now) == Decision::Cooldown {
            snapshot.last_result = Some("Local health failed; cooldown prevents repeated restarts".into());
        }
    }
    engine.store.save(&key(id), &json!(snapshot))?;
    Ok(json!(snapshot))
}

pub fn spawn(engine: &Arc<Engine>) -> tokio::task::JoinHandle<()> {
    let weak: Weak<Engine> = Arc::downgrade(engine);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(CHECK_EVERY)).await;
            let Some(engine) = weak.upgrade() else { return; };
            let Ok(sites) = engine.store.sites() else { continue; };
            for site in sites {
                // Check the current opt-in state each time; never cache a stale
                // permission or operate on a deleted site.
                if let Ok(state) = load(&engine, &site.id) {
                    if state.enabled { let _ = check(&engine, &site.id).await; }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guardian_never_restarts_stopped_or_unconsenting_sites() {
        let mut s = GuardianSnapshot::default();
        s.consecutive_failures = 5;
        assert_eq!(decision(&s,"running",true,100), Decision::Skip);
        s.enabled = true;
        assert_eq!(decision(&s,"stopped",true,100), Decision::Skip);
        assert_eq!(decision(&s,"running",false,100), Decision::Skip);
        assert_eq!(decision(&s,"starting",true,100), Decision::Skip);
    }
    #[test]
    fn requires_repeated_failures_and_cooldown() {
        let mut s = GuardianSnapshot { enabled:true, ..Default::default() };
        assert_eq!(decision(&s,"running",true,1000), Decision::Observe);
        s.consecutive_failures = 3;
        assert_eq!(decision(&s,"running",true,1000), Decision::Recover);
        s.last_restart_unix = Some(1000);
        assert_eq!(decision(&s,"running",true,1001), Decision::Cooldown);
        assert_eq!(decision(&s,"running",true,1899), Decision::Cooldown);
        assert_eq!(decision(&s,"running",true,1900), Decision::Recover);
    }
}

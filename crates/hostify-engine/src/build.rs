//! Engine-owned install/build tasks. Imported code runs as the signed-in user.
use crate::{
    state::{Engine, Store},
    supervisor::{Health, Role, Spec, Supervisor, tail_logs},
};
use anyhow::{Result, anyhow, bail};
use hostify_core::app_config::{AppConfig, Install, RestartPolicy};
use hostify_core::deploy::SiteRuntime;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::watch;

pub(crate) struct BuildTask {
    cancel: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
pub(crate) fn refuse_busy(engine: &Engine, id: &str) -> Result<()> {
    if engine
        .builds
        .lock()
        .map_err(|_| anyhow!("Build state unavailable."))?
        .get(id)
        .is_some_and(|b| !b.task.is_finished())
    {
        bail!("A build is already running for this website. Wait or cancel it first.");
    }
    Ok(())
}
pub(crate) async fn cancel(engine: &Engine, id: &str) -> Result<()> {
    engine.store.site(id)?;
    let signal = engine
        .builds
        .lock()
        .map_err(|_| anyhow!("Build state unavailable."))?
        .get(id)
        .map(|b| b.cancel.clone());
    if let Some(signal) = signal {
        let _ = signal.send(true);
        loop {
            let finished = engine
                .builds
                .lock()
                .map_err(|_| anyhow!("Build state unavailable."))?
                .get(id)
                .is_none_or(|b| !b.cancel.same_channel(&signal) || b.task.is_finished());
            if finished {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    Ok(())
}
pub(crate) async fn shutdown(engine: &Engine) {
    let builds = engine
        .builds
        .lock()
        .map(|mut b| b.drain().map(|(_, b)| b).collect::<Vec<_>>())
        .unwrap_or_default();
    for b in &builds {
        let _ = b.cancel.send(true);
    }
    for b in builds {
        let _ = b.task.await;
    }
}
fn event(
    store: &Store,
    id: &str,
    build: &str,
    step: &str,
    state: &str,
    detail: &str,
    code: Option<i32>,
) -> Result<()> {
    let value = json!({"siteId":id,"buildId":build,"step":step,"state":state,"detail":detail,"exitCode":code});
    store.save(&format!("build:{id}"), &value)?;
    store.emit("app-build", value);
    Ok(())
}
type ProcessEnvironment = (Vec<(String, String)>, Vec<String>);
pub(crate) fn environment(
    store: &Store,
    id: &str,
    exe: &Path,
    cwd: &Path,
    install: bool,
) -> Result<ProcessEnvironment, String> {
    let runtime = exe.parent().ok_or("The Node runtime path is invalid.")?;
    let path = std::env::join_paths([
        runtime.to_path_buf(),
        cwd.join("node_modules/.bin"),
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32"),
    ])
    .map_err(|e| e.to_string())?;
    let db = store.db().map_err(|e| e.to_string())?;
    let mut env = hostify_core::env_vars::resolved(&db, id)?
        .into_iter()
        .map(|(n, v)| (n, v.as_str().to_owned()))
        .collect::<Vec<_>>();
    let redact = hostify_core::env_vars::secret_values(&db, id)?
        .iter()
        .map(|v| v.as_str().to_owned())
        .collect();
    env.retain(|(n, _)| {
        !n.eq_ignore_ascii_case("PATH") && (!install || !n.eq_ignore_ascii_case("NODE_ENV"))
    });
    if !install && !env.iter().any(|(n, _)| n.eq_ignore_ascii_case("NODE_ENV")) {
        env.push(("NODE_ENV".into(), "production".into()));
    }
    env.push(("PATH".into(), path.to_string_lossy().into_owned()));
    env.push((
        "npm_config_cache".into(),
        store.root.join("cache/npm").to_string_lossy().into_owned(),
    ));
    env.push(("npm_config_update_notifier".into(), "false".into()));
    Ok((env, redact))
}
pub(crate) fn npm(exe: &Path) -> String {
    exe.parent()
        .unwrap()
        .join("node_modules/npm/bin/npm-cli.js")
        .to_string_lossy()
        .into_owned()
}
pub(crate) async fn begin(engine: &Arc<Engine>, p: &Value) -> Result<Value> {
    let _guard = engine.operations.lock().await;
    let _publishing = engine
        .publishing
        .try_lock()
        .map_err(|_| anyhow!("Wait for the current deployment to finish."))?;
    let id = p["siteId"]
        .as_str()
        .ok_or_else(|| anyhow!("Website ID is required."))?
        .to_owned();
    let site = engine.store.site(&id)?;
    if site.kind != "node" {
        bail!("Builds are available for Node websites.");
    }
    if p["trustAcknowledged"] != true {
        bail!("Acknowledge that you trust this release before running its code.");
    }
    refuse_busy(engine, &id)?;
    let mut config = engine.store.app_config(&id)?;
    // Rebuild into a fresh release too: npm/build scripts must never mutate the
    // active working tree when a user rebuilds without importing new files.
    if config.pending_release.is_none()
        && let Some(active) = &config.active_release
    {
        let site_dir = engine.store.root.join("sites").join(&id);
        let (_, release) = hostify_core::import::import_release(
            &site_dir.join("app").join(active),
            "directory",
            &site_dir,
        )?;
        config.pending_release = Some(release);
    }
    let release = config
        .pending_release
        .clone()
        .ok_or_else(|| anyhow!("Import a release before building."))?;
    let root = engine
        .store
        .root
        .join("sites")
        .join(&id)
        .join("app")
        .join(&release);
    config.validate(&root).map_err(|e| anyhow!(e))?;
    let exe = engine
        .store
        .runtimes
        .resolve(
            "node",
            site.runtime_version
                .as_deref()
                .ok_or_else(|| anyhow!("Choose a Node version on the Runtimes page."))?,
        )
        .map_err(|e| anyhow!(e))?;
    let supervisor = engine
        .supervisor
        .clone()
        .ok_or_else(|| anyhow!("The application supervisor is unavailable."))?;
    let previous_build = config.built_release.clone();
    let previous_trust = config.trusted_release.clone();
    let was_running = site.status == "running";
    config.trusted_release = Some(release.clone());
    if config.built_release.as_ref() == Some(&release) {
        config.built_release = None;
    }
    engine.store.save(
        &format!("build-prior:{id}"),
        &json!({"builtRelease":previous_build,"trustedRelease":previous_trust}),
    )?;
    engine.store.save_app_config(&id, &config)?;
    let build = uuid::Uuid::new_v4().to_string();
    event(
        &engine.store,
        &id,
        &build,
        "install",
        "running",
        "Preparing the application build.",
        None,
    )?;
    let (cancel, rx) = watch::channel(false);
    let store = engine.store.clone();
    let build_id = build.clone();
    let site_id = id.clone();
    let task = tokio::spawn(async move {
        let mut cancellation = rx.clone();
        let mut step = "install";
        let result = run(
            &supervisor,
            &store,
            &site_id,
            &build_id,
            &root,
            &exe,
            &config,
            &mut step,
            rx,
        )
        .await;
        match result {
            Ok(()) => {
                let saved = (|| -> Result<()> {
                    let mut c = store.app_config(&site_id)?;
                    c.built_release = Some(release);
                    store.save_app_config(&site_id, &c)
                })();
                if let Err(e) = saved {
                    let _ = event(
                        &store,
                        &site_id,
                        &build_id,
                        step,
                        "failed",
                        &e.to_string(),
                        None,
                    );
                    return;
                }
                if was_running {
                    let _ = event(
                        &store,
                        &site_id,
                        &build_id,
                        "start",
                        "running",
                        "Starting the newly built release.",
                        None,
                    );
                    let started = tokio::select! {result=supervisor.start_site(&site_id)=>Some(result),_=cancellation.changed()=>None};
                    let Some(started) = started else {
                        let _ = supervisor.stop_site(&site_id).await;
                        if let Ok(mut c) = store.app_config(&site_id) {
                            c.active_release = config.active_release.clone();
                            c.built_release = previous_build;
                            c.trusted_release = previous_trust;
                            let _ = store.save_app_config(&site_id, &c);
                            if c.active_release.is_some() {
                                let _ = supervisor.start_site(&site_id).await;
                            }
                        }
                        let _ = event(
                            &store,
                            &site_id,
                            &build_id,
                            "start",
                            "cancelled",
                            "The build was cancelled during startup; the previous release was restored when available.",
                            None,
                        );
                        return;
                    };
                    match started {
                        Ok(_) => {
                            let _ = event(
                                &store,
                                &site_id,
                                &build_id,
                                "start",
                                "done",
                                "The application started.",
                                None,
                            );
                            let _ = event(
                                &store,
                                &site_id,
                                &build_id,
                                "verify",
                                "running",
                                "Checking the application through the website server.",
                                None,
                            );
                            let _ = event(
                                &store,
                                &site_id,
                                &build_id,
                                "verify",
                                "done",
                                "The application responded through the website server.",
                                Some(0),
                            );
                        }
                        Err(e) => {
                            let _ =
                                event(&store, &site_id, &build_id, "verify", "failed", &e, None);
                        }
                    }
                } else {
                    let _ = event(
                        &store,
                        &site_id,
                        &build_id,
                        step,
                        "done",
                        "The release was built successfully. Start the website to use it.",
                        Some(0),
                    );
                }
            }
            Err((state, detail, code)) => {
                // A failed pending update must not revoke the active release's build/trust.
                if let Ok(mut c) = store.app_config(&site_id)
                    && c.active_release.is_some()
                    && c.active_release != config.pending_release
                {
                    c.built_release = previous_build;
                    c.trusted_release = previous_trust;
                    let _ = store.save_app_config(&site_id, &c);
                }
                let _ = event(&store, &site_id, &build_id, step, state, &detail, code);
            }
        }
    });
    engine
        .builds
        .lock()
        .map_err(|_| anyhow!("Build state unavailable."))?
        .insert(id, BuildTask { cancel, task });
    Ok(json!({"buildId":build}))
}
type Failure = (&'static str, String, Option<i32>);
#[allow(clippy::too_many_arguments)]
async fn run(
    supervisor: &Supervisor,
    store: &Store,
    id: &str,
    build: &str,
    root: &Path,
    exe: &Path,
    config: &AppConfig,
    step: &mut &'static str,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), Failure> {
    let cwd = hostify_core::app_config::inside(root, &config.working_dir)
        .map_err(|e| ("failed", e, None))?;
    let log = store.root.join("logs").join(format!("{id}.build.log"));
    for install in [true, false] {
        *step = if install { "install" } else { "build" };
        let mut args = vec![npm(exe)];
        if install {
            match config.install {
                Install::None => continue,
                Install::Ci => args.push("ci".into()),
                Install::Install => args.push("install".into()),
            };
            args.extend(["--no-audit".into(), "--no-fund".into()]);
            if !config.allow_install_scripts {
                args.push("--ignore-scripts".into());
            }
        } else if let Some(name) = &config.build_script {
            args.extend(["run".into(), name.clone()]);
        } else {
            continue;
        }
        let (mut env, redact) =
            environment(store, id, exe, &cwd, install).map_err(|e| ("failed", e, None))?;
        if install && config.allow_install_scripts {
            // Dependency install scripts are third-party code the user never reviewed.
            let secret_names = store
                .db()
                .map_err(|e| ("failed", e.to_string(), None))
                .and_then(|db| {
                    hostify_core::env_vars::list(&db, id).map_err(|e| ("failed", e, None))
                })?
                .into_iter()
                .filter(|v| v.secret)
                .map(|v| v.name)
                .collect::<Vec<_>>();
            env.retain(|(name, _)| !secret_names.contains(name));
        }
        event(
            store,
            id,
            build,
            step,
            "running",
            "Running the application step.",
            None,
        )
        .map_err(|e| ("failed", e.to_string(), None))?;
        let spec = Spec {
            key: format!("build:{id}"),
            site: Some(id.into()),
            exe: exe.into(),
            args,
            token: None,
            cwd: Some(cwd.clone()),
            env,
            redact,
            log: log.clone(),
            port: 0,
            path: "/".into(),
            role: Role::App,
            health: Health::AnyBelow500,
            restart: RestartPolicy::Never,
            memory_limit_mb: config.memory_limit_mb,
        };
        let mut child = supervisor
            .spawn(&spec)
            .map_err(|e| ("failed", e.to_string(), None))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20 * 60);
        let (state, message, code) = loop {
            if *cancel.borrow() {
                break ("cancelled", "The build was cancelled.".to_string(), None);
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    break (
                        if status.success() { "done" } else { "failed" },
                        format!(
                            "The {step} step exited with {}.",
                            status
                                .code()
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "an unknown exit code".into())
                        ),
                        status.code(),
                    );
                }
                Err(e) => break ("failed", e.to_string(), None),
                Ok(None) => {}
            }
            if tokio::time::Instant::now() >= deadline {
                break (
                    "failed",
                    format!("The {step} step timed out after 20 minutes."),
                    None,
                );
            }
            tokio::select! {_=cancel.changed()=>{},_=tokio::time::sleep(Duration::from_millis(50))=>{}}
        };
        child.terminate_tree();
        child.finish_output().await;
        let detail = format!(
            "{message}\n{}",
            tail_logs(&log, 20).unwrap_or_default().join("\n")
        );
        let detail = spec.redact.iter().fold(detail, |text, secret| {
            if secret.is_empty() {
                text
            } else {
                text.replace(secret, "[redacted]")
            }
        });
        if state != "done" {
            return Err((state, detail, code));
        }
        event(store, id, build, step, "done", &detail, code)
            .map_err(|e| ("failed", e.to_string(), None))?;
    }
    if *cancel.borrow() {
        return Err(("cancelled", "The build was cancelled.".into(), None));
    }
    Ok(())
}

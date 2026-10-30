use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use tauri::{
    Manager, State,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_dialog::DialogExt;
mod engine;
mod windows;
// Compile the shared pure resolver without adding a workspace dependency.
#[path = "../../crates/hostify-core/src/paths.rs"]
#[allow(dead_code)]
mod paths;
use engine::EngineClient;
type Client<'a> = State<'a, Arc<EngineClient>>;

macro_rules! forward {
    ($($name:ident ( $($arg:ident : $ty:ty => $key:literal),* ));* $(;)?) => {$ (
        #[tauri::command(rename_all = "camelCase")]
        async fn $name(client: Client<'_>, $($arg: $ty),*) -> Result<Value, String> {
            client.call(stringify!($name), json!({$($key: $arg),*})).await
        }
    )*};
}
forward! {
    engine_status(); system_stats(); list_sites();
    inspect_project(path: String => "path", kind: String => "kind");
    get_app_config(site_id: String => "siteId");
    set_app_config(site_id: String => "siteId", config: Value => "config", node_version: Option<String> => "nodeVersion");
    build_app(site_id: String => "siteId", trust_acknowledged: bool => "trustAcknowledged");
    cancel_build(site_id: String => "siteId"); get_build_status(site_id: String => "siteId");
    app_status(site_id: String => "siteId");
    app_logs(site_id: String => "siteId", stream: String => "stream", lines: u32 => "lines");
    list_runtimes();
    install_runtime(kind: String => "kind", version: String => "version");
    remove_runtime(kind: String => "kind", version: String => "version");
    list_env_vars(site_id: String => "siteId");
    set_env_var(site_id: String => "siteId", name: String => "name", value: String => "value", secret: bool => "secret");
    delete_env_var(site_id: String => "siteId", name: String => "name");
    reveal_env_var(site_id: String => "siteId", name: String => "name");
    import_env(site_id: String => "siteId", text: String => "text");
    export_env(site_id: String => "siteId");
    get_site(id: String => "id");
    validate_source(path: String => "path", kind: String => "kind");
    add_site(name: String => "name", source_path: String => "sourcePath", kind: String => "kind", site_kind: Option<String> => "siteKind", config: Option<Value> => "config");
    update_site_files(id: String => "id", source_path: String => "sourcePath", kind: String => "kind");
    start_site(id: String => "id"); stop_site(id: String => "id"); restart_site(id: String => "id");
    remove_site(id: String => "id", delete_files: bool => "deleteFiles");
    preview_url(id: String => "id"); cf_connect(token: String => "token"); cf_status(); cf_disconnect();
    cf_check_domain(hostname: String => "hostname");
    cf_plan_dns(site_id: String => "siteId", hostname: String => "hostname");
    publish_site(site_id: String => "siteId", hostname: String => "hostname", consent_dns: bool => "consentDns");
    get_deployment(id: String => "id"); list_deployments(site_id: String => "siteId");
    retry_verification(id: String => "id"); cancel_deployment(id: String => "id");
    unpublish_site(site_id: String => "siteId"); list_domains(); tunnel_health();
    list_activity(site_id: Option<String> => "siteId", limit: u32 => "limit");
    site_logs(site_id: String => "siteId", lines: u32 => "lines");
    get_settings();
}
#[tauri::command]
async fn ensure_components(
    client: Client<'_>,
    names: Option<Vec<String>>,
) -> Result<Value, String> {
    client
        .call("ensure_components", json!({"names":names}))
        .await
}
#[tauri::command]
async fn save_settings(client: Client<'_>, settings: Value) -> Result<Value, String> {
    client.call("save_settings", settings).await
}
#[tauri::command]
async fn pick_path(app: tauri::AppHandle, kind: String) -> Result<Option<String>, String> {
    if !matches!(kind.as_str(), "directory" | "zip") {
        return Err("Choose a directory or ZIP file".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let dialog = app.dialog().file();
        let selected = if kind == "directory" {
            dialog.blocking_pick_folder()
        } else {
            dialog
                .add_filter("ZIP archive", &["zip"])
                .blocking_pick_file()
        };
        selected
            .map(|p| {
                p.into_path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .map_err(|e| e.to_string())
            })
            .transpose()
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let client = EngineClient::new(app.handle().clone());
            app.manage(client.clone());
            tauri::async_runtime::spawn(async move {
                loop {
                    let _ = client.call("get_settings", json!({})).await;
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            });
            let open = MenuItem::with_id(app, "open", "Open Hostify", true, None::<&str>)?;
            let quit = MenuItem::with_id(
                app,
                "quit",
                "Quit window only — hosting continues",
                true,
                None::<&str>,
            )?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("Hostify — hosting continues when the window closes")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window
                    .state::<Arc<EngineClient>>()
                    .close_to_tray
                    .load(Ordering::Relaxed)
                {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            inspect_project,
            get_app_config,
            set_app_config,
            build_app,
            cancel_build,
            get_build_status,
            app_status,
            app_logs,
            list_runtimes,
            install_runtime,
            remove_runtime,
            list_env_vars,
            set_env_var,
            delete_env_var,
            reveal_env_var,
            import_env,
            export_env,
            engine_status,
            ensure_components,
            system_stats,
            list_sites,
            get_site,
            validate_source,
            add_site,
            update_site_files,
            start_site,
            stop_site,
            restart_site,
            remove_site,
            preview_url,
            cf_connect,
            cf_status,
            cf_disconnect,
            cf_check_domain,
            cf_plan_dns,
            publish_site,
            get_deployment,
            list_deployments,
            retry_verification,
            cancel_deployment,
            unpublish_site,
            list_domains,
            tunnel_health,
            list_activity,
            site_logs,
            get_settings,
            save_settings,
            pick_path
        ])
        .run(tauri::generate_context!());
    if let Err(error) = result {
        eprintln!("Hostify could not start: {error}");
    }
}

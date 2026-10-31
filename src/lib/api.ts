import type { PublishPreflight, PublishDiagnostic,
  ActivityEntry, AppConfig, AppSettings, AppStatus, BuildStatus, CloudflareAccount, DatabaseBackup, DatabaseConnection, Deployment, DnsPlan, Domain, DomainCheck,
  EngineStatus, EnvVar, ComponentName, LogStream, ManagedDatabase, MysqlStatus, ProjectReport, RuntimeKind, RuntimeStatus, Site, SiteKind, SourceKind, SourceReport, SystemStats, TunnelHealth,
} from "./types";
import { mock } from "./mock";

/** True inside the Tauri webview. In a plain browser we use the in-memory mock (design review only). */
export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri) return mock<T>(cmd, args);
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw new Error(typeof e === "string" ? e : (e as Error)?.message ?? "Something went wrong.");
  }
}

export type Unlisten = () => void;
export async function listen<T>(event: string, cb: (payload: T) => void): Promise<Unlisten> {
  if (!isTauri) return () => {};
  const { listen: l } = await import("@tauri-apps/api/event");
  return l<T>(event, (e) => cb(e.payload));
}

/** Native file drops (Tauri only). Returns absolute paths. */
export async function onFileDrop(cb: (over: boolean, paths: string[]) => void): Promise<Unlisten> {
  if (!isTauri) return () => {};
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  return getCurrentWebview().onDragDropEvent((e) => {
    const p = e.payload;
    if (p.type === "enter" || p.type === "over") cb(true, []);
    else if (p.type === "drop") cb(false, p.paths);
    else cb(false, []);
  });
}

export const api = {
  engineStatus: () => call<EngineStatus>("engine_status"),
  ensureComponents: (names?: ComponentName[]) => call<EngineStatus>("ensure_components", names ? { names } : {}),
  systemStats: () => call<SystemStats>("system_stats"),
  listSites: () => call<Site[]>("list_sites"),
  validateSource: (path: string, kind: SourceKind) => call<SourceReport>("validate_source", { path, kind }),
  addSite: (name: string, sourcePath: string, kind: SourceKind, siteKind?: SiteKind, config?: AppConfig) => call<Site>("add_site", { name, sourcePath, kind, siteKind, config }),
  updateSiteFiles: (id: string, sourcePath: string, kind: SourceKind) => call<Site>("update_site_files", { id, sourcePath, kind }),
  startSite: (id: string) => call<Site>("start_site", { id }),
  stopSite: (id: string) => call<Site>("stop_site", { id }),
  restartSite: (id: string) => call<Site>("restart_site", { id }),
  removeSite: (id: string, deleteFiles: boolean) => call<void>("remove_site", { id, deleteFiles }),
  previewUrl: (id: string) => call<string>("preview_url", { id }),
  cfConnect: (token: string) => call<CloudflareAccount>("cf_connect", { token }),
  cfStatus: () => call<CloudflareAccount>("cf_status"),
  cfDisconnect: () => call<void>("cf_disconnect"),
  cfCheckDomain: (hostname: string) => call<DomainCheck>("cf_check_domain", { hostname }),
  cfPlanDns: (siteId: string, hostname: string) => call<DnsPlan>("cf_plan_dns", { siteId, hostname }),
  publishPreflight: (siteId: string) => call<PublishPreflight>("publish_preflight", { siteId }),
  publishDiagnosis: (siteId: string) => call<PublishDiagnostic>("publish_diagnosis", { siteId }),
  recordPublishEvent: (siteId: string, step: string, outcome: "running" | "done" | "failed") => call<void>("publish_record_event", { siteId, step, outcome }),
  publishSite: (siteId: string, hostname: string, consentDns: boolean) =>
    call<Deployment>("publish_site", { siteId, hostname, consentDns }),
  getDeployment: (id: string) => call<Deployment>("get_deployment", { id }),
  listDeployments: (siteId: string) => call<Deployment[]>("list_deployments", { siteId }),
  retryVerification: (id: string) => call<Deployment>("retry_verification", { id }),
  cancelDeployment: (id: string) => call<Deployment>("cancel_deployment", { id }),
  unpublishSite: (siteId: string) => call<void>("unpublish_site", { siteId }),
  listDomains: () => call<Domain[]>("list_domains"),
  tunnelHealth: () => call<TunnelHealth[]>("tunnel_health"),
  listActivity: (limit = 100, siteId?: string) => call<ActivityEntry[]>("list_activity", { siteId, limit }),
  siteLogs: (siteId: string, lines = 200) => call<string[]>("site_logs", { siteId, lines }),
  inspectProject: (path: string, kind: SourceKind) => call<ProjectReport>("inspect_project", { path, kind }),
  getAppConfig: (siteId: string) => call<AppConfig>("get_app_config", { siteId }),
  setAppConfig: (siteId: string, config: AppConfig, version: { nodeVersion?: string; phpVersion?: string }) => call<{ restartRequired: boolean; rebuildRequired: boolean }>("set_app_config", { siteId, config, ...version }),
  buildApp: (siteId: string, trustAcknowledged: boolean) => call<{ buildId: string }>("build_app", { siteId, trustAcknowledged }),
  cancelBuild: (siteId: string) => call<void>("cancel_build", { siteId }),
  getBuildStatus: (siteId: string) => call<BuildStatus | null>("get_build_status", { siteId }),
  appStatus: (siteId: string) => call<AppStatus>("app_status", { siteId }),
  appLogs: (siteId: string, stream: LogStream, lines = 300) => call<string[]>("app_logs", { siteId, stream, lines }),
  listRuntimes: () => call<RuntimeStatus[]>("list_runtimes"),
  installRuntime: (kind: RuntimeKind, version: string) => call<void>("install_runtime", { kind, version }),
  removeRuntime: (kind: RuntimeKind, version: string) => call<void>("remove_runtime", { kind, version }),
  listEnvVars: (siteId: string) => call<EnvVar[]>("list_env_vars", { siteId }),
  setEnvVar: (siteId: string, name: string, value: string, secret: boolean) => call<{ restartRequired: boolean }>("set_env_var", { siteId, name, value, secret }),
  deleteEnvVar: (siteId: string, name: string) => call<{ restartRequired: boolean }>("delete_env_var", { siteId, name }),
  revealEnvVar: (siteId: string, name: string) => call<string>("reveal_env_var", { siteId, name }),
  importEnv: (siteId: string, text: string) => call<{ imported: number; errors: { line: number; message: string }[]; restartRequired: boolean }>("import_env", { siteId, text }),
  exportEnv: (siteId: string) => call<string>("export_env", { siteId }),
  mysqlStatus: () => call<MysqlStatus>("mysql_status"),
  mysqlSetup: (version: string) => call<MysqlStatus>("mysql_setup", { version }),
  mysqlStart: () => call<MysqlStatus>("mysql_start"),
  mysqlStop: () => call<MysqlStatus>("mysql_stop"),
  listDatabases: () => call<ManagedDatabase[]>("list_databases"),
  createDatabase: (name: string, siteId?: string, injectEnv?: boolean, replaceEnv?: boolean) => call<{ restartRequired?: boolean }>("create_database", { name, siteId, injectEnv, replaceEnv }),
  deleteDatabase: (name: string, confirmName: string) => call<void>("delete_database", { name, confirmName }),
  linkDatabase: (name: string, siteId: string, injectEnv?: boolean, replaceEnv?: boolean) =>
    call<{ restartRequired?: boolean }>("link_database", { name, siteId, injectEnv: injectEnv ?? false, replaceEnv: replaceEnv ?? false }),
  unlinkDatabase: (name: string) => call<{ restartRequired?: boolean }>("unlink_database", { name }),
  databaseConnection: (name: string) => call<DatabaseConnection>("database_connection", { name }),
  testConnection: (name: string) => call<{ ok: boolean; detail: string; elapsedMs: number }>("test_connection", { name }),
  revealDbPassword: (user: string) => call<string>("reveal_db_password", { user }),
  rotateDbPassword: (user: string) => call<{ restartRequired?: boolean }>("rotate_db_password", { user }),
  backupDatabase: (name: string) => call<DatabaseBackup>("backup_database", { name }),
  listBackups: (name: string) => call<DatabaseBackup[]>("list_backups", { name }),
  deleteBackup: (name: string, file: string) => call<void>("delete_backup", { name, file }),
  restoreDatabase: (name: string, file: string, confirmName: string) => call<{ safetyBackup?: string }>("restore_database", { name, file, confirmName }),
  getSettings: () => call<AppSettings>("get_settings"),
  saveSettings: (settings: AppSettings) => call<AppSettings>("save_settings", { settings }),
  pickPath: (kind: SourceKind) => call<string | null>("pick_path", { kind }),
  openExternal: async (url: string) => {
    if (!isTauri) { window.open(url, "_blank", "noopener"); return; }
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  },
};

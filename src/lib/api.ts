import type {
  ActivityEntry, AppSettings, CloudflareAccount, Deployment, DnsPlan, Domain, DomainCheck,
  EngineStatus, ComponentName, Site, SourceKind, SourceReport, SystemStats, TunnelHealth,
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
  addSite: (name: string, sourcePath: string, kind: SourceKind) => call<Site>("add_site", { name, sourcePath, kind }),
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
  publishSite: (siteId: string, hostname: string, consentDns: boolean) =>
    call<Deployment>("publish_site", { siteId, hostname, consentDns }),
  getDeployment: (id: string) => call<Deployment>("get_deployment", { id }),
  listDeployments: (siteId: string) => call<Deployment[]>("list_deployments", { siteId }),
  unpublishSite: (siteId: string) => call<void>("unpublish_site", { siteId }),
  listDomains: () => call<Domain[]>("list_domains"),
  tunnelHealth: () => call<TunnelHealth[]>("tunnel_health"),
  listActivity: (limit = 100, siteId?: string) => call<ActivityEntry[]>("list_activity", { siteId, limit }),
  siteLogs: (siteId: string, lines = 200) => call<string[]>("site_logs", { siteId, lines }),
  getSettings: () => call<AppSettings>("get_settings"),
  saveSettings: (settings: AppSettings) => call<AppSettings>("save_settings", { settings }),
  pickPath: (kind: SourceKind) => call<string | null>("pick_path", { kind }),
  openExternal: async (url: string) => {
    if (!isTauri) { window.open(url, "_blank", "noopener"); return; }
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  },
};

// In-memory stand-in used ONLY when running in a plain browser (no Tauri engine) for design review.
// It is never reached inside the desktop app. Numbers here are demo data, not measurements.
import type { ActivityEntry, AppSettings, Deployment, Domain, Site } from "./types";

const now = () => new Date().toISOString();
let sites: Site[] = [
  { id: "s1", name: "Bakery landing page", sourcePath: "C:\\Hostify\\sites\\s1", localPort: 18080, localUrl: "http://127.0.0.1:18080",
    status: "running", domainId: "d1", publicUrl: "https://bakery.example.com", autoStart: true, fileCount: 42, sizeBytes: 6_400_000, createdAt: now(), updatedAt: now() },
  { id: "s2", name: "Portfolio", sourcePath: "C:\\Hostify\\sites\\s2", localPort: 18081, localUrl: "http://127.0.0.1:18081",
    status: "stopped", autoStart: false, fileCount: 18, sizeBytes: 2_100_000, createdAt: now(), updatedAt: now() },
];
const domains: Domain[] = [
  { id: "d1", hostname: "bakery.example.com", zoneName: "example.com", zoneId: "z1", siteId: "s1", dnsStatus: "configured", httpsVerified: true },
];
let settings: AppSettings = { launchOnLogin: true, startEngineOnBoot: false, closeToTray: true, theme: "system", basePortStart: 18080 };
let cf = { connected: false } as { connected: boolean; accountName?: string; accountId?: string; tokenHint?: string };
const activity: ActivityEntry[] = [
  { id: 2, ts: now(), level: "info", siteId: "s1", message: "Bakery landing page is live at https://bakery.example.com" },
  { id: 1, ts: now(), level: "info", message: "Hostify engine started" },
];
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));
const patch = (id: string, p: Partial<Site>) => { sites = sites.map((s) => (s.id === id ? { ...s, ...p, updatedAt: now() } : s)); return sites.find((s) => s.id === id)!; };

export async function mock<T>(cmd: string, a: Record<string, any> = {}): Promise<T> {
  await wait(120);
  const r = (v: unknown) => v as T;
  switch (cmd) {
    case "engine_status": return r({ running: true, version: "preview", mode: "user", componentsReady: true, localHostingReady: true, caddyVersion: "preview", cloudflaredVersion: "preview", caddy: { installed: true, ready: true, version: "preview", path: "C:\Hostify\bin\caddy.exe" }, cloudflared: { installed: true, ready: true, version: "preview", path: "C:\Hostify\bin\cloudflared.exe" }, dataDir: "C:\Hostify", binDir: "C:\Hostify\bin" });
    case "ensure_components": return mock<T>("engine_status");
    case "system_stats": return r({ cpuPercent: 4 + Math.random() * 9, ramUsedBytes: 9.1e9, ramTotalBytes: 17e9, diskUsedBytes: 310e9, diskTotalBytes: 512e9, hostifyDataBytes: 8.5e6 });
    case "list_sites": return r(sites);
    case "get_site": return r(sites.find((s) => s.id === a.id));
    case "validate_source": return r({ ok: true, fileCount: 24, sizeBytes: 3_200_000, hasIndex: true, warnings: [], errors: [] });
    case "add_site": { const s: Site = { id: "s" + Date.now(), name: a.name, sourcePath: a.sourcePath, localPort: 18080 + sites.length + 5, localUrl: "", status: "running", autoStart: true, fileCount: 24, sizeBytes: 3_200_000, createdAt: now(), updatedAt: now() }; s.localUrl = `http://127.0.0.1:${s.localPort}`; sites = [s, ...sites]; return r(s); }
    case "start_site": return r(patch(a.id, { status: "running" }));
    case "stop_site": return r(patch(a.id, { status: "stopped" }));
    case "restart_site": return r(patch(a.id, { status: "running" }));
    case "update_site_files": return r(patch(a.id, {}));
    case "remove_site": sites = sites.filter((s) => s.id !== a.id); return r(undefined);
    case "preview_url": return r(sites.find((s) => s.id === a.id)?.localUrl);
    case "cf_connect": cf = { connected: true, accountName: "Preview account", accountId: "acc", tokenHint: "…a1b2" }; return r(cf);
    case "cf_status": return r(cf);
    case "cf_disconnect": cf = { connected: false }; return r(undefined);
    case "cf_check_domain": return r({ zoneName: String(a.hostname).split(".").slice(-2).join("."), zoneId: "z1", canManageDns: true, problems: [] });
    case "cf_plan_dns": return r({ hostname: a.hostname, action: "create", message: `Hostify will add one CNAME record for ${a.hostname}. Nothing else changes.` });
    case "publish_site": {
      const steps = ["Validate website", "Start local server", "Verify local site", "Create tunnel", "Add DNS record", "Start connector", "Verify HTTPS"]
        .map((label, i) => ({ key: "k" + i, label, state: "pending" as const }));
      return r({ id: "dep" + Date.now(), siteId: a.siteId, state: "running", steps, startedAt: now() } satisfies Deployment);
    }
    case "list_domains": return r(domains);
    case "tunnel_health": return r([{ tunnelId: "t1", name: "hostify-preview", connected: true, connections: 4, lastChange: now() }]);
    case "list_activity": return r(activity);
    case "site_logs": return r(["[preview] Logs appear here when the engine is running."]);
    case "get_settings": return r(settings);
    case "save_settings": settings = a.settings; return r(settings);
    case "pick_path": return r(a.kind === "zip" ? "C:\\Users\\you\\Downloads\\site.zip" : "C:\\Users\\you\\Documents\\my-site");
    default: return r(undefined);
  }
}

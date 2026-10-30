// In-memory stand-in used ONLY when running in a plain browser (no Tauri engine) for design review.
// It is never reached inside the desktop app. Numbers here are demo data, not measurements.
import type { ActivityEntry, AppSettings, Deployment, Domain, Site, VerifyCheck } from "./types";

const now = () => new Date().toISOString();
let sites: Site[] = [
  { id: "s1", name: "Bakery landing page", sourcePath: "C:\\Hostify\\sites\\s1", localPort: 18080, localUrl: "http://127.0.0.1:18080",
    status: "running", domainId: "d1", publicUrl: "https://bakery.example.com", publishState: "published", publishHostname: "bakery.example.com", autoStart: true, fileCount: 42, sizeBytes: 6_400_000, createdAt: now(), updatedAt: now() },
  { id: "s2", name: "Portfolio", sourcePath: "C:\\Hostify\\sites\\s2", localPort: 18081, localUrl: "http://127.0.0.1:18081",
    status: "stopped", autoStart: false, fileCount: 18, sizeBytes: 2_100_000, createdAt: now(), updatedAt: now() },
];
if (typeof location !== "undefined" && location.search.includes("empty")) sites = [];
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

// Timed walk through the publish states for design review. Hostnames starting "stuck." or "fail." preview those endings.
const started = new Map<string, { at: number; siteId: string; hostname: string }>();
const STEP_KEYS = [["validate_website", "Validate website"], ["start_local_server", "Start local server"], ["verify_local_site", "Verify local site"], ["create_tunnel", "Create tunnel"], ["add_dns_record", "Add DNS record"], ["start_connector", "Start connector"], ["verify_https", "Verify HTTPS"]];
function previewDeployment(siteId: string, hostname: string, at: number, id = `dep${at}`): Deployment {
  const t = Math.floor((Date.now() - at) / 1000); const i = Math.min(t, 6);
  const fail = hostname.startsWith("fail.") && t >= 3; const stuck = hostname.startsWith("stuck.");
  const state: Deployment["state"] = fail ? "failed" : t < 1 ? "validating" : t < 3 ? "provisioning_tunnel" : t < 4 ? "configuring_dns" : t < 5 ? "starting_connector"
    : t < 9 ? "waiting_dns" : stuck ? "recovery_required" : t < 12 ? "waiting_tls" : t < 14 ? "verifying_public" : "published";
  const pass = (key: VerifyCheck["key"], label: string, ok: boolean, detail?: string): VerifyCheck => ({ key, label, state: ok ? "passed" : "pending", detail: ok ? detail : undefined });
  return {
    id, siteId, hostname, state, startedAt: new Date(at).toISOString(), error: fail ? "Preview: Cloudflare refused the tunnel because the token lacks Account, Cloudflare Tunnel, Edit." : undefined,
    steps: STEP_KEYS.map(([key, label], j) => ({ key, label, state: fail && j === 3 ? "failed" : fail && j > 3 ? "pending" : state === "published" || j < i ? "done" : j === i ? "running" : "pending" })),
    verification: t < 5 || fail ? undefined : {
      attempt: Math.max(1, t - 4), deadlineAt: new Date(at + 1800000).toISOString(), lastCheckedAt: now(), nextRetryAt: state === "published" || stuck && t >= 9 ? undefined : new Date(Date.now() + 3000).toISOString(),
      errorClass: state === "published" ? undefined : t < 9 || stuck ? "dns_not_visible" : "tls_pending", visibleFromThisPc: state === "published" ? false : undefined,
      message: state === "published" ? undefined : t < 9 || stuck ? "Your DNS record is in place, but public resolvers aren't returning it yet." : "DNS is working. Cloudflare is finishing the HTTPS certificate.",
      checks: [pass("zone", "Domain active on Cloudflare", true, "Status active"), pass("dns_record", "DNS record in place", true, "Proxied CNAME to your tunnel"), pass("connector", "Connector online", true, "4 connections"),
        pass("dns_public", "Public DNS answers", t >= 9, "1.1.1.1 and authoritative nameservers"), pass("tls", "HTTPS certificate valid", t >= 12), pass("http", "Website responds", t >= 13), pass("content", "This deployment's marker returned", t >= 14)],
    },
  };
}

export async function mock<T>(cmd: string, a: Record<string, any> = {}): Promise<T> {
  await wait(120);
  const r = (v: unknown) => v as T;
  switch (cmd) {
    case "engine_status": return r({ running: true, version: "preview", mode: "user", componentsReady: true, localHostingReady: true, caddyVersion: "preview", cloudflaredVersion: "preview", caddy: { installed: true, ready: true, version: "preview", path: "C:\Hostify\bin\caddy.exe" }, cloudflared: { installed: true, ready: true, version: "preview", path: "C:\Hostify\bin\cloudflared.exe" }, dataDir: "C:\Hostify", binDir: "C:\Hostify\bin" });
    case "ensure_components": return mock<T>("engine_status");
    case "system_stats": return r({ cpuPercent: 4 + Math.random() * 9, ramUsedBytes: 9.1e9, ramTotalBytes: 17e9, diskUsedBytes: 310e9, diskTotalBytes: 512e9, hostifyDataBytes: 8.5e6 });
    case "list_sites": return r(sites.map((x) => { const e = [...started.entries()].reverse().find(([, v]) => v.siteId === x.id); return e ? { ...x, publishDeploymentId: e[0], publishHostname: e[1].hostname, publishState: previewDeployment(x.id, e[1].hostname, e[1].at, e[0]).state } : x; }));
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
      const d = previewDeployment(a.siteId, a.hostname, Date.now());
      started.set(d.id, { at: Date.now(), siteId: a.siteId, hostname: a.hostname });
      return r(d);
    }
    case "get_deployment": { const s = started.get(a.id); return r(s ? previewDeployment(s.siteId, s.hostname, s.at, a.id) : undefined); }
    case "retry_verification": { const s = started.get(a.id); if (s) s.at = Date.now() - 9000; return r(s && previewDeployment(s.siteId, s.hostname, s.at, a.id)); }
    case "cancel_deployment": { const s = started.get(a.id); return r(s && { ...previewDeployment(s.siteId, s.hostname, s.at, a.id), state: "recovery_required" }); }
    case "list_domains": return r(domains);
    case "tunnel_health": return r([{ tunnelId: "t1", name: "hostify-s1", connected: true, connections: 4, lastChange: now() }]);
    case "list_activity": return r(activity);
    case "site_logs": return r(["[preview] Logs appear here when the engine is running."]);
    case "get_settings": return r(settings);
    case "save_settings": settings = a.settings; return r(settings);
    case "pick_path": return r(a.kind === "zip" ? "C:\\Users\\you\\Downloads\\site.zip" : "C:\\Users\\you\\Documents\\my-site");
    default: return r(undefined);
  }
}

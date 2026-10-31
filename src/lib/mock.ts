// In-memory stand-in used ONLY when running in a plain browser (no Tauri engine) for design review.
// It is never reached inside the desktop app. Numbers here are demo data, not measurements.
import type { ActivityEntry, AppConfig, AppSettings, Deployment, Domain, EnvVar, ManagedDatabase, ProjectReport, RuntimeStatus, Site, VerifyCheck } from "./types";

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

// Design-preview stand-ins for the V2 screens. A path containing "fail" previews a failed build.
const baseConfig: AppConfig = { install: "ci", buildScript: "build", start: { type: "npmScript", name: "start" }, workingDir: ".", healthPath: "/", restart: "always", memoryLimitMb: null, allowInstallScripts: false,
  documentRoot: ".", phpWorkers: 4, phpExtensions: ["mbstring", "openssl", "curl", "fileinfo", "mysqli", "pdo_mysql"], phpSettings: {}, persistentPaths: [],
  internalPort: 19100, activeRelease: null, previousRelease: null, pendingRelease: "r1", builtRelease: null, trustedRelease: null };
const configs = new Map<string, AppConfig>();
const envs = new Map<string, EnvVar[]>();
const builds = new Map<string, { at: number; fail: boolean; id: string }>();
const runtimes: RuntimeStatus[] = [
  { kind: "node", version: "24.22.2", installed: true, ready: true, sizeBytes: 107_353_198, usedBy: [] },
  { kind: "node", version: "26.13.0", installed: false, ready: false, usedBy: [] },
  { kind: "php", version: "8.4.27-1", installed: false, ready: false, usedBy: [] },
  { kind: "php", version: "8.5.12-1", installed: false, ready: false, usedBy: [] },
  { kind: "mysql", version: "8.4.10", installed: false, ready: false, usedBy: [] },
];
let mysql = { setUp: false, running: false } as { setUp: boolean; running: boolean; version?: string; port?: number; healthy?: boolean; dataBytes?: number; memoryBytes?: number; freeDiskBytes?: number };
let databases: ManagedDatabase[] = [];
const backups = new Map<string, { file: string; sizeBytes: number; createdAt: string }[]>();
function inspect(path: string): ProjectReport {
  const node = /node|express|socket/i.test(path); const php = /php|wordpress/i.test(path);
  return { siteKind: node ? "node" : php ? "php" : "static", framework: /socket/i.test(path) ? "socket.io" : /express|node/i.test(path) ? "express" : /wordpress/i.test(path) ? "wordpress" : null, name: null, engines: null,
    recommendedRuntime: node ? "24.22.2" : "8.4.27-1", scripts: node ? { start: "node server.js", build: "node build.js" } : {}, lifecycleScripts: node ? { postinstall: "node setup.js" } : {}, dependenciesWithInstallScripts: node ? ["bcrypt"] : [],
    lockfile: node ? "package-lock" : null, hasServerEntry: node, dotenvNames: node ? ["API_TOKEN"] : [], usesDatabase: /mysql|wordpress/i.test(path), suggested: { ...baseConfig, buildScript: node ? "build" : null, install: node ? "ci" : "none" }, warnings: [] };
}
function buildStatus(id: string) {
  const b = builds.get(id); if (!b) return null;
  const t = (Date.now() - b.at) / 1000; const site = sites.find((s) => s.id === id);
  if (b.fail && t >= 1) return { siteId: id, buildId: b.id, step: "install", state: "failed", detail: "The install step exited with 1.\nnpm error Preview: package-lock.json is out of sync.", exitCode: 1 };
  if (site?.kind === "php" || t >= 2) {
    const c = configs.get(id); if (c) { c.builtRelease = c.pendingRelease; c.trustedRelease = c.pendingRelease; }
    return { siteId: id, buildId: b.id, step: "build", state: "done", detail: "The release was built successfully. Start the website to use it.", exitCode: 0 };
  }
  return { siteId: id, buildId: b.id, step: t < 1 ? "install" : "build", state: "running", detail: "Running the application step.", exitCode: null };
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
    case "inspect_project": return r(inspect(String(a.path)));
    case "get_app_config": return r(configs.get(a.siteId) ?? baseConfig);
    case "set_app_config": { const v = a.nodeVersion ?? a.phpVersion; const old = configs.get(a.siteId); configs.set(a.siteId, { ...a.config, activeRelease: old?.activeRelease ?? null, builtRelease: old?.builtRelease ?? null, trustedRelease: old?.trustedRelease ?? null, pendingRelease: old?.pendingRelease ?? null }); if (v) patch(a.siteId, { runtimeVersion: v }); return r({ restartRequired: false, rebuildRequired: false }); }
    case "build_app": { const id = "b" + Date.now(); builds.set(a.siteId, { at: Date.now(), fail: /fail/i.test(sites.find((s) => s.id === a.siteId)?.sourcePath ?? ""), id }); return r({ buildId: id }); }
    case "cancel_build": builds.delete(a.siteId); return r(undefined);
    case "get_build_status": return r(buildStatus(a.siteId));
    case "app_status": { const s = sites.find((x) => x.id === a.siteId); return r(s?.status === "running" ? { health: "running", exposedAddresses: [], pid: 4242, processCount: 2, cpuPercent: 0.4, memoryBytes: 63_000_000, restarts: 0, internalPort: 19100, activeRelease: "r1" } : { health: "stopped", exposedAddresses: [] }); }
    case "app_logs": return r(a.stream === "build" ? ["[preview] npm ci", "[preview] added 69 packages"] : ["[preview] Logs appear here when the engine is running."]);
    case "list_runtimes": return r(runtimes);
    case "install_runtime": { await wait(600); const x = runtimes.find((v) => v.kind === a.kind && v.version === a.version); if (x) { x.installed = true; x.ready = true; x.sizeBytes = 100_000_000; } return r(undefined); }
    case "remove_runtime": { const x = runtimes.find((v) => v.kind === a.kind && v.version === a.version); if (x) { x.installed = false; x.ready = false; x.sizeBytes = undefined; } return r(undefined); }
    case "list_env_vars": return r((envs.get(a.siteId) ?? []).map((v) => (v.secret ? { name: v.name, secret: true } : v)));
    case "set_env_var": { const l = (envs.get(a.siteId) ?? []).filter((v) => v.name !== a.name); l.push({ name: a.name, secret: a.secret, value: a.value }); envs.set(a.siteId, l); return r({ restartRequired: sites.find((s) => s.id === a.siteId)?.status === "running" }); }
    case "delete_env_var": envs.set(a.siteId, (envs.get(a.siteId) ?? []).filter((v) => v.name !== a.name)); return r({ restartRequired: false });
    case "reveal_env_var": return r((envs.get(a.siteId) ?? []).find((v) => v.name === a.name)?.value ?? "");
    case "import_env": return r({ imported: 1, errors: [], restartRequired: false });
    case "export_env": return r("# Omitted secrets: none");
    case "mysql_status": return r(mysql);
    case "mysql_setup": mysql = { setUp: true, running: true, version: "8.4.10", port: 3307, healthy: true, dataBytes: 190_000_000, memoryBytes: 400_000_000, freeDiskBytes: 200e9 }; return r(mysql);
    case "mysql_start": mysql = { ...mysql, running: true, healthy: true }; return r(mysql);
    case "mysql_stop": mysql = { ...mysql, running: false, healthy: undefined }; return r(mysql);
    case "list_databases": return r(databases);
    case "create_database": { if (/fail/.test(a.name)) throw new Error("Preview: MySQL refused the new database."); databases = [...databases, { name: a.name, siteId: a.siteId, siteName: sites.find((s) => s.id === a.siteId)?.name, sizeBytes: 16384, users: [`${a.name}_app`], connectionOk: true }]; return r({ restartRequired: false }); }
    case "delete_database": databases = databases.filter((d) => d.name !== a.name); return r(undefined);
    case "link_database": {
      databases = databases.map((d) => d.name === a.name ? { ...d, siteId: a.siteId, siteName: sites.find((s) => s.id === a.siteId)?.name } : d);
      return r({ restartRequired: true });
    }
    case "unlink_database": {
      databases = databases.map((d) => d.name === a.name ? { ...d, siteId: null, siteName: null } : d);
      return r({ restartRequired: true });
    }
    case "database_connection": return r({ host: "127.0.0.1", port: 3307, database: a.name, user: `${a.name}_app` });
    case "test_connection": return r({ ok: true, detail: "Connected as the website's user.", elapsedMs: 12 });
    case "reveal_db_password": return r("preview-only-not-a-real-password");
    case "rotate_db_password": return r({ restartRequired: true });
    case "backup_database": { const b = { file: `${Date.now()}.sql`, sizeBytes: 2048, createdAt: now() }; backups.set(a.name, [b, ...(backups.get(a.name) ?? [])]); databases = databases.map((d) => (d.name === a.name ? { ...d, lastBackupAt: b.createdAt, lastBackupBytes: b.sizeBytes } : d)); return r(b); }
    case "list_backups": return r(backups.get(a.name) ?? []);
    case "delete_backup": backups.set(a.name, (backups.get(a.name) ?? []).filter((b) => b.file !== a.file)); return r(undefined);
    case "restore_database": return r({ safetyBackup: "safety.sql" });
    case "add_site": { if (a.siteKind && a.siteKind !== "static") { const s: Site = { id: "s" + Date.now(), kind: a.siteKind, name: a.name, sourcePath: a.sourcePath, localPort: 18090 + sites.length, localUrl: "", status: "stopped", autoStart: true, fileCount: 12, sizeBytes: 90_000, createdAt: now(), updatedAt: now() }; s.localUrl = `http://127.0.0.1:${s.localPort}`; sites = [s, ...sites]; configs.set(s.id, { ...baseConfig, ...a.config, pendingRelease: "r1", builtRelease: null, activeRelease: null }); return r(s); }
      const s: Site = { id: "s" + Date.now(), name: a.name, sourcePath: a.sourcePath, localPort: 18080 + sites.length + 5, localUrl: "", status: "running", autoStart: true, fileCount: 24, sizeBytes: 3_200_000, createdAt: now(), updatedAt: now() }; s.localUrl = `http://127.0.0.1:${s.localPort}`; sites = [s, ...sites]; return r(s); }
    case "start_site": { const c = configs.get(a.id); if (c) { if (!c.builtRelease) throw new Error("Build and acknowledge trust for this release before starting this website."); c.activeRelease = c.builtRelease; c.pendingRelease = null; } return r(patch(a.id, { status: "running" })); }
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
    case "pick_path": { const forced = (window as unknown as { __hostifyMockPath?: string }).__hostifyMockPath; if (forced) return r(forced); return r(a.kind === "zip" ? "C:\\Users\\you\\Downloads\\site.zip" : "C:\\Users\\you\\Documents\\my-site"); }
    default: return r(undefined);
  }
}

// Mirrors docs/IPC.md. Keep in sync with the Rust side.
export type SiteStatus = "stopped" | "starting" | "running" | "error" | "deploying";
export type SourceKind = "directory" | "zip";

export interface Site {
  id: string; name: string; sourcePath: string; localPort: number; localUrl: string;
  status: SiteStatus; statusDetail?: string; domainId?: string; publicUrl?: string; autoStart: boolean;
  fileCount: number; sizeBytes: number; createdAt: string; updatedAt: string;
}
export interface Domain {
  id: string; hostname: string; zoneName: string; zoneId: string; siteId?: string;
  dnsStatus: "unconfigured" | "pending" | "configured" | "conflict" | "error"; httpsVerified: boolean;
}
export interface CloudflareAccount { connected: boolean; accountName?: string; accountId?: string; tokenHint?: string }
export interface DeploymentStep { key: string; label: string; state: "pending" | "running" | "done" | "failed" | "skipped"; detail?: string }
export interface Deployment {
  id: string; siteId: string; state: "running" | "live" | "failed" | "rolled_back";
  steps: DeploymentStep[]; error?: string; startedAt: string; finishedAt?: string;
}
export interface DnsPlan { hostname: string; action: "create" | "update_own" | "conflict"; existing?: { type: string; content: string }; message: string }
export interface SystemStats {
  cpuPercent: number; ramUsedBytes: number; ramTotalBytes: number;
  diskUsedBytes: number; diskTotalBytes: number; hostifyDataBytes: number;
}
export interface TunnelHealth { tunnelId: string; name: string; connected: boolean; connections: number; lastChange: string }
export interface ActivityEntry { id: number; ts: string; level: "info" | "warn" | "error"; siteId?: string; message: string }
export interface AppSettings { launchOnLogin: boolean; startEngineOnBoot: boolean; closeToTray: boolean; theme: "system" | "light" | "dark"; basePortStart: number }
export interface EngineStatus {
  running: boolean; version: string; mode: "user" | "service";
  caddyVersion?: string; cloudflaredVersion?: string; componentsReady: boolean;
}
export interface SourceReport { ok: boolean; fileCount: number; sizeBytes: number; hasIndex: boolean; warnings: string[]; errors: string[] }
export interface DomainCheck { zoneName: string; zoneId: string; canManageDns: boolean; problems: string[] }

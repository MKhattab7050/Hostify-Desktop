// Mirrors docs/IPC.md. Keep in sync with the Rust side.
export type SiteStatus = "stopped" | "starting" | "running" | "error" | "deploying";
export type SourceKind = "directory" | "zip";

export interface Site {
  id: string; name: string; sourcePath: string; localPort: number; localUrl: string;
  status: SiteStatus; statusDetail?: string; domainId?: string; publicUrl?: string; autoStart: boolean;
  fileCount: number; sizeBytes: number; createdAt: string; updatedAt: string;
  publishState?: DeploymentState; publishDeploymentId?: string; publishHostname?: string;
}
export interface Domain {
  id: string; hostname: string; zoneName: string; zoneId: string; siteId?: string;
  dnsStatus: "unconfigured" | "pending" | "configured" | "conflict" | "error"; httpsVerified: boolean;
}
export interface CloudflareAccount { connected: boolean; accountName?: string; accountId?: string; tokenHint?: string }
export interface DeploymentStep { key: string; label: string; state: "pending" | "running" | "done" | "failed" | "skipped"; detail?: string }
export type DeploymentState = "validating" | "starting_local" | "provisioning_tunnel" | "configuring_dns" | "starting_connector"
  | "waiting_dns" | "waiting_tls" | "verifying_public" | "published" | "failed" | "recovery_required" | "rolled_back";
export type VerifyClass = "zone_not_active" | "dns_record_missing" | "dns_not_visible" | "tls_pending" | "connector_down"
  | "origin_unreachable" | "routing_mismatch" | "network_error" | "cloudflare_api";
export interface VerifyCheck { key: "zone" | "dns_record" | "connector" | "dns_public" | "tls" | "http" | "content"; label: string; state: "pending" | "passed" | "failed"; detail?: string }
export interface Verification {
  attempt: number; nextRetryAt?: string; deadlineAt: string; errorClass?: VerifyClass; message?: string;
  checks: VerifyCheck[]; lastCheckedAt?: string; visibleFromThisPc?: boolean; nameservers?: string[];
}
export interface Deployment {
  id: string; siteId: string; hostname?: string; state: DeploymentState;
  steps: DeploymentStep[]; verification?: Verification; error?: string; startedAt: string; finishedAt?: string;
}
export interface DnsPlan { hostname: string; action: "create" | "update_own" | "conflict"; existing?: { type: string; content: string }; message: string }
export interface SystemStats {
  cpuPercent: number; ramUsedBytes: number; ramTotalBytes: number;
  diskUsedBytes: number; diskTotalBytes: number; hostifyDataBytes: number;
}
export interface TunnelHealth { tunnelId: string; name: string; connected: boolean; connections: number; lastChange: string }
export interface ActivityEntry { id: number; ts: string; level: "info" | "warn" | "error"; siteId?: string; message: string }
export interface AppSettings { launchOnLogin: boolean; startEngineOnBoot: boolean; closeToTray: boolean; theme: "system" | "light" | "dark"; basePortStart: number }
export interface ComponentStatus { installed: boolean; ready: boolean; version?: string; path: string; error?: string }
export interface EngineStatus {
  running: boolean; version: string; mode: "user" | "service";
  caddyVersion?: string; cloudflaredVersion?: string;
  /** Both components ready: required to publish. */
  componentsReady: boolean;
  /** Caddy ready: all that local hosting needs. */
  localHostingReady?: boolean;
  caddy?: ComponentStatus; cloudflared?: ComponentStatus;
  dataDir?: string; binDir?: string;
}
export type ComponentName = "caddy" | "cloudflared";
export interface SourceReport { ok: boolean; fileCount: number; sizeBytes: number; hasIndex: boolean; warnings: string[]; errors: string[] }
export interface DomainCheck { zoneName: string; zoneId: string; canManageDns: boolean; problems: string[] }

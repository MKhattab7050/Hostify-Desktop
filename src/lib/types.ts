// Mirrors docs/IPC.md. Keep in sync with the Rust side.
export type SiteStatus = "stopped" | "starting" | "running" | "error" | "deploying";
export type SourceKind = "directory" | "zip";

export type SiteKind = "static" | "node" | "php";
export interface Site {
  kind?: SiteKind; runtimeVersion?: string | null;
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

export type RuntimeKind = "node" | "php" | "mysql";
export interface RuntimeStatus { kind: RuntimeKind; version: string; installed: boolean; ready: boolean; error?: string; sizeBytes?: number; path?: string; usedBy: string[] }
export interface RuntimeProgress { kind: RuntimeKind; version: string; phase: "download" | "verify" | "extract" | "check"; done: number; total: number | null }
export interface EnvVar { name: string; secret: boolean; value?: string }
export interface AppConfig {
  install: "ci" | "install" | "none"; buildScript: string | null;
  start: { type: "npmScript"; name: string } | { type: "node"; args: string[] };
  workingDir: string; healthPath: string; restart: "always" | "onFailure" | "never"; memoryLimitMb: number | null; allowInstallScripts: boolean;
  documentRoot: string; phpWorkers: number; phpExtensions: string[]; phpSettings: Record<string, string>; persistentPaths: string[];
  internalPort: number | null; activeRelease: string | null; previousRelease: string | null; pendingRelease: string | null; builtRelease: string | null; trustedRelease: string | null;
}
export interface ProjectReport {
  siteKind: SiteKind | "unknown"; framework: string | null; name: string | null; engines: string | null; recommendedRuntime: string;
  scripts: Record<string, string>; lifecycleScripts: Record<string, string>; dependenciesWithInstallScripts: string[];
  lockfile: string | null; hasServerEntry: boolean; dotenvNames: string[]; usesDatabase: boolean; suggested: AppConfig; warnings: string[];
}
export interface BuildStatus { siteId: string; buildId: string; step: "install" | "build" | "start" | "verify"; state: "running" | "done" | "failed" | "cancelled"; detail: string; exitCode: number | null }
export interface AppStatus {
  health: string; exposedAddresses: string[]; pid?: number; processCount?: number; cpuPercent?: number; memoryBytes?: number; restarts?: number;
  lastExitCode?: number; lastExitAt?: string; internalPort?: number; activeRelease?: string; workers?: { port: number; pid: number | null; health: string }[];
}
export type LogStream = "app" | "build" | "access";
export interface MysqlStatus { setUp: boolean; version?: string; running: boolean; port?: number; pid?: number; memoryBytes?: number; dataBytes?: number; freeDiskBytes?: number; lastError?: string; healthy?: boolean }
export interface ManagedDatabase { name: string; siteId?: string | null; siteName?: string | null; sizeBytes?: number; users: string[]; connectionOk: boolean | null; lastBackupAt?: string | null; lastBackupBytes?: number | null }
export interface DatabaseBackup { file: string; sizeBytes: number; createdAt: string; time?: string }
export interface DatabaseConnection { host: string; port: number; database: string; user: string }

/** Shared engine diagnostics. Repairs never bypass security or approval. */
export type PublishErrorClass =
  | "TRANSIENT_RETRYABLE" | "AUTO_FIX_SAFE" | "USER_APPROVAL_REQUIRED"
  | "CONFIGURATION_ERROR" | "APPLICATION_ERROR" | "SECURITY_BLOCKER"
  | "DATA_INTEGRITY_BLOCKER" | "EXTERNAL_PROVIDER_ERROR";
export interface PublishDiagnostic {
  class: PublishErrorClass;
  message: string;
  nextAction: string;
  safeToRetry: boolean;
}
export interface PublishPreflight {
  ready: boolean;
  mode: "shared";
  blocked: string[];
  checks: { key: string; passed: boolean; detail: string }[];
  diagnostic?: PublishDiagnostic | null;
}

/** Opt-in Guardian monitoring. It never repairs project files or DNS. */
export interface GuardianStatus {
  enabled: boolean; consecutiveFailures: number; recoveryAttempts: number;
  lastRestartUnix?: number | null; lastCheckedAt?: string | null; lastResult?: string | null;
}

/** Integrity-checked local code snapshot. MySQL, secrets and machine disaster recovery excluded. */
export interface SiteSnapshot {
  id: string; siteId: string; siteKind: string; release: string | null; createdAt: string;
  fileCount: number; sourceBytes: number; archiveBytes: number; sha256: string;
  includesDatabase: false; includesSecrets: false;
}

/** Local file-only backup cadence; paused at five retained copies, never deleting automatically. */
export interface SiteSnapshotSchedule {
  enabled: boolean; everyHours: 24 | 72 | 168;
  lastRunUnix: number | null; lastAttemptUnix: number | null;
  pausedForCapacity: boolean;
}

/** Built-in Hostify templates. Dynamic projects are never auto-trusted or auto-started. */
export interface AppTemplate {
  id: string; name: string; kind: SiteKind; framework: string;
  description: string; requiresTrust: boolean; requiresDatabase: boolean;
}

export interface DoctorIssue {
  id: string; severity: "low" | "medium" | "high"; category: string;
  headline: string; explanation: string; action: string; autoFixed: false;
}
export interface DoctorReport {
  issues: DoctorIssue[]; linesReviewed: number;
  hasActionableFindings: boolean; engine: "offline-rules-v1";
}

/** Local Security Center evidence. An unverified result is never a security pass. */
export interface SecurityFinding {
  siteId?: string | null; id: string; severity: "medium" | "high" | "critical";
  status: "block" | "warn" | "unverified"; title: string;
  detail: string; recommendedAction: string;
}
export interface SecurityPosture {
  checkedAt: string; inspectedSites: number; blockingFindings: number;
  warningFindings: number; unverifiedChecks: number; findings: SecurityFinding[];
}

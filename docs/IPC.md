# IPC contract

Tauri commands, camelCase JSON. Rust returns `Result<T, String>`; the error string must be user-presentable.

## Types

```ts
type SiteStatus = 'stopped'|'starting'|'running'|'error'|'deploying';
interface Site { id:string; name:string; sourcePath:string; localPort:number; localUrl:string;
  status:SiteStatus; statusDetail?:string; domainId?:string; publicUrl?:string; autoStart:boolean;
  fileCount:number; sizeBytes:number; createdAt:string; updatedAt:string;
  publishState?:DeploymentState; publishDeploymentId?:string; publishHostname?:string; }
interface Domain { id:string; hostname:string; zoneName:string; zoneId:string; siteId?:string;
  dnsStatus:'unconfigured'|'pending'|'configured'|'conflict'|'error'; httpsVerified:boolean; }
interface CloudflareAccount { connected:boolean; accountName?:string; accountId?:string; tokenHint?:string; }
interface DeploymentStep { key:string; label:string; state:'pending'|'running'|'done'|'failed'|'skipped'; detail?:string; }
type DeploymentState = 'validating'|'starting_local'|'provisioning_tunnel'|'configuring_dns'|'starting_connector'
  |'waiting_dns'|'waiting_tls'|'verifying_public'|'published'|'failed'|'recovery_required'|'rolled_back';
type VerifyClass = 'zone_not_active'|'dns_record_missing'|'dns_not_visible'|'tls_pending'|'connector_down'|'origin_unreachable'
  |'routing_mismatch'|'network_error'|'cloudflare_api';
interface VerifyCheck { key:'zone'|'dns_record'|'connector'|'dns_public'|'tls'|'http'|'content'; label:string; state:'pending'|'passed'|'failed'; detail?:string; }
interface Verification { attempt:number; nextRetryAt?:string; deadlineAt:string; errorClass?:VerifyClass; message?:string;
  checks:VerifyCheck[]; lastCheckedAt?:string; visibleFromThisPc?:boolean; nameservers?:string[]; }
interface Deployment { id:string; siteId:string; hostname?:string; state:DeploymentState; steps:DeploymentStep[]; verification?:Verification; error?:string; startedAt:string; finishedAt?:string; }
interface DnsPlan { hostname:string; action:'create'|'update_own'|'conflict'; existing?:{type:string;content:string}; message:string; }
interface SystemStats { cpuPercent:number; ramUsedBytes:number; ramTotalBytes:number; diskUsedBytes:number; diskTotalBytes:number; hostifyDataBytes:number; }
interface TunnelHealth { tunnelId:string; name:string; connected:boolean; connections:number; lastChange:string; }
interface ActivityEntry { id:number; ts:string; level:'info'|'warn'|'error'; siteId?:string; message:string; }
interface AppSettings { launchOnLogin:boolean; startEngineOnBoot:boolean; closeToTray:boolean; theme:'system'|'light'|'dark'; basePortStart:number; }
interface ComponentStatus { installed:boolean; ready:boolean; version?:string; path:string; error?:string; }
interface EngineStatus { running:boolean; version:string; mode:'user'|'service'; caddyVersion?:string; cloudflaredVersion?:string; componentsReady:boolean;
  localHostingReady?:boolean; caddy?:ComponentStatus; cloudflared?:ComponentStatus; dataDir?:string; binDir?:string; }
interface SourceReport { ok:boolean; fileCount:number; sizeBytes:number; hasIndex:boolean; warnings:string[]; errors:string[]; }
interface DomainCheck { zoneName:string; zoneId:string; canManageDns:boolean; problems:string[]; }
```

## Commands

| command | args | returns |
|---|---|---|
| engine_status | - | EngineStatus |
| ensure_components | - | EngineStatus (downloads and verifies caddy + cloudflared; emits `components-progress` {name,percent}) |
| system_stats | - | SystemStats |
| list_sites | - | Site[] |
| get_site | {id} | Site |
| validate_source | {path, kind:'directory'\|'zip'} | SourceReport |
| add_site | {name, sourcePath, kind} | Site (copies/extracts safely into the managed dir) |
| update_site_files | {id, sourcePath, kind} | Site (atomic swap, keeps previous release) |
| start_site / stop_site / restart_site | {id} | Site |
| remove_site | {id, deleteFiles} | void |
| preview_url | {id} | string (`http://127.0.0.1:<port>`) |
| cf_connect | {token} | CloudflareAccount (verifies; stores in Credential Manager) |
| cf_status | - | CloudflareAccount |
| cf_disconnect | - | void |
| cf_check_domain | {hostname} | DomainCheck |
| cf_plan_dns | {siteId, hostname} | DnsPlan |
| publish_site | {siteId, hostname, consentDns} | Deployment (async; progress via `deployment-update`) |
| get_deployment | {id} | Deployment |
| list_deployments | {siteId} | Deployment[] |
| retry_verification | {id} | Deployment (non-destructive; only from waiting_*, verifying_public or recovery_required; restarts the budget and checks now) |
| cancel_deployment | {id} | Deployment (before resources exist or during provisioning: rolls back what this deployment created; during verification: stops checking, keeps every Cloudflare resource, state becomes recovery_required) |
| unpublish_site | {siteId} | void (removes only the DNS record + ingress it created; also valid from waiting_*, verifying_public and recovery_required) |
| list_domains | - | Domain[] |
| tunnel_health | - | TunnelHealth[] |
| list_activity | {siteId?, limit} | ActivityEntry[] |
| site_logs | {siteId, lines} | string[] |
| get_settings / save_settings | - / AppSettings | AppSettings |
| pick_path | {kind:'directory'\|'zip'} | string \| null (native dialog) |

Events: `site-status` (Site), `deployment-update` (Deployment), `activity` (ActivityEntry), `components-progress` ({name, percent}).

## Runtime and environment IPC (Task 11)

`Site` additionally exposes `kind: string` (existing websites default to `static`) and `runtimeVersion: string | null`. The import argument `kind: 'directory'|'zip'` retains its existing meaning.

| command | args | returns |
|---|---|---|
| list_runtimes | - | `{kind,version,installed,ready,error?,sizeBytes?,path?,usedBy:string[]}[]` |
| install_runtime | `{kind,version}` | null after successful installation; errors are plain text |
| remove_runtime | `{kind,version}` | null; refuses website use or existing MySQL data |
| list_env_vars | `{siteId}` | `{name,secret,value?}[]`; secret values are always omitted |
| set_env_var | `{siteId,name,value,secret}` | `{restartRequired:boolean}` |
| delete_env_var | `{siteId,name}` | `{restartRequired:boolean}` |
| reveal_env_var | `{siteId,name}` | one explicit value as a string |
| import_env | `{siteId,text}` | `{imported:number,errors:{line:number,message:string}[],restartRequired:boolean}` |
| export_env | `{siteId}` | dotenv string with secret names in an omission comment |

Event: `runtime-progress {kind,version,phase:'download'|'verify'|'extract'|'check',done:number,total:number|null}`. `done` and `total` are bytes for download/verify/extract, and 0/1 for the executable check; an unknown total is null. Runtime sizes are measured installed bytes or absent, never the manifest's planning estimates. Installs hold only their runtime's lock; other pipe sessions and unrelated engine requests remain available. A second operation for the same runtime receives an already-in-progress error. Runtime installs complete independently of later requests on the same shared pipe; other commands keep their existing ordering. A disconnected caller does not cancel an engine-owned install. The shell retains its existing 120-second request timeout, so after that timeout refresh the runtime list; the installation continues in the engine.

Environment edits require a restart when a non-static website has an active or unhealthy running state (`starting`, `running`, or `error`). An import with no successful assignments reports false. Dotenv supports `export`, whitespace/comments, literal single quotes, double quotes and escaped newlines; errors report line numbers without source text or values. Import commits valid assignments and reports invalid lines individually. Names with SECRET, TOKEN, PASSWORD, PASS, KEY, PRIVATE or CREDENTIAL default to secret. Explicit reveal is the only command that returns a secret. A restored secret that cannot decrypt names the variable and asks the user to set it again under the current Windows user. Environment commands do not append their parameters or values to activity/logs.

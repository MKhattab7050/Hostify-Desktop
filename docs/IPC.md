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

## Node hosting and routing IPC (Task 12)

The source argument `kind: 'directory'|'zip'` keeps its V1 meaning. `add_site` additionally accepts optional `siteKind: 'static'|'node'|'php'` (default `static`) and `config: AppConfig`. PHP is detected but hosting is refused until Task 13. A Node import allocates distinct public/internal ports and never installs or starts code. Updates import a pending immutable release; the active app continues serving during installation/build.

```ts
interface AppConfig {
  install: 'ci'|'install'|'none'; buildScript: string|null;
  start: {type:'npmScript';name:string}|{type:'node';args:string[]};
  workingDir: string; healthPath: string; restart:'always'|'onFailure'|'never';
  memoryLimitMb: number|null; allowInstallScripts:boolean; internalPort:number|null;
  activeRelease:string|null; previousRelease:string|null; pendingRelease:string|null;
  builtRelease:string|null; trustedRelease:string|null;
}
```

Every configuration field has a serde default. Defaults are install=`install`, start=`npmScript/start`, workingDir=`.`, healthPath=`/`, restart=`always`, install scripts disabled, and optional values null. Inspection suggests `ci` for package-lock, a build script when available, and an appropriate Node entry. Website kind and Node version remain the website's `kind` and `runtimeVersion` columns, outside the configuration JSON. Clients cannot replace engine-managed port/release fields through `set_app_config`.

| command | args | returns |
|---|---|---|
| inspect_project | `{path,kind:'directory'|'zip'}` | `{siteKind,framework,name,engines,recommendedRuntime,scripts,lifecycleScripts,dependenciesWithInstallScripts,lockfile,hasServerEntry,dotenvNames,usesDatabase,suggested,warnings}` |
| get_app_config | `{siteId}` | `AppConfig` |
| set_app_config | `{siteId,config,nodeVersion?}` | `{restartRequired,rebuildRequired}` |
| build_app | `{siteId,trustAcknowledged:true}` | `{buildId}` immediately; build continues independently of the caller |
| cancel_build | `{siteId}` | null after the process tree ends and any promotion rollback completes |
| get_build_status | `{siteId}` | latest `app-build` payload, or null before a build |
| app_status | `{siteId}` | `{pid?,processCount?,cpuPercent?,memoryBytes?,peakMemoryBytes?,restarts?,lastExitCode?,lastExitAt?,internalPort?,activeRelease?,health,exposedAddresses}` |
| app_logs | `{siteId,stream:'app'|'build'|'access',lines}` | rotated log tail, at most 5000 lines |

Event `app-build`: `{siteId,buildId,step:'install'|'build'|'start'|'verify',state:'running'|'done'|'failed'|'cancelled',detail,exitCode:number|null}`. Step completion does not by itself mean the whole task is complete. A running website promotes a successfully built pending release, verifies it through Caddy, and restores the previous release if startup fails. A first deployment remains stopped until `start_site`. A reopened engine marks interrupted build work failed and explains that the release must be built again. Build metadata is durable in SQLite. Concurrent same-site builds and conflicting site operations are refused.

Inspection executes no project code, limits each parsed file to 2 MiB, and returns dotenv names only. Root `.env` is excluded from app imports; the UI must explicitly submit its contents through `import_env` for encrypted secret storage. Project code runs as the signed-in Windows user after explicit trust acknowledgment. npm install lifecycle scripts run only when `allowInstallScripts` is enabled.

The engine emits app-build on its pipe. The existing desktop reader whitelist in src-tauri/src/engine.rs does not forward that event to Tauri; that file is outside Task 12 ownership. Desktop consumers currently need get_build_status, pending the shell integration change.


## PHP hosting IPC (Task 13)

PHP imports and updates now use the same immutable releases, explicit trust, build events, promotion and rollback as Node. `build_app` acknowledges trust and marks a PHP release built without running Composer, npm, install hooks or build scripts. Code runs as the signed-in user. `set_app_config` accepts `phpVersion` for PHP sites and checks the pinned runtime manifest; `nodeVersion` retains its Node meaning. Both optional version arguments are forwarded by the desktop shell. The desktop event reader forwards `app-build` as well as `runtime-progress`.

Additional defaulted `AppConfig` fields:

```ts
interface PhpAppConfigFields {
  documentRoot: string;             // default ".", relative to release
  phpWorkers: number;               // default 4, range 1..16
  phpExtensions: string[];          // mbstring, openssl, curl, fileinfo, mysqli, pdo_mysql
  phpSettings: Record<string,string>; // memory_limit, upload_max_filesize,
                                     // post_max_size, max_execution_time only
  persistentPaths: string[];        // default [], applies to Node too
  phpPorts: number[];               // engine managed, default []
}
```

Extension names are restricted to the task's allowlist and must have a DLL in the selected runtime's ext directory. Size settings accept digits with an optional uppercase K/M/G suffix; max_execution_time accepts digits only. Relative document and persistent paths use the release's lexical and canonical containment checks, including junction checks. Inspection suggests public when only public/index.php exists, identifies WordPress from wp-config-sample.php, suggests wp-content and wp-config.php persistence, checks simple Composer require.php constraints against pinned PHP versions, reports database use, and warns about Composer dependencies without vendor. Inspection runs no project code.

For PHP, `app_status` additionally returns `workers: {port:number,pid:number|null,health:string}[]`. `processCount`, `memoryBytes`, `cpuPercent`, `restarts`, and the most recent `lastExitCode`/`lastExitAt` aggregate the pool. `activeRelease` identifies the promoted release; there is no single PHP internalPort or pid. Statistics are sampled only on request. A worker's crash-loop error remains visible even while the other workers and Caddy respond.

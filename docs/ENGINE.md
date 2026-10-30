# Hostify engine implementation status

The earlier core pass hardened `hostify-core` deployment and import only. Its offline workspace build confirmed compatibility with the core APIs. The Task 5 engine changes and validation are described below; these do not certify a fully installed end-to-end hosting product.

## Deployment journal and rollback

Deployment persists its request, ownership information, and create intent in SQLite before each Cloudflare POST. New tunnels use the stable site name `hostify-<siteId>`, recorded before creation; its returned ID is saved before fetching its token. DNS creation persists an intent first and adds the `managed-by=hostify` comment marker plus a `hostify-deployment:<deployment UUID>` DNS tag. The returned DNS ID is saved immediately. Tokens remain in the secret store, outside the journal.

Recovery looks up ambiguous tunnel creates by their recorded site name (or legacy deployment name) and DNS creates by hostname, management marker, and deployment tag. It rolls back interrupted attempts and retries failed attempts whose cleanup is incomplete. Successful cleanup operations are checkpointed. A shared hostname or management marker alone is never proof that this deployment created a resource.

Hostify uses remotely managed tunnels exclusively: `config_src:"cloudflare"`, ingress written through the configurations API, and `cloudflared tunnel --no-autoupdate --metrics 127.0.0.1:<port> run` with `TUNNEL_TOKEN` in the environment only. There are no local tunnel config or credentials files. Tunnel lookup paginates and ignores deleted tunnels. Existing live journals with legacy physical names continue to be reused.

A newly created tunnel has no configuration snapshot: deployment skips the GET and journals `previous_config:null`. Config reads return `Option<TunnelConfig>`; null, missing, empty, or ingress-less configuration and HTTP 404 mean `None`. An ingress array produces `Some`; malformed shapes produce `Unexpected`, never `Network`. Transport failures produce `Network`; unsuccessful API responses retain HTTP status, Cloudflare code and message, with distinct friendly authentication, permission, rate-limit, and not-found descriptions.

For a reused tunnel, deployment snapshots the full config, preserves existing hostname entries and origin options, places the current hostname first with `http://127.0.0.1:<port>`, and always finishes with `http_status:404`. The intended config is durable before PUT. On rollback, reused tunnels are never deleted. If current config still matches the intended config, rollback restores the full snapshot. With concurrent changes it removes only the exact entries added by this attempt and restores a replaced hostname entry when safe. A `None` snapshot means removing our hostname entry and keeping a terminal 404. A failed reused-config read/write leaves cleanup pending for retry.

For a tunnel created by this deployment (journaled `tunnel_created` and matching `owner`), rollback stops that exact connector and deletes that tunnel without any config GET. Missing tunnels count as cleaned up. Cloudflare active-connection rejection (code 1022 or HTTP 409) triggers DELETE of that tunnel's `/connections`, then retries tunnel deletion. Cleanup does not infer ownership from ingress contents. Unrelated tunnels and reused tunnels are never deleted.

DNS cleanup checks both the management comment and deployment ownership tag before DELETE; missing records count as success. Reused DNS is never adopted for deletion. An owned record repointed by this attempt has its previous writable fields journaled before PUT and restored on rollback if its ID and target still match our update. The desired DNS target remains durable even after tunnel deletion, allowing a failed DNS restore to succeed on later recovery. Connector and local-site stop calls otherwise apply only to resources this attempt started. Remote cleanup failures do not prevent attempts to stop local resources. Every successful cleanup is checkpointed; incomplete cleanup retains `rollback_pending:true`.

Startup invokes `recover_interrupted()` before desired-state reconciliation. A background worker then retries durable pending cleanup every 30 seconds, serialized with publishing, including failures from new publishes during that engine session. A new same-site publish is refused while older cleanup remains pending, so it cannot adopt a tunnel still scheduled for deletion. Recovery of failed rows with `tunnel_created:true`, a saved tunnel ID, `previous_config:null`, and `dns_id:null` no longer depends on configurations being readable. Offline fixtures mirror the two reported orphan rows using fake IDs; they assert exact deletion and a no-op second recovery. The actual account cleanup is performed by the orchestrator's real-Cloudflare run.

No DNS consent means zero Cloudflare calls. DNS conflicts detected during preflight cause no writes or local starts; planning is checked again before tunnel configuration and DNS creation. Existing ingress is merged and the final catch-all normalized to 404. Re-publishing a live site and hostname reuses its journaled physical tunnel, repeats validation and public verification, and does not create another tunnel or DNS record. A previously saved `live` snapshot alone is not treated as successful verification.

Public verification retains the production HTTPS fetch and index-body SHA-256 comparison, with a bounded retry deadline. `SiteRuntime::verify_public` supplies a default production implementation and an offline test seam. Existing public method signatures and engine-facing journal fields remain compatible. Additional Cloudflare trait methods have fail-closed defaults for older adapters; those adapters must implement the journaled operations to deploy safely. The production runtime waits for `/ready` and a positive `cloudflared_tunnel_ha_connections` metric before reporting connector startup success; partial starts are stopped on failure.

## DNS and sanitized Cloudflare API log (Task 7)

Apex hostnames (for example `niga.online`), `www`, and deeper labels use the same proxied CNAME target `<tunnel-id>.cfargotunnel.com`; Cloudflare apex flattening requires no special A record. Zone lookup walks suffixes starting with the complete hostname, including the apex itself. Foreign A, AAAA, CNAME, and conflicting TXT records cause conflict before writes or local starts. A `managed-by=hostify` CNAME pointing at the same tunnel is reused without a DNS write. A stale managed CNAME is `update_own`, requires DNS consent, and saves its old contents for rollback. To retain the existing IPC action union, same-target plans also use `update_own` with an explicit reuse message; deployment detects the matching target and does not write. Create/update GET readback requires the exact tunnel target and `proxied:true` before connector startup.

All production engine Cloudflare clients, including account connection and startup/retry recovery, append sanitized request metadata to `<dataDir>/logs/cloudflare.log` (normally `%LOCALAPPDATA%\Hostify\logs\cloudflare.log`). It shares the engine log rotation: 5 MiB per file, three numbered backups. Each line contains UTC timestamp, HTTP method, API path (resource IDs allowed), HTTP status (`0` for transport failure), provider error codes/messages, and elapsed milliseconds. It never writes Authorization headers, API tokens, tunnel tokens, request/response bodies, or `/token` results. Known credentials are redacted even if echoed in provider error messages; CR/LF are removed from logged fields. A wiremock regression fabricates both credentials and asserts neither appears anywhere in captured logs. The optional client hook emits one line per completed request; process termination or future cancellation may prevent a completion log.

Offline Task 7 coverage additionally exercises unconfigured null/empty/missing configurations through fresh and reused publishes, exact ingress PUT bodies with final 404, failure at every publish step while config GET would return 500, paginated tunnel lookup ignoring deleted entries, 401/403/429/404 and malformed JSON classification, transport failure, active-connector deletion cleanup, DNS readback validation, and two-orphan recovery idempotence. Task 7 final validation: `cargo test --workspace --offline` passed 69 core tests, 1 shell resolver test, 6 engine unit tests, and 6 engine integration tests (82 total, zero failures). `cargo build --workspace --offline` passed. No new crates or external network access were used; only `crates/**` and this document were edited. Existing conditional symlink privilege skips described below still apply.

## Import hardening

Imports retain only the newest three previous releases, ordered by their numeric timestamp names. Unsafe release entries and existing links or reparse points in managed destination ancestors are rejected.

On Windows, directory traversal retains non-delete-shared directory handles during recursion. Source files, including ZIP archives, are opened with `FILE_FLAG_OPEN_REPARSE_POINT`; metadata from the opened handle must identify a regular, non-reparse file with the scanned size. File handles deny write/delete sharing while copying. Size limits are applied after opening and the copied byte count is checked. These checks reduce scan/open replacement races; they are not a complete filesystem security audit. In particular, destination ancestor validation is still path based and is not an atomic defense against hostile concurrent changes. ZIP edge cases and other platform behavior need further adversarial review.

## Covered by offline tests

The core suite uses a fake `SiteRuntime`, in-memory SQLite, and wiremock Cloudflare on loopback. It covers:

- Full seven-step success ending `live`; a separate failure at every step with exact DELETE and local-stop assertions, preserving unrelated DNS and tunnels.
- No-consent refusal with zero Cloudflare requests; apex A/AAAA/CNAME/TXT conflicts without writes; stale managed CNAME repoint, rollback restore, and subsequent no-write reuse.
- Propagation failure ending `failed`, reused DNS preservation, full reused-ingress restoration, and already-running local resources remaining running.
- An ingress write that takes effect but returns an error; concurrent additions on a reused tunnel preserved. Created tunnels now follow journal ownership for deletion, independent of config reads.
- Tunnel and DNS creation interrupted before ID save, both as seeded crash snapshots and by cancelling real mock HTTP POSTs after remote creation while their response is delayed.
- Incomplete failed rollback retried by recovery, repeat recovery doing nothing, and another deployment's tagged DNS being preserved.
- Re-publish without duplicate creates, and failed re-verification preserving previously live resources.
- Release retention, post-scan size changes, Windows source handle sharing restrictions, directory junction rejection, managed destination junction rejection, and file-symlink replacement rejection when Windows permits creating the test link.

Validation on this Windows machine: 42 core tests pass; the file-symlink test exits early with an explicit `SKIP` message because creating a symlink fails with privilege error 1314. Junction and handle-sharing tests execute and pass. Rust reports conditional skips inside test bodies as passed rather than ignored. `cargo build --workspace --offline` also passes. No new crates or external network access were used. Rustfmt is not installed for the active toolchain.

Commands use `%USERPROFILE%\.cargo\bin` on PATH:

```text
cargo test -p hostify-core --offline
cargo test -p hostify-core --offline -- --nocapture
cargo build --workspace --offline
```

## Still unverified against real services and processes

No operation in this pass was verified against real Cloudflare, release downloads, Windows Credential Manager, installed Caddy or cloudflared, reboot, Windows Service, scheduled tasks, or installer behavior. In particular, Cloudflare token scopes, DNS tag acceptance, unconfigured-tunnel configuration responses, list visibility immediately after creates, ingress restoration behavior, and real HTTPS propagation remain integration checks before release. The mock tests do not exercise real TLS or the production HTTPS retry loop.

Cloudflare calls are not a distributed transaction: another account actor can change configuration between a read and a write or delete. The client has no provider-side compare-and-swap or transaction primitive; preserving changes already visible during rollback does not eliminate that concurrent-update window. Recovery assumes successful Cloudflare list responses accurately expose committed creates. Stable-name ambiguous-create recovery also assumes same-site publish/recovery operations are serialized; an external actor creating the identical site name during an interrupted POST is outside that ownership proof. Historical deployment snapshots made before create intents existed cannot reconstruct IDs that were never saved.

`SiteRuntime` must report `true` only when it actually starts a resource, and must clean up partial starts before returning an error. A process death between a local start taking effect and its boolean result being saved is outside the remote-create journal guarantee; the engine must reconcile its own child processes. Core cannot identify an unknown start through the existing runtime API. Callers must serialize publish/recovery operations for the same site or shared tunnel; this pass adds no cross-process deployment lock.

Component acquisition now uses compiled-in pinned versions and publisher hashes as described below. Real release downloads remain unverified. Port selection remains advisory and can race with another process.


## Component detection and startup (Task 5)

The pure resolver in hostify-core/src/paths.rs is the single source for argument/environment precedence and executable names. It resolves --data-dir first, then HOSTIFY_DATA_DIR, then %LOCALAPPDATA%\Hostify, deriving bin from that root. The engine makes its root absolute. Scheduled tasks retain their explicit root in XML; service launch arguments retain the same root plus owner SID, so the service does not fall back to LocalSystem's environment.

The shell still spawns the engine beside its executable (or target/debug in development) without passing a data-dir override, inheriting the environment. It compiles the same resolver source to compare the connected engine's dataDir echo against its expected root and logs discrepancies to stderr. It does not redirect the existing per-user pipe to another root.

engine_status retains componentsReady and legacy version fields and adds:

- caddy and cloudflared: {installed, ready, version?, path, error?}.
- dataDir, binDir, and localHostingReady.
- localHostingReady equals Caddy readiness; componentsReady requires both components for publishing.

Installed means a regular file exists. Ready also requires a successful version command (version for Caddy, --version for cloudflared); empty files are rejected without execution. Probes run through spawn_blocking, have a five-second process deadline, and capture bounded stdout. Status results are cached for about two seconds, keyed by each executable's existence, length, and modified time. External replacement, emptying, or deletion is reflected on the next engine_status call; start preflight and installation checks bypass the cache. Each install attempt invalidates the cache immediately, including failure paths.

ensure_components accepts optional {"names":["caddy"]}; omitted/null names default to both. It validates names before downloads, preserves ready binaries, and reinstalls missing, empty, corrupt, or non-running files. Progress {name,percent} is emitted per selected component. Downloads and expanded executables are limited to 256 MiB. Verification remains fail-closed: the single PINNED_COMPONENTS table in crates/hostify-core/src/components.rs pins Caddy v2.11.7 and cloudflared 2026.10.6, their asset filenames, direct release download URLs, and lowercase SHA-256 hashes; Caddy also requires its published SHA-512. All listed hashes must match before any file is written. Installation makes no GitHub API request, latest-release lookup, or release-note/checksum-list parsing. To bump a pin: download the intended asset, compute its hashes, verify them against the publisher's checksum list, then update the version, filename, URL, and all hashes in that table. After replacement, the version probe must succeed and its output must contain the pinned version; installation returns the binary's reported version and errors with a warning on a version mismatch. Only the exact root ZIP entry caddy.exe is read; archive paths are never used as filesystem destinations. Verified bytes are written to a unique temporary file beside the target and renamed over it. Download, checksum, extraction, and replacement failures clean up any temporary file and preserve the prior target. A post-install probe error is reported after the verified executable has replaced the target. Locked targets advise stopping websites/tunnels and retrying. Offline and GitHub rate-limit errors explain how to retry.

Starting/restarting a local site checks only Caddy, before stopping/replacing a running site or spawning. Failure sets statusDetail to the Dashboard installation instruction. The production supervisor repeats this guard for startup reconciliation and automatic child restarts. A file-not-found spawn race gets the same instruction. Publishing names exactly the components that are not ready before any Cloudflare work.

The CLI status prints the connected engine's data/bin directories and each component's readiness, version, and error. Startup logs Hostify engine started (data: <dir>) before reconciliation.

Offline regression coverage includes resolver precedence, service/task root preservation, the readiness matrix with fake probes, metadata cache refresh/expiry/invalidation, reinstalling zero-byte Caddy through mocked HTTP, preserving ready binaries, checksum failure preserving existing bytes, locked-target cleanup on Windows, traversal ZIPs, real corrupt-executable rejection, local start/restart without cloudflared, missing-Caddy refusal without runtime calls, publish preflight, selective install validation, and the new fields over the real named-pipe transport.

Real GitHub downloads, installed Caddy/cloudflared behavior, elevated service/task installation, and installed-app stderr collection remain unverified in this offline pass. Local hosting success tests use the existing fake runtime with an injected version runner.

Task 5 validation on this Windows workspace: cargo test --workspace --offline passed 52 core tests, 1 shell resolver test, 6 engine unit tests, and 5 engine integration tests (64 total, zero failures). cargo build --workspace --offline passed. git diff --check passed for the owned paths. Existing conditional symlink privilege skips described above still apply. No new crates or external network access were used.


Task 7 validation output excerpts (2026-10-30, this workspace):

```text
cargo test --workspace --offline
    Finished `test` profile [unoptimized + debuginfo] target(s) in 10.92s
test cloudflare::regression_tests::api_log_redacts_api_and_tunnel_tokens_including_provider_errors ... ok
test cloudflare::regression_tests::tunnel_delete_cleans_active_connections_then_retries_and_missing_is_success ... ok
test deploy::tests::failed_connector_stop_keeps_owned_tunnel_pending_until_retry ... ok
test deploy::tests::failed_dns_restore_retries_after_owned_tunnel_has_been_deleted ... ok
test deploy::tests::recovers_two_failed_orphans_with_null_snapshot_idempotently ... ok
test deploy::tests::fresh_null_empty_and_missing_config_publish_apex_to_live ... ok
test deploy::tests::tagged_stale_dns_success_repoints_then_republish_has_no_dns_write ... ok
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
cargo build --workspace --offline
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 14.73s
```

The orchestrator must still verify actual cleanup of the two account tunnels, real Cloudflare configuration response variants, DNS tags and apex flattening, the active-connection deletion response code, cloudflared readiness/metrics, and public HTTPS propagation. This offline pass did not access the user's live Cloudflare account or modify the real deployment database.

DNS create and update requests omit `tags` for Cloudflare Free accounts. Ownership comes from SQLite deployment journals and tunnel IDs, never DNS labels: the planner reuses an exact tunnel target, permits consented repointing only from locally recorded Hostify tunnels, and rejects other A/AAAA/CNAME/TXT records. Cleanup rechecks the journaled record ID and CNAME tunnel target; crash recovery adopts an ID only for an exact hostname targeting the tunnel that deployment created. Comments remain informational, and rollback restores prior content without sending tags.

## Publishing state machine (Task 9, supersedes the rollback-on-verify behaviour above)

The IPC types and command arguments are defined in `docs/IPC.md`. Publishing journals its request, ownership, remote create intents, and a random per-deployment nonce before taking the corresponding actions. Provisioning progresses through `validating`, `starting_local`, `provisioning_tunnel`, `configuring_dns`, and `starting_connector`. Failures or cancellation in this phase roll back only this deployment's changes; incomplete cleanup remains durable for recovery. Schema version 2 migrates legacy `live` rows and their JSON snapshots to `published`; legacy `running` rows remain interrupted provisioning work.

Starting `verify_https` is the retention boundary. The engine releases the provisioning lock and runs one verifier task per deployment. Attempts collect ordered zone, journaled proxied-CNAME, connector, public-DNS, TLS, HTTP, and nonce-content evidence through `PublicProbe`. Each external observation is bounded by ten seconds. DNS observations use authoritative UDP, Cloudflare and Google DNS-over-HTTPS, and the OS resolver independently. Up to four edge addresses, IPv4 first, are pinned to the HTTPS hostname with normal certificate validation and redirects restricted to the same HTTPS host and port. Publishing requires the exact nonce at the marker URL and a successful root response; root HTML is never hashed.

DNS classes select `waiting_dns`, certificate/handshake failures select `waiting_tls`, and other failures select `verifying_public`. Attempts persist their checks, class, PC visibility, timestamps and retry schedule, then emit `deployment-update`. The retry base doubles from five seconds to sixty seconds with +/-20% UUID-derived jitter. The thirty-minute budget ends in `recovery_required`. Verification failure, expiry and cancellation retain DNS, tunnel, connector and local server. An authoritative/public edge success can publish despite a stale negative answer on this PC; its message explains that the cached answer clears itself.

`retry_verification` restarts the budget and wakes an existing verifier, or starts one if none is running. `cancel_deployment` first stops its task, then either rolls back provisioning or marks verification `recovery_required`. Another publish for a site with retained resources points to retry or explicit unpublish. `unpublish_site` stops verification before ownership-checked DNS/ingress cleanup, stops the connector, removes the nonce and leaves the local site and physical tunnel available. Cleanup failures are retried durably. Removing a site with a waiting, recovery or published deployment is refused.

The marker lives at `<data>/run/verify/<siteId>/<nonce>`, outside public website files. Caddy serves only the verification prefix from that directory with `Cache-Control: no-store`; its fallback still blocks dot paths and directory listing. The marker is removed on publish, explicit unpublish and rollback. Waiting domains have pending DNS and unverified HTTPS; published domains are configured and verified. A site's public URL requires verified HTTPS, and its publish fields describe its latest deployment other than `rolled_back`. Local server status remains independent of verification waits. Activity records are written once per state transition.

Startup rolls back interrupted provisioning, starts local sites and connectors for all retained/public deployments regardless of `auto_start`, and resumes waiting verification with a fresh attempt. A future deadline is preserved; an expired deadline receives a new budget. `recovery_required` retains its resources without automatically resuming checks.

Offline regressions use fake observations, SQLite and loopback Cloudflare fixtures. The installed Caddy binary is also validated and exercised for marker serving, hidden paths and disabled listing; when absent, that test prints a skip reason. Live Cloudflare, public DNS, production TLS and real cloudflared connectivity require external verification and are not certified by these tests.

## Runtime manager, environment vault, and supervisor groundwork (Task 11)

Schema v4 adds website kind, runtime version and app-config JSON with static-compatible defaults, plus `env_vars`. The v4 alterations and table creation share one transaction; constraints require exactly one plaintext/ciphertext value and enforce the secret flag. Website removal explicitly deletes environment rows before deleting the website. The v3 migration regression preserves a website and a published deployment.

The runtime manifest pins exactly the five Node/PHP/MySQL releases and SHA-256 hashes in V2-PLAN.md. Production downloads require HTTPS, including redirects. PHP/MySQL archive URLs follow a 404 or transport error; integrity failure never selects a fallback. Per-entry download limits, installed-space estimates and a 100 MiB safety margin feed a Win32 data-volume free-space check. These are conservative planning bounds, not measured installed sizes. Streamed downloads hash each chunk while writing `.partial/<kind>-<version>.zip`; extraction starts only after hash verification. ZIP extraction shares import's path validation, rejects links and duplicate paths, limits entry count and expanded bytes, and strips a sole wrapper directory when present (PHP's unwrapped ZIP stays unwrapped).

Installation probes the expected executable with a five-second, bounded-output, hidden-window version command and checks the pinned version (PHP excludes the packaging suffix). Missing VCRUNTIME140 or Windows loader exit code 0xC0000135 identifies the absent Microsoft Visual C++ runtime and links Microsoft's x64 redistributable; Hostify does not install it. A validated staging directory is renamed into place; an old version directory is moved aside first and restored if promotion fails. Startup clears `.partial` before requests are accepted. Failed attempts clean staging/download files and preserve the final installation. Runtime operations serialize per manifest entry and do not hold publishing/operations locks. The pipe continues reading requests and sending progress while installs run. Installs belong to the engine background-task set, so losing a caller does not release their lock or interrupt extraction; engine shutdown ends those tasks, and startup removes interrupted partial files. Installed sizes are measured in blocking workers and cached for 60 seconds, invalidated after install/remove. `resolve` validates kind/version before constructing paths. Removal names website IDs using a version; MySQL also refuses existing managed data roots (`mysql/data`, `mysql/<version>/data`, `databases/mysql`, or installation-local `data`). Future database work must retain this guard when choosing its data layout.

The vault calls user-scoped CryptProtectData/CryptUnprotectData with fixed Hostify entropy and UI forbidden. Returned native allocations are cleared and released with LocalFree; owned intermediate buffers and resolved secret strings clear themselves where practical. Non-Windows vault calls fail explicitly. There is no plaintext fallback if DPAPI is unavailable. Environment values are bounded to 64 KiB and reject NUL; identifiers obey the documented ASCII rule and reject Hostify-controlled names. Lists and exports omit secret values, while export comments name omitted variables. The engine-only process-resolution/redaction helpers return strings that clear their owned bytes on drop. Pipe and shell transport were inspected: neither logs request parameters for environment commands; import/parser/storage errors do not include values.

Supervisor specifications support cwd, explicit environment and a redaction set. Any cwd or explicit environment selects env_clear plus the task's Windows allowlist, followed by the specification's values. Legacy empty-spec Caddy/cloudflared environment behavior stays compatible. Log pumps replace token and redaction strings, longest first. Five consecutive exits within 30 seconds of starting end automatic restarts with the last exit code in site status/activity; a stable run resets the streak and manual start creates a fresh worker. Connectors keep retrying indefinitely. Legacy Caddy for retained/published websites is also exempt so the limit cannot strand a public static site. A real hidden `cmd.exe /C exit 7` fixture exercises the limit and manual restart using temporary engine data, without starting a Hostify engine or touching live installation data.

Offline tests cover fixture runtime install and measured sizes, bad hash with no fallback/preservation of the old install, 404 archive fallback, truncated HTTP, traversal, partial cleanup, same-runtime serialization, unknown versions, low disk space and removal guards. Windows DPAPI round-trip/tamper and SQLite/pipe secret-at-rest tests are present and intentionally fail rather than silently skip when the executing account lacks DPAPI support. Validation results and execution limitations follow below.

### Task 11 validation on 2026-10-30

Actual command summaries from this Windows workspace (all Cargo compilation/test commands offline):

```text
cargo fmt --all
cargo fmt --all exit code: 0

cargo build --workspace --offline
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.20s
EXIT CODE: 0

cargo test --workspace --offline
    Finished `test` profile [unoptimized + debuginfo] target(s) in 15.16s
test result: FAILED. 106 passed; 2 failed; 3 ignored; 0 measured; 0 filtered out; finished in 0.48s
EXIT CODE: 101

cargo test --workspace --offline --no-fail-fast
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.79s
test result: FAILED. 106 passed; 2 failed; 3 ignored; 0 measured; 0 filtered out; finished in 0.46s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 33.38s
test result: FAILED. 8 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.74s
error: 2 targets failed:
EXIT CODE: 101

cargo clippy --workspace --all-targets --offline
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.83s
EXIT CODE: 0
```

The no-fail-fast run executes every test target: 140 passed, 3 failed, 3 ignored (plus empty binary/doc-test targets). Failures are `vault::tests::round_trip_and_tamper`, `env_vars::tests::secret_is_ciphertext_and_never_listed`, and `environment_secrets_are_not_in_pipe_errors_or_activity`. Each stops at real user-scope DPAPI sealing with `The system cannot find the file specified. (os error 2)`. An independent .NET `ProtectedData.Protect(...CurrentUser)` probe also fails and reports that the executing user context may lack a loaded profile. These tests remain enabled; this is not a passing definition of done. Real DPAPI round-trip, tamper rejection and secret-at-rest/reveal success must be rerun in a Windows user context with working DPAPI. The nonsecret/decryption-error regression, all fixture runtime tests and the real short-lived-child test pass.

Clippy reports existing warnings (core lib 13 / tests 14, desktop 4, engine lib 9 / tests 10), with no errors and no diagnostics in the new runtime/environment/vault modules. No broad lint cleanup was attempted. `git diff --check` passes. Workspace formatting was executed; unrelated/protected source formatting was preserved to respect file ownership. No new dependencies or lockfile changes remain. Existing documentation is preserved as an exact normalized prefix, with only Task 11 sections appended.

Real publisher downloads, real Node/PHP/MySQL executables, the Visual C++ loader-failure path and full installed-app flows were not exercised. Production HTTPS/redirect enforcement is implemented; fixture HTTP tests do not certify real TLS. Final test commands set LOCALAPPDATA to an isolated workspace test path so the existing optional real-Caddy test skips its installed-binary lookup; three existing live-network tests remain ignored. Windows import tests may retain their pre-existing privilege-dependent symlink skip. No real Hostify engine was started/stopped, no live installation data was modified, and no commit, push or branch switch was performed.

## Node application hosting (Task 12)

Node projects use immutable `<data>/sites/<id>/app/<releaseId>` directories. Imports share static import traversal, link, duplicate, size and ZIP checks; they omit `.git` and `node_modules` at every depth and root `.env`. The root dotenv values are never copied into application releases. Inspection is passive and returns only dotenv names. Static sites retain the existing `current` layout, static-only index check for publishing, and byte-identical Caddy output (covered against a V1 literal). V1 deployment request JSON defaults its new kind field to static.

Application configuration is camelCase JSON in `websites.app_config_json`; kind and Node version retain their existing columns. Relative working directories and path arguments reject traversal, drive/UNC prefixes and NUL, and canonical checks reject junction escapes. Build/start script names must exist in package.json and are individual argv entries. Only `{PORT}` is substituted. Node entry paths are canonical files inside the release at startup, including entries installed by npm.

Builds belong to the engine, return a build ID immediately, persist their latest step/state in SQLite and emit `app-build`. They do not hold the operation/publishing locks during npm work. Same-site builds and conflicting site mutations are refused. Trust acknowledgment is required before code executes. npm is invoked with the pinned runtime's node.exe and npm-cli.js, never npm.cmd or a Hostify shell command. Install uses ci/install, no-audit/no-fund and ignore-scripts unless explicitly enabled. Each install/build step has a 20-minute deadline and its own kill-on-close Job Object. Cancellation and timeout terminate descendants. Real exit codes and the last 20 redacted log lines accompany failures. Interrupted builds become failed on engine restart, restoring the prior active release's build/trust markers when available. A rebuild of an existing active release first copies it to a fresh release so build scripts cannot modify its running files.

App/build processes clear inherited environment, restore the existing Windows allowlist and resolved site variables, and set PATH to the runtime, the release's node_modules/.bin and System32. npm uses `<data>/cache/npm` with update notifications disabled. Install unsets NODE_ENV; builds/apps default to production. App PORT and HOST are set last. Logs rotate at 5 MiB with three backups; secret_values supplies redaction for process output and build failure details. App logs are `<id>.app.log`, build logs `<id>.build.log`, and Caddy access logs retain `<id>.log`.

Every app/build process is assigned, while suspended, to the engine job and then its own nested job before resuming. Per-app jobs optionally enforce a total memory limit. Stops attempt Ctrl+C under a process-wide console lock in a blocking worker, wait up to ten seconds, then terminate the complete job if necessary. Console control dispatch is asynchronous: the engine keeps its ignore handler until dispatch/exit has settled before detaching and restoring it. Activity distinguishes graceful stops from forced termination. Crash exits terminate surviving descendants before any restart. Roles distinguish app, Caddy and connector messages/preflight. Always/onFailure/never policies and the app crash-loop limit govern restarts. Per-process health is aggregated under a lock, so healthy Caddy checks cannot overwrite an app error.

The compiled-in CommonJS preload patches host-less TCP listen overloads to use 127.0.0.1, including ESM users of net/http and listen(port, callback). Explicit hosts, pipes and handles retain their meaning. NODE_OPTIONS prepends the quoted preload path, using forward slashes for Node's option parser on Windows. After each healthy start, GetExtendedTcpTable checks IPv4/IPv6 listeners owned by the app job. Non-loopback addresses produce warning activity and appear in app_status. Resource statistics are measured on request using the job's process IDs; CPU sampling runs in a blocking worker, without background statistics polling.

Node startup requires matching built/trusted release markers, reallocates an occupied internal port, starts the app and waits up to 60 seconds for health below 500. It then validates a temporary Caddyfile before atomically replacing the prior file, starts that site's Caddy and requests the configured health path through Caddy with a foreign Host header. The proxy preserves marker routing, verification/security headers, logging and WebSocket upgrades, and disables response buffering with flush_interval -1. Caddy's own worker probes `/.well-known/hostify-caddy-ok`, separately from app health. Config changes can cause a brief gap for that one site; this is not zero-downtime. Stops end Caddy before gracefully stopping the app. Failed promotion restarts the previous release and records the update failure. Successful promotion prunes releases while retaining active, previous and newest. Trusted/built websites can auto-start after an engine restart; untrusted releases cannot.

Ignored real-Node tests in `crates/hostify-engine/tests/node_live.rs` read HOSTIFY_LIVE_RUNTIMES (the runtime data root), copy fixtures into temporary data, and read only the installed Caddy executable. They never launch a real Hostify engine or modify the live installation. Offline fixtures use only built-in Node modules. Real DPAPI success still requires a Windows user context where CryptProtectData works. Final command results and limitations are recorded below.

Task 12 shell limitation: the engine emits `app-build` to named-pipe subscribers, and the commands are registered in the permitted `src-tauri/src/lib.rs` forward/handler lists. The pre-existing shell reader in `src-tauri/src/engine.rs` whitelists only site-status, deployment-update, activity and components-progress; it therefore drops app-build (and the earlier runtime-progress) before Tauri event delivery. That file is outside this task's explicit ownership, so it was left unchanged. Desktop UI delivery of app-build is an outstanding integration change; clients can retrieve durable progress with get_build_status.


### Task 12 validation on 2026-10-31

CARGO_BUILD_JOBS remained 4. Compilation/test commands were offline. These are the actual final command summaries; full output is retained under target/task12-checks.

```text
cargo fmt --all
cargo fmt --all exit code: 0

cargo build --workspace --offline
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 32.53s
EXIT CODE: 0

cargo test --workspace --offline --no-fail-fast
    Finished `test` profile [unoptimized + debuginfo] target(s) in 40.44s
test result: FAILED. 120 passed; 2 failed; 4 ignored; 0 measured; 0 filtered out; finished in 1.54s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 25.34s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.38s
test result: FAILED. 8 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.60s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
error: 2 targets failed:
EXIT CODE: 101

cargo clippy --workspace --all-targets --offline
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.18s
EXIT CODE: 0

HOSTIFY_LIVE_RUNTIMES=D:\Hostify-Desktop\target\live-runtimes cargo test -p hostify-engine --test node_live --offline -- --ignored --test-threads=1 --nocapture
    Finished `test` profile [unoptimized + debuginfo] target(s) in 18.81s
test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 186.06s
EXIT CODE: 101

```

Workspace totals: 157 passed, 3 failed, 8 ignored, including empty binary/doc-test targets. The real Node suite separately passed 3 scenarios and failed 1. Its passing scenarios cover npm install with lifecycle hooks ignored, build cancellation, persisted interrupted status, build/start, foreign-Host proxy requests, raw WebSocket upgrade/echo, new-PID restart, five-exit crash loop, failed-build preservation, failed-health rollback, grandchild cleanup, graceful SIGINT with engine survival, host-less loopback/LAN behavior, explicit bind warnings, ESM/404 health, trust/path refusal, three concurrent sites, and trusted auto-start after reopening isolated engine data.

Failed or unverified:

- `vault::tests::round_trip_and_tamper`, `env_vars::tests::secret_is_ciphertext_and_never_listed`, `environment_secrets_are_not_in_pipe_errors_or_activity`, and ignored real-Node `live_secret_environment_is_redacted` fail at user-scoped DPAPI with `The system cannot find the file specified. (os error 2)`. The real secret-injection/redaction fixture therefore remains unverified after sealing; rerun it in a Windows user context with working DPAPI. There is no plaintext fallback.
- Desktop app-build event delivery remains incomplete because the existing shell whitelist is in the excluded src-tauri/src/engine.rs, as documented above. Engine pipe emission and durable status are implemented. No desktop UI end-to-end run was performed.
- Four existing publisher-download/network tests remained ignored under the no-network constraint. Live Cloudflare publishing, DNS/TLS and installed-engine/reboot behavior were not exercised.
- The 20-minute timeout, memory-limit enforcement, Never/OnFailure process policies, cancellation during release promotion, and Caddy-validation rejection/preservation were implemented but not separately exercised live.
- Clippy exits 0 with no errors; style warnings remain, including three in the new inspection/interrupted-build code, alongside pre-existing warnings.

`git diff --check` passes. Existing IPC/ENGINE documentation is preserved as an exact normalized prefix with sections appended. Workspace formatting was run; unrelated/protected source formatting was restored. No new crates, lockfile changes, commit, real engine start/stop, or live-installation changes were made. Tests copied the installed Caddy executable only into temporary fixture data. samples/ and docs/SECURITY.md were left unchanged.

## Review notes on task 12 (run outside the Codex sandbox)

The DPAPI failures in the section above are limited to the Codex sandbox. Outside it: 160 tests passed, plus all 4 `node_live` and 5 `node_policies_live` tests.

`peakMemoryBytes` comes from Windows `PeakJobMemoryUsed`, which counts a refused allocation. A 256 MiB job refusing a 900 MiB allocation reported a peak of 970 MB while using 52 MB, so it can exceed the limit. Do not present it as memory used. `memoryBytes` is the measured figure.

Not exercised: the 20-minute build timeout, cancelling during promotion, Caddy rejecting a generated configuration.


## PHP application hosting (Task 13)

PHP generalizes the Node import/build/activation path rather than creating another release manager. Both kinds preserve configured persistentPaths from the active release before stopping it for promotion. Recursive copies reject links and junctions, fail on incompatible destination types or copy errors, and leave the active process pool running on failure. Failed startup or Caddy health restores the previous release using the shared rollback path. Static routing and its literal Caddyfile regression remain unchanged.

Each PHP site starts 1 to 16 php-cgi workers, default 4, on separately reserved internal loopback ports. Workers are keyed php:<id>:<n>, have App role, individual managed Job Objects, the document-root working directory, the site's redacted environment/output and <id>.app.log. The shared environment builder puts the PHP runtime first on PATH followed by System32; workers do not receive PORT or HOST, npm additions or a default NODE_ENV. PHP_FCGI_MAX_REQUESTS is fixed at 10000 and PHP_INI_SCAN_DIR is empty.

The per-site run/php/<id>/php.ini is generated only after validating settings, extension names/DLLs and quoted paths, then written using a temporary file and rename. Paths use forward slashes and reject quotes/control characters. Sessions and temporary uploads have directories created before launch. The ini disables user ini scanning, public PHP error display, expose_php, URL includes and URL fopen, enables logging and strict/httponly sessions, sets UTC/GPCS and the CGI directives, and points extension_dir/error_log/session/upload/temp paths at managed locations. This configuration does not restrict imported code's access to the user's files, network or registry.

Workers use TCP readiness/health; HTTP health is requested only through Caddy at the configured healthPath, with the same ten-second healthy interval as Node. Clean worker exit immediately enters the restart path without delay, crash counting or activity, including onFailure policy; never prevents restart. Nonzero exits retain the shared crash-loop policy and Microsoft Visual C++ loader error wording. The site aggregates every worker and Caddy, and CPU/memory measurement is on demand. Stop ends Caddy before the pool; workers use the existing console graceful path with a three-second deadline and then Job Object termination.

PHP Caddyfiles keep marker routing, separate Caddy readiness, verification headers and access logs. An explicit route enforces deny rules before PHP/file handling: hidden path segments, NTFS stream syntax, trailing dots/spaces, Composer files, ini/log files and root vendor. php_fastcgi uses every worker with least_conn, lb_try_duration 30s and unhealthy_request_count 1, all accepted by the real installed Caddy. A case-insensitive PHP filename guard after php_fastcgi prevents any PHP fallback being served by file_server. Win32 extended-path prefixes are removed from PHP's Caddy root because PHP CGI rejects them in SCRIPT_FILENAME. Caddy is validated before replacing its configuration; the final health request uses a foreign Host header and requires a response below 500.

The ignored tests in crates/hostify-engine/tests/php_live.rs use copied pinned runtimes and the installed Caddy executable in temporary fixture data. They never start an installed engine or modify live installation data. The low-request recycling fixture compiles a small test-only wrapper which starts the real copied php-cgi with a request count of three; production always uses 10000. Live scenarios cover the repo sample, both PHP versions concurrently, environment injection, loaded extensions, source-disclosure spellings, simultaneous slow requests, recycling, killed-worker recovery, persistent data, copy failure, failed-health rollback, mixed PHP/Node/static sites, trust/path/junction refusal, disabled .user.ini, child cleanup and released ports.


### Task 13 validation on 2026-10-31

CARGO_BUILD_JOBS remained 4. All Cargo compilation and test commands were offline. The first workspace pass exposed Task 12's obsolete PHP-refusal assertion; it was updated to check stopped/untrusted PHP import, a pinned PHP runtime, version refusal and engine-owned ports. Inspection also now keeps WordPress classified as PHP when package.json contains asset start scripts and checks require-dev dependencies. Final workspace checks were repeated after those fixes. Actual final summary lines follow; logs are under target/task13-checks/verified, with the final ignored PHP suite under target/task13-checks/final.

```text
cargo fmt --all
EXIT CODE: 0

cargo build --workspace --offline
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.92s
EXIT CODE: 0

cargo test --workspace --offline --no-fail-fast
    Finished `test` profile [unoptimized + debuginfo] target(s) in 24.03s
test result: FAILED. 124 passed; 2 failed; 4 ignored; 0 measured; 0 filtered out; finished in 1.59s
error: test failed, to rerun pass `-p hostify-core --lib`
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 24.45s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.50s
test result: ok. 0 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: FAILED. 8 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.68s
error: test failed, to rerun pass `-p hostify-engine --test pipe_integration`
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
error: 2 targets failed:
EXIT CODE: 101

cargo clippy --workspace --all-targets --offline
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.36s
EXIT CODE: 0

HOSTIFY_LIVE_RUNTIMES=D:\Hostify-Desktop\target\live-runtimes
cargo test -p hostify-engine --test php_live --offline -- --ignored --test-threads=1 --nocapture
    Finished `test` profile [unoptimized + debuginfo] target(s) in 14.64s
1 PHP workers, four 2-second requests: 8.519s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 204.71s
EXIT CODE: 0

```

Workspace totals: 163 passed, 3 failed, 19 ignored. The separate ignored PHP suite passed all 6 grouped tests, covering the twelve requested live scenarios. Latest four-worker concurrency was 2.011 seconds; one-worker concurrency was 8.519 seconds. The recycling fixture observed 25 worker restarts without a crash-loop or unexpected-restart activity. An earlier unstaggered concurrency attempt took 4.019 seconds with four workers and failed the strict limit; the identical simultaneous-request test was retained, without launch staggering or a relaxed threshold, and passed its subsequent runs (2.010, 2.014 and 2.011 seconds). Consistently sub-four-second burst performance across runs is therefore not established.

Failed or unverified:

- vault::tests::round_trip_and_tamper fails in real user-scoped DPAPI with os error 2.
- env_vars::tests::secret_is_ciphertext_and_never_listed fails at DPAPI sealing with os error 2.
- environment_secrets_are_not_in_pipe_errors_or_activity fails at DPAPI sealing with os error 2. Rerun these three tests outside this sandbox with a loaded Windows user profile. No plaintext fallback was added.
- One earlier four-worker burst failed at 4.019 seconds despite later passes; strict burst timing was not consistently met in every run.
- The missing Visual C++ runtime exit path was not exercised live. Existing runtime wording tests pass; installed runtimes have their dependencies available.
- The other ignored Node/network suites were not run. The PHP suite exercises the requested simultaneous PHP/Node/static scenario. Real installed-engine, desktop UI, Cloudflare publishing and public DNS/TLS flows were not exercised.

Clippy exits 0 with no errors; style warnings remain. cargo fmt --all and git diff --check pass. File ownership checks pass, including append-only IPC/ENGINE documentation with normalized line endings. No new crates or Cargo.lock changes, commits, branch changes, installed-engine start/stop, or live installation modifications were made. The installed Caddy executable was only read/executed or copied into temporary fixture data. Protected source, samples, UI tests and SECURITY.md remain unchanged.

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

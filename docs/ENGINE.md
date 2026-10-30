# Hostify engine implementation status

The earlier core pass hardened `hostify-core` deployment and import only. Its offline workspace build confirmed compatibility with the core APIs. The Task 5 engine changes and validation are described below; these do not certify a fully installed end-to-end hosting product.

## Deployment journal and rollback

Deployment persists its request, ownership information, and create intent in SQLite before each Cloudflare POST. A newly created tunnel has a deployment-specific physical name (`hostify-<deployment UUID>`), recorded before creation; its returned ID is saved before fetching its token. DNS creation persists an intent first and adds the `managed-by=hostify` comment marker plus a `hostify-deployment:<deployment UUID>` DNS tag. The returned DNS ID is saved immediately. Tokens remain in the secret store, outside the journal.

Recovery looks up ambiguous tunnel creates by their recorded unique name and DNS creates by hostname, management marker, and deployment tag. It rolls back interrupted attempts and retries failed attempts whose cleanup is incomplete. Successful cleanup operations are checkpointed. A shared hostname or management marker alone is never proof that this deployment created a resource.

Before changing ingress, deployment saves the entire prior config and intended config, including origin options and existing entries. Rollback restores the snapshot when the remote config still equals the intended config. If another actor has changed it, rollback removes only the exact ingress entries this deployment added, preserving other entries and options. Reused tunnels are never deleted. A newly created tunnel is deleted only after removing this deployment's ingress and checking that no ingress remains except the standard terminal 404 entry. A tunnel with other ingress is retained, with its ID and ownership still in the deployment journal.

DNS deletion checks the deployment ownership tag during reconciliation and uses `delete_dns_if_ours` to recheck the management marker before DELETE. Reused DNS is never adopted for cleanup. Connector and local-site stop calls occur only for resources whose runtime start call returned `true`. Remote cleanup failures do not prevent attempts to stop the local resources; incomplete cleanup stays eligible for recovery.

No DNS consent means zero Cloudflare calls. DNS conflicts detected during preflight cause no writes or local starts; planning is checked again before tunnel configuration and DNS creation. Existing ingress is merged rather than replaced. Re-publishing a live site and hostname reuses its journaled physical tunnel, repeats validation and public verification, and does not create another tunnel or DNS record. A previously saved `live` snapshot alone is not treated as successful verification.

Public verification retains the production HTTPS fetch and index-body SHA-256 comparison, with a bounded retry deadline. `SiteRuntime::verify_public` supplies a default production implementation and an offline test seam. Existing public method signatures and engine-facing journal fields remain compatible. Additional Cloudflare trait methods have fail-closed defaults for older adapters; those adapters must implement the journaled operations to deploy safely.

## Import hardening

Imports retain only the newest three previous releases, ordered by their numeric timestamp names. Unsafe release entries and existing links or reparse points in managed destination ancestors are rejected.

On Windows, directory traversal retains non-delete-shared directory handles during recursion. Source files, including ZIP archives, are opened with `FILE_FLAG_OPEN_REPARSE_POINT`; metadata from the opened handle must identify a regular, non-reparse file with the scanned size. File handles deny write/delete sharing while copying. Size limits are applied after opening and the copied byte count is checked. These checks reduce scan/open replacement races; they are not a complete filesystem security audit. In particular, destination ancestor validation is still path based and is not an atomic defense against hostile concurrent changes. ZIP edge cases and other platform behavior need further adversarial review.

## Covered by offline tests

The core suite uses a fake `SiteRuntime`, in-memory SQLite, and wiremock Cloudflare on loopback. It covers:

- Full seven-step success ending `live`; a separate failure at every step with exact DELETE and local-stop assertions, preserving unrelated DNS and tunnels.
- No-consent refusal with zero Cloudflare requests; foreign and mismatched tagged DNS conflicts without writes.
- Propagation failure ending `failed`, reused DNS preservation, full reused-ingress restoration, and already-running local resources remaining running.
- An ingress write that takes effect but returns an error; concurrent ingress additions retained and a shared created tunnel preserved.
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

Cloudflare calls are not a distributed transaction: another account actor can change configuration between a read and a write or delete. The client has no provider-side compare-and-swap or transaction primitive; preserving changes already visible during rollback does not eliminate that concurrent-update window. Recovery assumes successful Cloudflare list responses accurately expose committed creates. Historical deployment snapshots made before create intents existed cannot reconstruct IDs that were never saved.

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

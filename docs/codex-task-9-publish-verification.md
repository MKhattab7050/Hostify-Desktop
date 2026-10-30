# Codex Task 9 — publishing state machine and evidence-based public verification

Repo D:\Hostify-Desktop. `%USERPROFILE%\.cargo\bin` on PATH, always `cargo ... --offline`. No network, no new crates (reqwest, tokio, uuid, sha2, serde_json, wiremock, tempfile are available).

## FILE OWNERSHIP
Edit ONLY `crates/**` and `docs/ENGINE.md`. Do NOT touch `src/**`, `src-tauri/**`, `docs/IPC.md`. `docs/IPC.md` is already updated and is the contract: read it first and match its JSON exactly (`DeploymentState`, `VerifyClass`, `VerifyCheck`, `Verification`, `Deployment`, the `Site.publish*` fields, `retry_verification`, `cancel_deployment`).

## Root cause (measured on the user's real account, do not re-litigate)
`SiteRuntime::verify_public` in `crates/hostify-core/src/deploy.rs` uses reqwest with the OS resolver, discards every error, and after 180 s returns the hard-coded "DNS has not propagated yet". `deploy()` then rolls back and deletes the DNS record and tunnel.
Measured 2 minutes after the record was created: authoritative NS and 1.1.1.1 returned the record, 8.8.8.8 and this PC's resolver returned a cached negative answer (`SOA ttl=1658`, `ttl=1212`), default curl said "Could not resolve host", and curl with `--resolve host:443:<edge ip>` got HTTP 200 from Cloudflare. The rollback then deletes the record, so each retry re-arms the negative cache.
Second defect: the check compares the SHA-256 of the served body with `index.html`, which fails forever if Cloudflare rewrites HTML (Rocket Loader, email obfuscation), and an unrelated identical page would pass.

## Required behaviour

### 1. States
`Deployment.state` uses the `DeploymentState` values. Each step sets its state as it starts: validate=`validating`, start server and verify local=`starting_local`, create tunnel=`provisioning_tunnel`, add DNS=`configuring_dns`, start connector=`starting_connector`, then `waiting_dns` / `waiting_tls` / `verifying_public` according to the latest attempt, ending in `published`. Add a migration in `db.rs` that rewrites existing rows and their `snapshot_json.$.state`: `live`→`published`. A legacy `running` row is treated like an interrupted provisioning row. Update EVERY SQL list and comparison on state (`deploy.rs`, `state.rs` including `live_resources`, `supervisor.rs` `start_tunnel`, the retry loop). Add `hostname` to `Deployment` (optional for old snapshots, serde default).

### 2. Verification never rolls back
Failure in steps validate through start connector keeps today's behaviour (roll back only what this deployment created, `failed`). Once step `verify_https` starts, NOTHING is deleted automatically: no DNS delete, no tunnel delete, no connector stop, no site stop. Exhausting the budget gives `recovery_required` with every resource retained, and the connector and site still running.

### 3. New module `crates/hostify-core/src/verify.rs`, readable code (not the dense one-line style)
A trait `PublicProbe` (async_trait, Send + Sync) wraps every external observation so tests can fake them: zone status + nameservers, API record check, connector health, authoritative lookup, recursive lookup, system lookup, pinned HTTPS GET. Add it to `Deployer` so existing test construction changes minimally.
One attempt runs these with a timeout on each network call (max 10 s), filling `Verification.checks` in order:
- `zone`: zone `status` and `name_servers` from the Cloudflare API (extend `Zone` with serde defaults). Not `active` ⇒ `zone_not_active`, put the nameservers in `Verification.nameservers`.
- `dns_record`: the journaled record still exists, is a proxied CNAME to `<tunnel>.cfargotunnel.com`. Gone or changed ⇒ `dns_record_missing`, never recreate or overwrite it automatically.
- `connector`: local cloudflared `/ready` and connection count (`supervisor.rs` helpers via `SiteRuntime`). Not connected ⇒ `connector_down`.
- `dns_public`: gather IPv4 and IPv6 from (a) UDP DNS directly against the zone's authoritative nameservers, with a small hand-written query builder and answer parser in `verify.rs`; (b) DNS-over-HTTPS JSON at `https://cloudflare-dns.com/dns-query` and `https://dns.google/resolve`; (c) the OS resolver via `tokio::net::lookup_host`. Any source may fail without failing the check. Do NOT expect to see the tunnel CNAME: proxied and apex records return Cloudflare edge A/AAAA. No addresses from any source ⇒ `dns_not_visible`. Set `visibleFromThisPc` from (c) alone.
- `tls`, `http`, `content`: for up to 4 addresses (IPv4 first), GET `https://<hostname>/.well-known/hostify-verify/<nonce>` with `ClientBuilder::resolve_to_addrs(hostname, addr)`, default certificate validation (NEVER disable or weaken it), no redirects to other hosts. Success requires HTTP 200 and a body exactly equal to the nonce. Then GET `/` on the same client and require a 2xx, without comparing the body. Classify using the reqwest error source chain: certificate or handshake ⇒ `tls_pending`; 530 or Cloudflare 1033 ⇒ `connector_down`; 502/503/504 ⇒ `origin_unreachable`; 404 or wrong body ⇒ `routing_mismatch`; connect or timeout ⇒ `network_error`; API failures ⇒ `cloudflare_api`.
State mapping: `zone_not_active`, `dns_record_missing`, `dns_not_visible` ⇒ `waiting_dns`; `tls_pending` ⇒ `waiting_tls`; otherwise `verifying_public`. Each class has a specific plain-language `message` saying what is happening and what, if anything, the user should do. The string "DNS has not propagated yet" must no longer exist. Publishing with `visibleFromThisPc=false` is valid: the deployment is `published` and the message says this PC is showing an old cached answer that clears by itself.

### 4. Nonce marker
Per deployment: random 32 hex chars from uuid, journaled in `Resources`, file at `<data>/run/verify/<siteId>/<nonce>` with the nonce as its content, never inside the website's public files. `caddy.rs` adds, per site, `handle_path /.well-known/hostify-verify/*` rooted at that directory with `file_server` and `Cache-Control: no-store`. The existing dot-path 404 rule must still protect everything else, so `/.well-known/anything-else` and `/.env` stay 404. Directory listing stays off. Delete the file on publish, unpublish and rollback. Create the directory before Caddy starts. Prove the Caddyfile with the real binary if `%LOCALAPPDATA%\Hostify\bin\caddy.exe` exists; skip that test with a clear printed message if it does not.

### 5. Retry
Exponential backoff with jitter: 5 s doubling, cap 60 s, ±20 % jitter (jitter source: uuid bytes), budget 30 minutes. Persist `attempt`, `nextRetryAt`, `deadlineAt`, `lastCheckedAt`, `errorClass`, `checks` and emit `deployment-update` after every attempt. Backoff and budget are pure, tested functions; waits use `tokio::select!` with a per-deployment cancel signal. Do not write unchanged snapshots more than once per attempt.

### 6. Concurrency
The verifier must NOT hold `Engine.publishing` or `operations` during the wait, so other sites can publish and start/stop during a long DNS wait. Track one verifier task per deployment id in the engine. A second `publish_site` for a site with a non-terminal or `recovery_required` deployment returns an actionable error naming `retry_verification`. `publish_site` keeps returning the first snapshot immediately.

### 7. Commands
`retry_verification` and `cancel_deployment` as specified in `docs/IPC.md`. Retry is idempotent: if a verifier is already running, wake it instead of starting a second one. Cancel during provisioning rolls back this deployment's own resources via the existing rollback. Cancel during verification keeps every resource. `unpublish_site` must stop the verifier first, work from `waiting_*`, `verifying_public`, `recovery_required` and `published`, remove the nonce file, and keep its ownership checks.

### 8. Restart recovery
`recover_interrupted` must NOT roll back `waiting_dns`, `waiting_tls`, `verifying_public` or `recovery_required`. On engine startup: start the site and connector for every deployment in those states and in `published`, whatever `auto_start` says for a published site. Then resume the verifier for the waiting states with a fresh attempt, keeping `deadlineAt` if still in the future and otherwise granting a new budget. Rows interrupted before `verify_https` keep today's rollback. Include these states in `live_resources` so `remove_site` refuses while they hold Cloudflare resources.

### 9. Domain and site surfacing
While waiting, upsert the `domains` row with `dns_status='pending'`, `https_verified=0`; on `published`, `configured` and `1`. `publicUrl` stays conditional on `https_verified`. `Site` gains `publishState`, `publishDeploymentId`, `publishHostname` from the latest deployment that is not `rolled_back`. Site status must not stay `deploying` during a verification wait: it reflects the local server. Record state changes in `activity_logs` once per transition, not once per attempt.

## Regression tests (real assertions, using the fake `PublicProbe` and existing fixtures)
- Verification failing past the budget ends in `recovery_required` with zero Cloudflare writes and no connector or site stop. Assert the fake recorded no delete.
- The exact production case: system and 8.8.8.8 negative, authoritative positive, pinned HTTPS OK ⇒ `published`, `visibleFromThisPc=false`.
- Unrelated HTTP 200 or wrong body ⇒ never `published`, class `routing_mismatch`.
- Body changed on `/` by a Cloudflare rewrite, marker correct ⇒ `published`.
- Each class maps to the right state and message; state strings serialize exactly as in IPC.md.
- Pure backoff: bounds, cap, jitter range, budget expiry.
- Cancel during verification keeps resources; retry wakes one verifier, not two.
- Restart: `waiting_dns` row resumes without rollback; provisioning row still rolls back; legacy `live` row migrates to `published`.
- `remove_site` refuses during `recovery_required`; `unpublish_site` from `waiting_dns` removes only this deployment's own record.
- DNS packet builder and parser: A, AAAA, CNAME chain, compression pointers, truncated and malformed input return an error and never panic.
- Caddy config: marker route present, `/.env` still hidden, no `browse`.
- Pinned client code path contains no `danger_accept_invalid_certs` (grep-style test on the source is acceptable).

## Definition of done
Run `cargo build --workspace --offline` and `cargo test --workspace --offline` and paste the real summary lines. Report files changed, and state plainly anything not exercised. Live Cloudflare behaviour cannot be tested from your sandbox: say so, do not claim it. Keep `docs/ENGINE.md` to a short accurate section on the state machine.

# Hostify V2: Fast Publish and Standard Publish

Implementation branch: `feature/fast-standard-publish`.
Base: `release/v2-rc1`. This is an implementation candidate, not yet a certified production release.

## Two experiences, one deployment engine

- **Fast Publish**: pick a folder or ZIP, enter a Cloudflare hostname, approve safety-sensitive operations, and publish. Project detection, bounded read-only retries, runtime and component setup, trusted release build, local start, ownership checks, and the existing journaled HTTPS deployment are orchestrated automatically.
- **Standard Publish**: retains the advanced runtime, working directory, health endpoint, build, database, environment, PHP and domain configuration screens. Uses the same underlying Rust deployer and Cloudflare verification.
- Both modes use `publish_preflight`, `publish_diagnosis`, and the existing `publish_site` engine. The backend remains authoritative for DNS consent, build trust and local health.

The fast frontend itself never claims success from a DNS write. It shows the existing `PublishProgress` state machine and durable server-side verification. The engine records selected Fast Publish stages as fixed, secret-free messages in the existing activity database via `publish_record_event`. Actual deployment steps and verification attempts remain in the existing deployment journal.

## Approval and repair policy

| Operation | Fast Publish policy |
| --- | --- |
| Project type, framework and health path discovery | Read-only detection |
| Missing Caddy/cloudflared | Install integrity-checked components only after authorization |
| Missing selected Node/PHP runtime | Install pinned version only after authorization |
| Node 26 with known incompatible dependency | Fail closed with Node 24 LTS guidance; no silent version switch |
| Dependency installation and app execution | Require explicit trust of the project's code |
| Missing MySQL instance required by project | Require explicit choice to provision a new dedicated database; never import SQL silently |
| Previously linked stopped MySQL instance | Restart the managed instance |
| Database name collision | Stop and report, never link an existing database as a fallback |
| Foreign Caddy Host / HTTP 421 | Reject; do not mark healthy |
| Temporary read-only API checks | At most two retries with bounded exponential backoff |
| Cloudflare tunnel, ingress or DNS writes | Use the durable backend engine only after explicit domain consent |
| DNS propagation | Wait/reverify via existing verification engine; no duplicate DNS creation |
| Any rollback/data/trust/security blocker | Fail closed with error and corrective guidance |

For safety, changes to customer `.env`, source files, `node_modules`, and SQL schemas are **not** made during startup. Explicit user builds may still generate files inside an isolated Hostify release copy. Existing release rollback remains implemented by the engine.

## API and implementation

- `hostify-core/src/diagnostics.rs`: deterministic class and suggested action; classification does not mutate state.
- `hostify-engine/src/state.rs`: read-only publish preflight and diagnosis; fixed-template persistent activity entries; build trust guard before provisioning.
- `hostify-engine/src/supervisor.rs`: non-destructive app startup; strict 2xx/3xx routing health; compatibility diagnostics.
- `hostify-engine/src/mysql.rs`: prevents implicit reassignment of a database owned by another site.
- `hostify-engine/src/cli.rs`: disables privileged Windows Service hosting pending isolated service accounts.
- `src/components/FastPublish.tsx`: guided flow, explicit consent, isolated staged operations, detailed history and shared progress tracking.
- `src/components/PublishWizard.tsx`: mode selection and detailed Standard Publish.
- `src/components/BuildPanel.tsx`: explicitly passes the trust acknowledgment rather than implicitly setting it to true.

## Verification boundaries

- The browser preview is not the actual Windows engine; its tests do not prove live Cloudflare or runtime functionality.
- Rust tests and UI tests are set up in GitHub Actions. Their results must be checked before merging; adding a workflow does not make its checks pass.
- Live Node/PHP/Caddy/MySQL, real DNS/HTTPS, clean Windows installer, reboot, Windows Service lifecycle, and offline-to-online recovery still require dedicated isolated Windows testing.
- No test requires or modifies `niga.online`. Do not use production Cloudflare tokens for CI.
- Error classifications are guidance; safe automated repairs are limited to approved component setup, runtime installation, managed service restart, port allocation, read-only retry, and the existing durable deployment recovery. The implementation cannot guarantee repair of arbitrary application errors.

## Manual release acceptance checklist

- [ ] Inspect the change set and review security-sensitive Rust code.
- [ ] Verify Windows CI is green.
- [ ] On an isolated Windows VM, import Static, Express, Socket.IO, Node+MySQL, PHP and PHP+MySQL examples.
- [ ] Confirm Fast Publish cannot proceed without dynamic code trust or DNS permission.
- [ ] Confirm Standard Publish blocks untrusted builds.
- [ ] Test duplicate DB names across two sites and confirm ownership cannot transfer implicitly.
- [ ] Test HTTP 421 foreign Host, legitimate Host 200, and application 500/404 behavior.
- [ ] Test crash recovery with Node 24 and incompatible Node 26 dependencies.
- [ ] Test rollback after connector failure and no foreign DNS removal.
- [ ] Test power loss/reboot and recovery, offline-to-online tunnel reconnection, and uninstall preserving user data.
- [ ] Test on a clean Windows 10/11 PC with new installation paths.

Do not merge or publish to paying users until these verifications are complete.

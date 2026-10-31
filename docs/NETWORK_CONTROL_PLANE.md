# Hostify Network Local Control Plane (Developer Foundation)

The `hostify-control` binary implements a **loopback-only** HTTP backend for the existing Hostify Network tenant and device registry. It is designed for trusted administrators on a development machine, **not for public deployment**.

## Run locally

Requires Rust with the repository's pinned `Cargo.lock`. Use an isolated database, a **random** 32+ character administrator token, and a test namespace you control. For example:

```powershell
$env:HOSTIFY_CONTROL_ADMIN_TOKEN = [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(32))
$env:HOSTIFY_CONTROL_BASE_DOMAIN = "example.test"
$env:HOSTIFY_CONTROL_DATABASE = "$env:TEMP\hostify-control-test.sqlite"
$env:HOSTIFY_CONTROL_PORT = "8787"
cargo run --locked -p hostify-control
```

The binary binds only to `127.0.0.1` and cannot be configured to bind to `0.0.0.0`. It caps active local HTTP connections at 64 and times out idle or slow connections after 30 seconds. Connections above the limit are closed without processing; this is a local resource bound, **not** a replacement for per-IP rate limiting on a future public gateway.

## Development endpoints

All endpoints are JSON, have a 32 KiB body limit, disable caches, and return no CORS grants.

| Endpoint | Authentication | Behavior |
|---|---|---|
| `GET /healthz` | none | Returns `alive:true, publicReady:false` |
| `POST /v1/tenants` | Admin Bearer | Creates a tenant |
| `POST /v1/invitations` | Admin Bearer | Issues short-lived single-use device invitation |
| `POST /v1/agents/enroll` | Enrollment invitation | Records Ed25519 public key, never private key |
| `POST /v1/routes/reserve` | Admin Bearer | Reserves globally unique, **non-routable** hostname |
| `POST /v1/agents/challenge` | Admin Bearer | Issues 30–300 second signed device challenge |
| `POST /v1/agents/proof` | Signed Ed25519 challenge | Stores identity proof only, **not public routing** |
| `POST /v1/accounts/provision` | Admin Bearer | Preferred developer onboarding: creates tenant, first owner and audit row atomically |
| `POST /v1/accounts/bootstrap` | Admin Bearer | Legacy two-step developer flow: assigns first owner to an existing tenant, once only |
| `POST /v1/auth/login` | Tenant ID + email + password | Creates a 12-hour hashed session; returns token only in response |
| `POST /v1/auth/me` | Tenant session Bearer | Returns principal if session is active for the specified tenant |
| `POST /v1/auth/logout` | Tenant session Bearer | Revokes session |
| `POST /v1/auth/password/change` | Tenant session + current password | Changes own password, revokes **all** old sessions and writes audit atomically; must log in again |
| `POST /v1/auth/sessions/list` | Tenant session Bearer | Lists only the caller's active sessions, identifies the current one, never exposes tokens or hashes |
| `POST /v1/auth/sessions/revoke` | Tenant session Bearer | Revokes one of caller's own sessions by session UUID and audits atomically (can revoke current session) |
| `POST /v1/auth/sessions/revoke-all` | Tenant session Bearer | Revokes the caller's other sessions (`keepCurrent:true`) or all sessions including current (`keepCurrent:false`), atomically audited |
| `POST /v1/members/invite` | Tenant owner/admin session | Issues role-scoped, single-use member invitation; response also includes non-secret `invitationId` |
| `POST /v1/members/invitations/list` | Tenant Owner/Admin session | Lists pending, unexpired, non-revoked invitations without token or hash; Admin sees viewer invitations only |
| `POST /v1/members/invitations/revoke` | Tenant Owner/Admin session | Invalidates a pending invitation by UUID and audits atomically; Admin can revoke viewer invitations only |
| `POST /v1/members/accept` | Single-use member invitation | Creates invited account without exposing admin credentials |
| `POST /v1/members/disable` | Tenant Owner/Admin session | Disables an eligible member, invalidates all sessions and writes a security audit event; never disables an owner or caller |
| `POST /v1/members/list` | Tenant Owner/Admin session | Bounded, paginated list of own tenant's users (including disabled) with only ID, email, role, status and creation date |
| `POST /v1/members/role/update` | Tenant **Owner** session | Promotes/demotes only non-owner active members between Admin and Viewer, revokes all target sessions, audits atomically |
| `POST /v1/tenant/invitations` | Tenant ManageDevices session | Issues one-use device enrollment invitation for own tenant, with a non-secret invitation ID |
| `POST /v1/tenant/invitations/list` | Tenant ManageDevices session | Lists own pending unconsumed/unexpired device invitations, without tokens or hashes |
| `POST /v1/tenant/invitations/revoke` | Tenant ManageDevices session | Atomically cancels a pending enrollment invitation by ID and audits revocation |
| `POST /v1/tenant/routes/reserve` | Tenant ManageSites session | Reserves a non-routable hostname for own tenant |
| `POST /v1/tenant/routes/reservations/list` | Tenant View session | Lists own reserved and disabled hostnames on non-revoked devices, with status and paging |
| `POST /v1/tenant/routes/disable` | Tenant ManageSites session | Disables a reserved hostname without releasing global ownership; frees local quota and invalidates proof/challenges |
| `POST /v1/tenant/routes/restore` | Tenant ManageSites session | Rechecks active device and quota, then restores a disabled hostname to **reserved only**, never publicly active |
| `POST /v1/tenant/routes/reassign` | Tenant ManageSites session | Reassigns a reserved hostname to a tenant-owned active device/site without changing global hostname ownership; invalidates old proofs |
| `POST /v1/tenant/agents/revoke` | Tenant ManageDevices session | Revokes own device and disables its reserved routes |
| `POST /v1/tenant/agents/rename` | Tenant ManageDevices session | Sets a friendly name for an active device; no key or routing change; audited atomically |
| `POST /v1/tenant/agents/list` | Tenant View session | Bounded, paginated inventory of the tenant's non-revoked devices |
| `POST /v1/tenant/routes/list` | Tenant View session | Bounded, paginated inventory of the tenant's non-disabled hostnames; not public reachability |
| `POST /v1/tenant/audit/list` | Tenant ManageUsers session | Owner/Admin-only security activity: tenant-scoped, newest-first and bounded pages |
| `POST /v1/tenant/usage` | Tenant View session | Own tenant's device and hostname reservation counts, quota limits and remaining capacity; **not** a public traffic or billing meter |

For new operator-provisioned tenants, prefer `POST /v1/accounts/provision` with `name`, `email` and `password`. It rolls back the whole operation if owner creation or auditing fails. The old `POST /v1/tenants` plus `POST /v1/accounts/bootstrap` developer flow remains for compatibility but is not atomic across requests. **Neither is a public self-signup API.**

Account login and single-use token redemption do not require an admin Bearer. All other account actions require either the operator secret or a tenant session with the appropriate role. A tenant session cannot reserve or revoke another tenant's resources.

`/v1/tenant/audit/list` accepts the same page bounds but requires **Owner or Admin**, not Viewer. It returns only the caller's tenant audit entries, with internal event IDs and non-secret subjects. It remains loopback-only and must not be published by Cloudflare.

Both inventory endpoints require a valid scoped tenant session and strict `tenantId`, `limit` (1–100), and `offset` (0–1,000,000). They return `items`, `limit`, and `offset` (plus `publiclyRoutable: false` for routes). Results never include another tenant's rows or revoked devices. Inventory does not prove public HTTPS reachability.

New member and session inventory calls accept `tenantId`, `limit` (1–100), and `offset` (0–100,000), and return `items`, `limit`, and `offset`. Member inventory requires Owner or Admin; Viewer cannot enumerate users. Session inventory is restricted to **the authenticated user's own** active sessions and returns `sessionId`, `createdAt`, `expiresUnix` and `isCurrent`. Session revocation accepts `tenantId` and `sessionId`. Member role updates accept `tenantId`, `userId` and `role` (admin/viewer). Only the owner may change roles, never the owner role; a real change invalidates all target member sessions and requires fresh sign-in. Password rotation accepts `tenantId`, `currentPassword` and `newPassword` (12–256 bytes and control-character restrictions). On success, existing sessions including the caller's are revoked. Transaction rollback prevents changing a credential or revoking a session without its audit event. These endpoints remain **local development facilities**, not a production self-service gateway.

Bulk session revocation accepts `tenantId` and `keepCurrent` (required boolean). A true value signs out all other active sessions for that user; false signs out **every** session including the current request's bearer token. It returns `revokedCount` and `currentSessionActive`. Only the caller's own tenant/user sessions are affected, and audit persistence is in the same SQLite transaction; a failed audit rolls back the revocation. No password changes, token exposure, or public gateway is performed.

Pending invitation management uses `tenantId`, `limit` (1–100) and `offset` (0–100,000) for lists, and `tenantId` plus `invitationId` for revocation. The invitation ID is metadata, **not** its one-use secret. Revocations persist separately in SQLite, without changing legacy invitation tables, and token redemption always checks revocation inside the write transaction. An audit insertion failure rolls back revocation. Revoked invitations can never be redeemed and are omitted from pending lists. The revoke operation can be repeated without issuing duplicate audit events. The user's tenant and privilege checks always apply.

Device enrollment invitation list uses `tenantId`, `limit` (1–100), `offset` (0–1,000,000). To cancel, submit `tenantId` and `invitationId`. IDs are metadata, not enrollment bearer tokens. Cancellation uses an additive SQLite revocations table; enrollment verifies revocation in the same write transaction. Consumed invitations cannot be cancelled (revoke the enrolled device instead). Repeated cancellation is idempotent and does not duplicate audit entries. All operations are scoped to `ManageDevices` on an active tenant; no public gateway or DNS activation occurs.

Device rename accepts `tenantId`, `agentId` and `displayName` (1–48 trimmed Unicode characters, 160-byte maximum, no control characters, angle brackets or backslashes). Only active devices owned by the caller's tenant can be renamed. The display name is optional in device inventory, stored separately from Ed25519 keys and never used for authorization or routing. Renaming an unchanged display name is idempotent; real changes are audited atomically. Revoked devices cannot be renamed.

The `tenant/usage` request accepts only `tenantId` and returns `usage` plus `publicReady:false`. Its counts mirror the quota checks for non-revoked devices and reserved/active hostnames, including an update after a device is revoked. `publiclyRoutable:false` is explicit because this local development service cannot provision an internet route. Usage is not billable metering and does not prove the existence of a live site.

The reservation lifecycle API accepts `tenantId` and `routeId` for disable/restore, and `tenantId`, `limit` (1–100), `offset` (0–1,000,000) for listing including disabled reservations. Disabling a reserved hostname removes it from active quota accounting but **retains its unique ownership** to prevent another tenant claiming it. Restoring checks the configured quota again, and an agent must still be enrolled and not revoked. Repeated disable/restore operations are idempotent. Both mutations purge old challenges and identity proofs and append audit events atomically. Neither acts on a route whose state is already `active`; public teardown/activation would require a separate secure gateway. No DNS or Cloudflare changes occur.

Reassignment accepts `tenantId`, `routeId`, `agentId` (destination active agent) and `siteId` (destination site UUID). It is limited to **reserved** routes, never `active` or `disabled`. The route UUID, tenant and hostname remain unchanged; only the assigned device and site identifiers change. Old route challenges and identity proofs are removed inside the same SQLite audit transaction. Cross-tenant or revoked-device reassignment fails. It is a reservation management operation, NOT a live site migration, file copy, DNS modification or a signal of public readiness. Any later publication requires fresh device proof and a separate production gateway.

The hostname returned by `routes/reserve` or `tenant/routes/reserve` does **not** create public DNS, a Cloudflare tunnel, or a valid customer web route.

A malicious browser page cannot call admin operations without the secret, and the process sends no Access-Control-Allow-Origin response.

## Security release blockers

The local administrator secret still controls operator endpoints, while the new tenant session endpoints enforce basic per-tenant RBAC. Account sessions are stored as hashes and expire, but this service has **no public TLS, MFA/passkeys, CSRF gateway, per-IP throttling or comprehensive abuse protection**. Treat both passwords and session tokens as confidential. Never expose the operator credential, a tenant session token or this listener through a desktop webview, customer device or tunnel. Production requires:

1. External HTTPS ingress with strict origin controls, CSRF-safe authentication, MFA and per-tenant RBAC.
2. Persistent scoped API tokens, revocation, audit, threat/abuse rate limits and IP controls.
3. Device private key provisioning and authenticated outbound connector.
4. Explicit owner confirmation and health-gated, transactional Cloudflare route activation.
5. Public DNS, TLS, response nonce and 421 host isolation verification.
6. Subscription billing, metering, quota abuse protection, privacy and takedown operations.
7. Independent Windows integration tests, crash recovery and active incident monitoring.
8. Secure deployment, backup and restore of the central SQLite database.

**Do not open a Cloudflare Tunnel to this backend as shipped.** There is no production-ready public gateway. No commercial launch should depend on this service yet.

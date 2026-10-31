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
| `POST /v1/members/invite` | Tenant owner/admin session | Issues role-scoped, single-use member invitation |
| `POST /v1/members/accept` | Single-use member invitation | Creates invited account without exposing admin credentials |
| `POST /v1/tenant/invitations` | Tenant ManageDevices session | Issues one-use device enrollment invitation for own tenant |
| `POST /v1/tenant/routes/reserve` | Tenant ManageSites session | Reserves a non-routable hostname for own tenant |
| `POST /v1/tenant/agents/revoke` | Tenant ManageDevices session | Revokes own device and disables its reserved routes |

For new operator-provisioned tenants, prefer `POST /v1/accounts/provision` with `name`, `email` and `password`. It rolls back the whole operation if owner creation or auditing fails. The old `POST /v1/tenants` plus `POST /v1/accounts/bootstrap` developer flow remains for compatibility but is not atomic across requests. **Neither is a public self-signup API.**

Account login and single-use token redemption do not require an admin Bearer. All other account actions require either the operator secret or a tenant session with the appropriate role. A tenant session cannot reserve or revoke another tenant's resources.

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

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

The binary binds only to `127.0.0.1` and cannot be configured to bind to `0.0.0.0`.

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

Only enroll and proof endpoints accept one-use credentials without admin authorization. The hostname returned by `routes/reserve` does **not** create public DNS, a Cloudflare tunnel, or a valid customer web route.

A malicious browser page cannot call admin operations without the secret, and the process sends no Access-Control-Allow-Origin response.

## Security release blockers

The local admin secret is a single process-level credential and is not tenant-level RBAC. Never expose it through a desktop webview or customer device. Production requires:

1. External HTTPS ingress with strict origin controls, CSRF-safe authentication, MFA and per-tenant RBAC.
2. Persistent scoped API tokens, revocation, audit, threat/abuse rate limits and IP controls.
3. Device private key provisioning and authenticated outbound connector.
4. Explicit owner confirmation and health-gated, transactional Cloudflare route activation.
5. Public DNS, TLS, response nonce and 421 host isolation verification.
6. Subscription billing, metering, quota abuse protection, privacy and takedown operations.
7. Independent Windows integration tests, crash recovery and active incident monitoring.
8. Secure deployment, backup and restore of the central SQLite database.

**Do not open a Cloudflare Tunnel to this backend as shipped.** There is no production-ready public gateway. No commercial launch should depend on this service yet.

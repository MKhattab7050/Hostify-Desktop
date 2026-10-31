# Hostify Network: Commercial Multi-Tenant Hosting

> **Status:** Domain and enrollment registry implemented in `hostify-core::network`. No public control plane, authenticated HTTP endpoints, device channel, billing, or production DNS provisioning is shipped yet. An enrolled device is **not** automatically routable.

## Product contract

Each customer installs Hostify on **their own Windows computer**, and the website's files and execution stay on that machine. The central Hostify Network service will control customer accounts, unique subdomain allocations, device revocation, audit, billing and Cloudflare routing, not host customers' site files itself.

Examples using a **documentation-only** domain:

- `alpha.example.test` → Tenant A, Agent A, Website A
- `beta.example.test` → Tenant B, Agent B, Website B

The real `niga.online` zone and customer-owned sites are out of scope for development tests.

## Trust boundaries

**Central control plane (future service):**
- Owns the top-level Cloudflare API token in a server-side secret store, never distributes it to customers.
- Authenticates users with strong identity, per-tenant RBAC, MFA for administrators.
- Issues a random, short-lived, single-use enrollment invitation; SQLite contains **only SHA-256** of the invitation.
- Stores a customer device public key for *future* mutual-authentication.
- Reserves globally unique subdomain names inside a SQLite immediate transaction to avoid races.
- Enforces per-tenant device and hostname quotas.
- Keeps an append-only audit of registration, reservation, and revocation metadata (never secrets).
- Leaves all newly reserved hostnames **non-routable** until possession-of-key is verified via an authenticated gateway and the owner explicitly approves route activation.
- Revokes access fail-closed by disabling routes; does not silently delete other users' DNS records.

**Customer device (future restricted agent):**
- Has only its own device identity and a per-device connector credential, not the master Cloudflare API token.
- Serves only assigned websites on localhost; Caddy enforces Host allowlists.
- Establishes an **outbound-only** authenticated connector; does not open administrative ports to the internet.
- Runs Node/PHP workloads with restricted per-customer OS permissions, not LocalSystem.
- Supports versioned updates, explicit admin permissions and certificate/key rotation.

**Cloudflare / public access (future):**
- Route each reserved hostname to the tenant-owned tunnel only after explicit, durable approval, correct ownership, and healthy local origin verification.
- TLS public verification must check public DNS, certificate, HTTP routing and deployment nonce.
- A DNS write must use idempotent operation IDs and refuse unmanaged/conflicting records.
- Provide abuse controls, rate limiting, incident response, domain revocation and traceable ownership.

## Implemented registry invariants

1. Tenant IDs, agents, website IDs and invitations are unique.
2. Invitations expire within 1 hour, are one-use, and commit consumption and agent enrollment atomically.
3. Public key format must be 32 bytes, hex encoded and non-zero.
4. Different tenants cannot claim each other's devices or a hostname already reserved.
5. Reserved system names and non-ASCII DNS labels are rejected until IDNA handling is formally audited.
6. Device quota defaults to 3, hostname quota to 10; reservations count toward quota.
7. Revoked devices cannot have active routes. Their hostnames remain reserved to prevent takeover.
8. **No invocation** of `network::Registry` is exposed to the local desktop IPC or a public listener.
9. Hostname assignments never change live DNS from registry methods.
10. Cloudflare secret references, enrollment tokens and customer private keys are not stored in audit rows.

## Still required before commercial launch

- Central service deployment, HTTPS authentication and secure secret store.
- Device-side Ed25519 identity generation and challenge-signature proof or mTLS.
- Strong owner RBAC, identity verification, MFA, tenant-level audit access and account deletion.
- Verified end-to-end gateway and per-agent tunnel tokens with rotation/revocation.
- Legitimate external DNS/Cloudflare integration using service-owned credentials.
- Per-tenant isolation at operating system and process level; no untrusted workloads as SYSTEM.
- Plan limits, subscription/billing webhooks, metering and payments.
- Content abuse reporting, takedown response and rate limits.
- Cross-network tests with multiple real Windows computers, outages/restarts, stolen device revocation and attempted hostname takeover.
- Privacy notices, service terms, data retention and operational incident response.

**Never mark a Hostify Network registration as publicly available based on a reserved hostname or stored public key alone.**

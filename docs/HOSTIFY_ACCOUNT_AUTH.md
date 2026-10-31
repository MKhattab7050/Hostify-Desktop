# Hostify Network account authentication foundation

**Status: central-only Rust library, not an externally reachable service.**
This package must only run in a trusted control-plane process, never inside an untrusted tenant app or over the desktop IPC.

The `hostify_core::network_auth::Accounts` registry stores tenant-scoped account identities and invitations in the same SQLite database as `network::Registry`.

## Implemented

- An offline, strictly one-time tenant owner bootstrap. No second owner can replace the first. New operator-created tenants now commit the tenant row, initial owner and audit event in **one SQLite transaction**, preventing ownerless records after a failed insertion.
- Owner, admin and viewer roles with explicit capability checks. Only owners have billing permission. Admins may invite only viewers; owners may invite admins and viewers.
- 16-byte cryptographically random salts and 210,000-iteration PBKDF2-HMAC-SHA256 password derivation. Raw password strings are not persisted.
- 256-bit unpredictable login and invitation tokens with SHA-256 digests persisted in SQLite, not raw tokens.
- 12-hour expiring sessions; explicit logout invalidates a session, and cross-tenant access is denied.
- Per-account five-failure, 15-minute login lockout. This is not a substitute for per-IP or device abuse throttling on a public ingress.
- Invite-only member creation, expiring invitations, atomic single-use consumption, and tests for revoked sessions, account takeover attempts and RBAC.
- Member removal safety: owner/admin accounts can disable eligible members and revoke all their live sessions in one audited transaction. Admins can disable only viewers, nobody can disable an owner or themselves, and disabled members cannot sign in. This is a local control-plane operation, not public user management.
- **Transactional membership audit:** invitation creation and redemption each write an audit entry in the same SQLite transaction as the affected row. Failing the audit write rolls the operation back. Audit subjects use internal UUIDs, never invitation tokens, passwords or member emails. This is not yet a publicly exposed audit viewer.

## Security restrictions

- **No public account API, TLS server, MFA, password reset, self-registration or billing endpoints yet.**
- The `bootstrap_owner` method is a trusted operator action only and must not be exposed to public callers.
- The repository contains no example credentials for production and this library does not read any live Cloudflare secrets or customer files.
- The central operator must configure encrypted transport, secrets at rest, MFA or passkeys, audit recording, session fixation/CSRF protections, origin checks and rate limits before deployment.
- This library does not activate domain routes, create tunnels, or provision Cloudflare DNS. Verified device identities are still non-routable.
- The account table permits tenant-local email identities. Multi-tenant organization memberships and invitations need explicit migration before broad release.
- Run live Windows, multi-device and external-network integration tests with synthetic test hosts and independent users.

The `network_service::NetworkControlPlane` facade composes the account
permissions with the tenant registry. It requires a live account session before
issuing device invitations, reserving subdomains, issuing device-key challenges
or revoking devices, and rejects cross-tenant misuse. It still **cannot**
activate a public route and exposes no network listener. The operator-only
provisioning method must not be exposed through public HTTP.

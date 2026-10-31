# Hostify Security Center

**Current status: local read-only evidence, not a commercial hosting certification.**

The dashboard exposes `security_posture` via the user's existing local authenticated Tauri IPC. It does not accept unauthenticated remote requests.

For each website it checks local configuration evidence: an active release has matching build/trust state, whether Caddy's expected host-guard configuration exists, whether the origin is running when the site has a published URL, whether restricted dynamic execution in privileged service mode would be unsafe, and whether there is a local file snapshot.

Findings are deliberately separated into:

- `block`: locally confirmed risk, such as a published site with a stopped origin or a dynamic workload with untrusted active release.
- `warn`: risk requiring attention, such as absent file snapshots or unverified cross-tenant Windows process isolation.
- `unverified`: conditions requiring another trusted checker (public DNS, public HTTPS, Cloudflare tunnel, independent OS-level checks and off-device encrypted backups).

**No finding is auto-fixed.** No process is restarted, no database is accessed, no secret is revealed, and no DNS record is created or modified. The check only uses public site metadata, local Caddy configuration text, and the list of snapshot manifests.

The HTTP 421 foreign-host requirement, Windows ACLs, restricted OS user tokens, HTTPS correctness, and public isolation cannot be certified by a local configuration text scan. Run real multi-site integration tests and manual Windows VM/Cloudflare tests before allowing outside customers.

## Current limitations

- The local site status is an in-memory process observation, not a fresh independent public health check.
- Snapshot presence does not prove the content is restorable or copied off-device. Use the Time Machine checksum verification and separately test restoration.
- No independent authorization server, role separation, MFA, secret rotation or multi-tenant billing is implemented.
- A malicious customer application is not sandboxed from another customer on the same Windows user identity.

## Tests

`security_center::tests` asserts that stopped published origins, missing trust, privileged runtime and missing host guard are reported as blockers. It also verifies that configured local state never implies public HTTPS verification. CI adds a browser regression for the Security Center dashboard.

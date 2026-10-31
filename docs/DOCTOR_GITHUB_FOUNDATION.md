# Hostify Doctor and GitHub Webhook Verification

## Implemented

### Hostify Doctor (offline diagnostics)

The Doctor is a local, *read-only* deterministic rules engine, not an external LLM. It parses the website's own redacted app/build/process logs for specific known signatures and suggests next actions without returning raw log content, running arbitrary commands, or changing database schema, customer code, DNS, or deployment trust.

Known signatures include Node 26 SlowBuffer dependency incompatibility, missing packages, occupied local ports, PHP/application exceptions, MySQL authentication failures, DNS propagation, insufficient disk, and foreign Caddy hosts.

A diagnostic report says **no known signatures** when nothing matches; that is not a guarantee of a healthy site.

### GitHub Webhook Guard (authentication only)

`hostify_core::github_webhooks::verify_push` validates GitHub's `X-Hub-Signature-256` using SHA-256 HMAC, checks push events, an allowed repository and branch, delivery UUID, non-zero commit hash, and rejects deleted/force-push/fork events.

**No webhook HTTP listener is deployed; no source download, deployment queue, build or automatic publish is triggered by this pure helper.**

## Remaining before GitHub Auto Deploy

1. A dedicated authenticated ingress endpoint with TLS, strict Content-Type, body limits and IP/rate controls.
2. Durable anti-replay ledger keyed by `X-GitHub-Delivery` (per-delivery idempotency).
3. Secure installation/revocation of per-repository webhook secrets in a secret store.
4. Immutable commit checkout via verified Git refs and content hash, with safe repository allowlist.
5. Explicit opt-in from the site's owner, no silent dependency scripts or code trust.
6. Asynchronous immutable release builds, vulnerability/dependency scans, isolated execution accounts, artifact provenance.
7. Preview URL verified before exposure, health-gated cutover, stop-on-failure rollback.
8. Windows and external GitHub end-to-end integration tests without touching production domains.

## Restrictions

No claim of zero downtime, autonomous AI repairs, or production-grade webhook deployment follows from this work. The future LLM layer requires a privacy consent flow, explicit risk approval, strict tool authorization and redaction tests.

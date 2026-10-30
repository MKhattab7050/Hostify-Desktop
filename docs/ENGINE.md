# Hostify engine implementation status

The engine and Tauri shell are not yet connected to the core and do not provide a working hosting product. The core work in this pass is incomplete and must not be used for production deployment.

## Core work

Core contains SQLite migration, paths, staged directory and ZIP import, Caddyfile generation and port selection, a keyring-backed secret store, a Cloudflare API client, checksum-gated component downloads, and a deployment implementation with SQLite snapshots and rollback. Public verification compares a successful HTTPS response body with the local index file hash. The Caddyfile also emits an `X-Hostify-Verify` header. Cloudflare and component unit tests use mocks rather than the network.

## Known gaps

Deployment is not yet safe to run: ownership cannot be reliably recovered if a process stops between a remote create operation and saving its result, and automatic tunnel deletion is withheld because the implementation cannot establish that a newly created tunnel is unused by other actors. Reusing a tunnel replaces its ingress and failure rollback does not restore its former configuration. Recovery currently rolls back running deployments only; incomplete rollback after a failure is not retried automatically. Full success, every failure step, and interruption recovery are not yet tested. SiteRuntime implementations must return whether they actually started a resource, to avoid stopping a previously running one.

The import code has not been security audited. Directory traversal checks metadata before opening files and is susceptible to concurrent filesystem replacement (TOCTOU); managed destination ancestors are not checked for reparse points. ZIP symlink behavior and Windows filename edge cases need further adversarial tests. Do not import untrusted sources.

Component acquisition expects a cloudflared `.sha256` asset; it fails closed when unavailable. Real release asset formats and installation replacement on Windows have not been verified. Component downloads do not yet report byte-level progress. Caddy and cloudflared binary execution has not been tested.

The engine supervisor and child Job Object, named pipe ACL and JSON-RPC server, event forwarding, service and scheduled-task management, tray behavior, installer hooks, and IPC commands remain unimplemented. Port selection is advisory and can race with another process.

## Verification

Run `cargo build -p hostify-core --offline` and `cargo test -p hostify-core --offline` with `%USERPROFILE%\.cargo\bin` on PATH. Tests use local mocks and no real credentials. Rustfmt is unavailable in the current toolchain.

No operation has been verified against real Cloudflare, release downloads, Windows Credential Manager, installed Caddy or cloudflared, a Windows reboot, Windows Service, scheduled task, or installer. The API behavior, permissions, release checksum availability, and public propagation need real integration verification before release.

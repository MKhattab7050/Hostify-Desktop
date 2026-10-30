# Hostify engine implementation status

This is an incomplete backend foundation, not a usable or secure release. The engine binary exits with an error and does not start hosting. The Tauri scaffold does not yet forward IPC commands. Do not use the import code with untrusted input until the issues below are resolved and tested.

## Implemented foundation

The workspace contains `hostify-core`, `hostify-engine`, and the existing Tauri crate. Core contains the initial SQLite schema migration, local data paths, an import staging/swap implementation, and Caddyfile and port selection helpers. The database stores credential references, not credentials. Port selection is advisory: another process can claim the port before Caddy binds it.

The import code checks archive paths, entry types, compression ratio and size limits, and stages files before swapping the current release. It has not been security audited. In particular, directory traversal currently checks metadata before opening files and is susceptible to a concurrent filesystem replacement (TOCTOU); managed destination ancestors are not checked for reparse points. ZIP extraction and Windows filename corner cases also need further adversarial tests. These issues must be addressed before importing untrusted sources.

## Missing work

Cloudflare API, credential storage, component downloads and pinned verification, deployment persistence and rollback, HTTP verification, the supervisor and child Job Object, named pipe ACL and JSON-RPC server, event forwarding, service and scheduled-task management, tray behavior, installer hooks, and the contract commands are not implemented. The existing UI shell is still the initial scaffold. No DNS or external state is changed by this code.

## Running and testing

Add `%USERPROFILE%\.cargo\bin` to `PATH`, then run `cargo build --workspace` and `cargo test --workspace` from the repository root. Dependencies must be available from crates.io or a populated Cargo cache. The initial attempt in this environment could not reach crates.io and `rusqlite` was not cached, so neither build nor tests were verified. `cargo fmt` was also unavailable because the rustfmt component was not installed.

## Not verified

Nothing has been verified against real Cloudflare, an installed Caddy or cloudflared, a Windows reboot, a Windows Service, a scheduled task, an NSIS installer, or real user credentials. The core implementation and tests could not be compiled in the current environment.

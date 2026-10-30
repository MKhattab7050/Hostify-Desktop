# Codex Task 3A — `hostify-engine` crate ONLY

Read `docs/codex-task-3-engine.md` (items 1, 2, 3, 6 and the BUILD ENVIRONMENT section apply to you), `docs/ARCHITECTURE.md`, `docs/IPC.md`, `docs/ENGINE.md`, and the public API of `crates/hostify-core` first. Always use `cargo ... --offline` with `%USERPROFILE%\.cargo\bin` on PATH. No network.

## FILE OWNERSHIP (strict, another agent edits `src-tauri/**` in parallel)
Edit ONLY `crates/hostify-engine/**` and `docs/ENGINE.md`. Do not touch `src-tauri/**`, `src/**`, root `Cargo.toml`, or `crates/hostify-core/**` (request core changes in `docs/CONTRACT-REQUESTS.md`).

## Pipe wire protocol (FIXED — the Tauri shell is being written against this)
Pipe: `\\.\pipe\hostify-engine-<current user SID string>`; DACL = current user + SYSTEM only. Newline-delimited JSON, UTF-8, max 1 MiB per line.
* Request: `{"id": <u64>, "method": "<ipc command name from IPC.md>", "params": { ...camelCase args exactly as in IPC.md... }}`
* Response: `{"id": <u64>, "result": <json>}` or `{"id": <u64>, "error": "<user-presentable string>"}`
* Server push events (after the client sends method `subscribe`): `{"event": "site-status"|"deployment-update"|"activity"|"components-progress", "payload": <json>}`
* Also implement method `ping` -> `"pong"`. Unknown methods -> error. All JSON field names camelCase per IPC.md.

## Deliver
Engine bin + lib: single instance mutex, Supervisor implementing core's `SiteRuntime` (caddy + cloudflared children via `std::process::Command`/tokio, no shell, `CREATE_NO_WINDOW`, Job Object KILL_ON_JOB_CLOSE, `TUNNEL_TOKEN` via env only, exponential backoff restart, health checks, port-conflict statusDetail), startup `recover_interrupted()` + reconcile, graceful shutdown, the pipe server above implementing EVERY command in IPC.md (use `sysinfo` for `system_stats`), CLI subcommands `install-task`/`uninstall-task` (per-user Scheduled Task at logon via `schtasks`, XML definition, no admin), `install-service`/`uninstall-service`/`--service`, `status`, size-based log rotation. Add `--data-dir <path>` (and env `HOSTIFY_DATA_DIR`) so tests and smoke runs use a temp dir.
Tests: backoff, request validation, port conflict, and an integration test that starts the server on a uniquely named test pipe with a fake `SiteRuntime`, then calls ping/list_sites/add_site/start_site/stop_site/remove_site over the pipe.

## Definition of done
`cargo build -p hostify-engine --offline` and `cargo test -p hostify-engine --offline` pass; paste the real output. Note anything unverified in `docs/ENGINE.md`. Work in small steps and compile often; do not stop with partial work.

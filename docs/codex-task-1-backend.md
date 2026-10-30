# Codex Task 1 — Rust backend for Hostify Desktop

Repo: D:\Hostify-Desktop (Tauri 2 scaffold already present). Read `docs/ARCHITECTURE.md` and `docs/IPC.md` FIRST; they are binding. Rust is at `%USERPROFILE%\.cargo\bin` (add to PATH). Windows 11 x64, MSVC toolchain.

## FILE OWNERSHIP (strict)
You own and may edit ONLY: `Cargo.toml` (new workspace root), `crates/**`, `src-tauri/**`, `docs/ENGINE.md` (new), `scripts/**` (new).
Do NOT touch: `src/**`, `package.json`, `index.html`, `vite.config.ts`, `docs/ARCHITECTURE.md`, `docs/IPC.md` (another agent is editing the frontend concurrently). If you need a contract change, write it in `docs/CONTRACT-REQUESTS.md` and keep going.

## Deliver (in priority order; commit-quality code, `cargo build` and `cargo test` must pass at the end)
1. **Workspace**: root Cargo workspace with `crates/hostify-core` (lib), `crates/hostify-engine` (bin `hostify-engine`), and the existing `src-tauri` as members. Keep `src-tauri` crate lib name `hostify_lib`.
2. **hostify-core**
   - `db`: rusqlite (bundled) + versioned migrations. Tables: websites, domains, deployments, tunnel_connections, application_settings, activity_logs. NO secrets in SQLite.
   - `paths`: `%LOCALAPPDATA%\Hostify` layout from ARCHITECTURE.md.
   - `import`: validate + import a directory or ZIP. ZIP-slip protection (reject `..`, absolute paths, drive letters, UNC, reserved Windows names, NTFS ADS `:`), reject symlinks/reparse points, zip-bomb limits (total uncompressed size, file count, ratio), no executing anything. Extract to a temp dir then atomic rename into `sites\<id>\current`, keeping the previous one in `releases\`. Detect single top-level folder in ZIPs and flatten it. Report `SourceReport`.
   - `caddy`: generate a Caddyfile (admin off, one `127.0.0.1:<port>` site block per site, file_server only, hide dotfiles, no browse, sensible security headers, per-site access log to `logs\`). Port allocation with conflict detection (try bind; skip busy ports).
   - `cloudflare`: reqwest (rustls) client for token verify, accounts, zone lookup (walk up labels for subdomains), tunnel create/reuse (idempotent by name `hostify-<machine>`), tunnel config ingress (hostname -> `http://127.0.0.1:<port>`, final `http_status:404`), DNS CNAME create with conflict detection (never overwrite a record we didn't create; tag with comment `managed-by=hostify`), delete only our own records. Map API errors to friendly messages. Unit-test with a local mock HTTP server (e.g. wiremock) — mocks only in tests.
   - `secrets`: Windows Credential Manager via `keyring`.
   - `deploy`: the persistent, resumable state machine from ARCHITECTURE.md, emitting `Deployment` snapshots; rollback only what this deployment created; success only after real local + public HTTPS verification (retry with backoff for DNS propagation; give a clear "pending propagation" result rather than false success).
   - `components`: download caddy and cloudflared Windows amd64 from their official GitHub releases, verify SHA-256 (pin the hashes you fetch from the release assets/checksums; fail closed), into `bin\`. Emit progress.
3. **hostify-engine** (bin): single-instance (named mutex), owns DB + supervisor. Supervisor starts/stops caddy and cloudflared with `std::process::Command` (NO shell, args as arrays, `CREATE_NO_WINDOW`, assign to a Job Object with KILL_ON_JOB_CLOSE so children never orphan), restarts on crash with exponential backoff, graceful stop, health checks (HTTP GET to each site port, tunnel connection state via cloudflared metrics or API). On start it reconciles: every site with `autoStart` and every published deployment is brought back up. Handles Ctrl+C / console close / WM_QUERYENDSESSION gracefully. Control server on named pipe `\\.\pipe\hostify-engine-<SID>` with an explicit DACL (current user + SYSTEM) — newline-delimited JSON-RPC implementing every command in IPC.md. `--service` flag runs as a Windows Service (`windows-service` crate); subcommands `install-task` / `uninstall-task` (per-user Scheduled Task at logon via `schtasks`, no admin) and `install-service` / `uninstall-service` (requires elevation). Log rotation (size-based, keep N). Real CPU/RAM/disk via `sysinfo`.
4. **src-tauri** (UI shell): register ALL commands in IPC.md as `#[tauri::command]` that forward over the pipe; if the engine isn't running, spawn it detached (`hostify-engine.exe` next to the app exe) and wait for the pipe. Forward engine events to the frontend via `app.emit`. Plugins: dialog (for `pick_path`), opener. Tray icon with Open / Quit-UI; closing the window hides to tray when `closeToTray` (default true) — hosting keeps running. Strict CSP in `tauri.conf.json` (self + ipc), `capabilities` minimal. Set bundle to NSIS (`perUser`), include `hostify-engine.exe` as a sidecar/resource, and write an NSIS hook that registers the logon task on install and removes it on uninstall (stopping the engine first), leaving `%LOCALAPPDATA%\Hostify\sites` untouched unless the user ticks a "delete my website data" option.
5. **Tests**: unit tests for zip-slip/symlink/bomb handling, Caddyfile generation, port conflicts, Cloudflare client (mock server) incl. DNS conflict and invalid token, deployment state machine incl. failure + rollback, migrations.
6. **docs/ENGINE.md**: how the engine works, how to run/test it, and an explicit list of what is NOT yet verified against real Cloudflare/Windows reboot.

## Rules
- No `unwrap()` on fallible runtime paths. Typed errors (`thiserror`) mapped to friendly strings at the IPC edge.
- Never log secrets. Tunnel token only via env var `TUNNEL_TOKEN`.
- No admin requirement for the default path.
- Do not claim anything works unless you ran it. Run `cargo build --workspace` and `cargo test --workspace`; report the real results, including failures, in your final message.
- Work autonomously; do not stop to ask questions. Make sound decisions and note them in docs/ENGINE.md.

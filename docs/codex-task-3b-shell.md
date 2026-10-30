# Codex Task 3B — Tauri shell + installer config ONLY

Read `docs/codex-task-3-engine.md` (items 4, 5, 7 and the BUILD ENVIRONMENT section apply to you), `docs/ARCHITECTURE.md` and `docs/IPC.md`. Always use `cargo ... --offline` with `%USERPROFILE%\.cargo\bin` on PATH. No network. The frontend in `src/**` is finished and calls the commands exactly as in IPC.md.

## FILE OWNERSHIP (strict, another agent edits `crates/hostify-engine/**` in parallel)
Edit ONLY `src-tauri/**`, `scripts/**`, `docs/INSTALLER.md`. Do not touch `src/**`, `crates/**`, root `Cargo.toml`, `package.json`.

## Pipe wire protocol (FIXED — the engine is being written against this)
Pipe `\\.\pipe\hostify-engine-<current user SID string>`. Newline-delimited JSON, UTF-8.
* Request `{"id": <u64>, "method": "<command name>", "params": {camelCase args}}`; response `{"id", "result"}` or `{"id", "error": "<string>"}`.
* After sending method `subscribe`, the server pushes `{"event": "site-status"|"deployment-update"|"activity"|"components-progress", "payload": ...}` lines; re-emit each with `app.emit(event, payload)`.
* `ping` -> `"pong"`.

## Deliver
1. Register EVERY command in IPC.md as `#[tauri::command]` (snake_case command names, camelCase args as in the table) that forwards over the pipe via one reusable async client with reconnect and a request timeout; error strings returned to the UI as `Err(String)`. `pick_path` is handled locally with `tauri-plugin-dialog` (directory or `.zip` filter). If the pipe is absent, spawn `hostify-engine.exe` (beside the app exe; in dev, `target/debug/hostify-engine.exe`) detached with `CREATE_NO_WINDOW`, and retry connecting for up to ~10s. Look up the current user SID with the Windows API (`windows-sys` is NOT a dependency of src-tauri — compute the pipe name by asking `whoami /user`-free means, or add the SID helper via the existing deps; if impossible, document the need in `docs/CONTRACT-REQUESTS.md` and have the engine publish its pipe name in `%LOCALAPPDATA%\Hostify\run\engine.pipe`, which the shell reads).
2. Tray icon (Open Hostify / Quit window only — hosting continues); window close hides to tray when `closeToTray` (read via `get_settings`, default true).
3. `capabilities/default.json` minimal (core:default, dialog, opener open-url, event listen, webview drag-drop). Strict CSP: `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self' data:; connect-src ipc: http://ipc.localhost`.
4. Installer config: NSIS `installMode: currentUser`, icons generated from a simple original SVG logo (a rounded square with the house/arch mark used in `src/components/Sidebar.tsx`; produce PNG/ICO files by script, no network), bundle `hostify-engine.exe` as a resource via `scripts/prepare-sidecar.ps1`, NSIS hooks in `src-tauri/installer-hooks.nsh` (post-install: run `hostify-engine.exe install-task` and start it; pre-uninstall: stop engine + `uninstall-task`; keep `%LOCALAPPDATA%\Hostify\sites` unless the user opts in to delete it), product name "Hostify". Write `docs/INSTALLER.md`: unsigned-build notice, Caddy/cloudflared licences (Apache-2.0) and the choice to download them at first run rather than bundle.
5. `scripts/smoke.ps1`: builds, starts the engine headless with `--data-dir` temp, adds `scripts/sample-site/` (create a small sample site), curls the local preview, stops it, prints PASS/FAIL honestly.

## Definition of done
`cargo build -p hostify-desktop --offline` passes (the shell compiles; `pnpm build` output `dist/` already works — do not run installer bundling, the orchestrator does that with network). Paste real command output. Work in small steps; do not stop with partial work.

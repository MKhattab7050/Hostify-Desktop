# Hostify Desktop — Architecture (V1)

## Processes

```
 Hostify.exe (Tauri UI shell) --named pipe (per-user ACL)--> hostify-engine.exe (headless, owns all state)
                                                               |- Caddy        (static file server, 127.0.0.1 only)
                                                               '- cloudflared  (one connector per tunnel)
```

* **UI shell** (`src-tauri`): thin. Every Tauri command forwards to the engine over the pipe. Closing the UI never stops hosting. Tray icon; "Quit UI" is not "Stop hosting".
* **Engine** (`crates/hostify-engine`): long-running supervisor. Runs per-user (no admin) via a Scheduled Task at logon; optional real Windows Service mode (`--service`, `windows-service` crate) when installed elevated, for pre-login boot. Restarts crashed children with backoff, reconciles desired state on start, stops children gracefully on shutdown.
* **Core** (`crates/hostify-core`): library — SQLite (rusqlite + migrations), models, safe import (dir/ZIP), Caddyfile generation, Cloudflare API client, deployment state machine, credential store (Windows Credential Manager via `keyring`).
* **Control channel**: named pipe `\\.\pipe\hostify-engine-<user SID>`, DACL = current user + SYSTEM only, newline-delimited JSON-RPC. No TCP admin port, ever. Caddy admin API is disabled (`admin off`); Caddy binds `127.0.0.1` only; cloudflared is the only public path and only maps hostnames to local site ports.
* **Isolation**: sites are inert static files. One loopback listener per site, `file_server` only, no `php`/`exec`, dotfiles denied, directory browsing off.

## Data

`%LOCALAPPDATA%\Hostify\`: `hostify.db`, `sites\<id>\current\` (served files), `sites\<id>\releases\` (previous, for rollback), `bin\` (caddy.exe, cloudflared.exe — verified by pinned SHA-256), `logs\` (rotated), `run\`.

Secrets (Cloudflare token, tunnel tokens) live ONLY in Windows Credential Manager. SQLite stores a credential reference id.

## Deployment state machine (persistent, idempotent, resumable)

`validating > staged > local_serving > local_verified > tunnel_ready > dns_pending_consent > dns_configured > connector_running > public_verifying > live`. Any failure becomes `failed(step, reason)` and rolls back only resources *created by this deployment*. DNS is never changed without an explicit consent flag from the UI; existing records are never overwritten (conflict gives an error with details). Success is reported only after a real HTTPS fetch of the public URL returns the expected content.

## Cloudflare

Auth: scoped API token (UI shows a guide: Account > Cloudflare Tunnel:Edit, Zone > DNS:Edit, Zone > Zone:Read). Token verified via `/user/tokens/verify`; zone found via `/zones?name=`; tunnels via `/accounts/{id}/cfd_tunnel` (remotely managed, `config_src=cloudflare`); ingress via `/cfd_tunnel/{id}/configurations`; DNS CNAME `<tunnel-id>.cfargotunnel.com` (proxied). Connector: `cloudflared tunnel run` with the token passed via env `TUNNEL_TOKEN`, never argv.

## Frontend

React 19 + TS + Tailwind 4 + zustand. `src/lib/api.ts` is the only module that calls `invoke`; it falls back to an in-memory mock when not running in Tauri (browser dev / design review).

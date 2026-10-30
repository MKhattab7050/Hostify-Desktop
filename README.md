# Hostify Desktop

Host static websites from your Windows PC. Pick a folder or ZIP, preview it locally, connect a domain on Cloudflare, and publish over HTTPS — no router changes, no public IP.

**Status: V1 MVP, unsigned.** Local hosting, the installer and background hosting are verified on Windows 11. Publishing through Cloudflare has **not** been run against a real account yet — see [Verification status](#verification-status).

## Install

1. Run `Hostify_0.1.0_x64-setup.exe` (per-user install, no administrator rights). Windows SmartScreen will warn about an unknown publisher because the build is unsigned: choose *More info → Run anyway*.
2. Open Hostify. On the Dashboard choose **Install components**. Hostify downloads Caddy (web server) and cloudflared (tunnel) from their official GitHub releases and installs each only if it matches the checksum the publisher ships.
3. **Add website** → choose a folder or ZIP → preview it.

Closing the window hides Hostify to the tray; hosting continues. The engine restarts at every Windows sign-in (per-user scheduled task). Uninstalling keeps your website data unless you tick *Delete my website data*.

## Put a site on your domain (needs Cloudflare)

Requirements: a domain whose DNS is already on Cloudflare (free plan works).

1. Cloudflare dashboard → *My Profile → API Tokens → Create Token → Custom token*.
2. Permissions: **Account · Cloudflare Tunnel · Edit**, **Zone · DNS · Edit**, **Zone · Zone · Read**. Limit *Zone Resources* to the one domain and *Account Resources* to your account.
3. In Hostify: **Domains → Connect Cloudflare**, paste the token (stored in Windows Credential Manager, never in the database).
4. **Publish website** → enter the hostname (e.g. `www.example.com`) → review the exact DNS record Hostify will add → tick consent → publish. Hostify reports success only after fetching `https://<hostname>/` and getting your site back. If the hostname already has a DNS record, Hostify stops and shows it; it never overwrites records it did not create.

## Develop

Prerequisites: Node 22+, pnpm, Rust (MSVC toolchain), Visual Studio C++ Build Tools, WebView2.

```powershell
pnpm install
pwsh scripts/vendor.ps1        # optional: vendor crates for offline builds
pnpm tauri dev                 # UI + engine
cargo test --workspace         # Rust tests
pwsh scripts/smoke.ps1 -ComponentsDir <dir with caddy.exe>   # real local-hosting check
pnpm tauri build               # installer → target/release/bundle/nsis/
```

Layout: `src/` React UI (`src/lib/api.ts` is the only IPC caller; it uses an in-memory mock in a plain browser) · `src-tauri/` desktop shell · `crates/hostify-core` (SQLite, import, Caddyfile, Cloudflare client, deploy state machine) · `crates/hostify-engine` (supervisor, named-pipe control server, autostart) · `docs/` architecture, IPC contract, engine and installer notes.

## Verification status

| Area | Status |
|---|---|
| Static import (folder/ZIP), zip-slip, zip-bomb, link rejection | Unit-tested |
| Local hosting through real Caddy (add, start, HTTP preview, stop) | **Verified** (`scripts/smoke.ps1`) |
| Real Caddy / cloudflared download with checksum verification | **Verified** against Caddy v2.11.7 and cloudflared 2026.10.6 |
| NSIS install, engine autostart task, UI launch, uninstall cleanup, data retention | **Verified** on one Windows 11 machine (not a clean VM) |
| UI keeps hosting after the window is closed; engine restarts via logon task | **Verified** (task triggered manually) |
| Cloudflare client and deploy state machine | Unit-tested against mock servers only |
| **Publish to a real domain with HTTPS** | **Not verified** — needs a real Cloudflare account, token and domain |
| **Windows reboot / sign-in recovery** | Not verified with a real reboot |
| Clean-VM install, Windows Service mode, code signing | Not verified / not done |
| Network-loss and tunnel-reconnect behavior against real Cloudflare | Not verified |

To complete verification: connect a scoped token, publish a test site to a spare subdomain, open the HTTPS URL from another network, reboot, confirm the site returns without opening Hostify, then unpublish and confirm the DNS record is removed.

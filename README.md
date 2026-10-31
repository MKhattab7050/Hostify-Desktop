# Hostify Desktop

Host static websites from your Windows PC. Pick a folder or ZIP, preview it locally, connect a domain on Cloudflare, and publish over HTTPS — no router changes, no public IP.

**Current code: 2.0.0-rc.1 (unsigned pre-release).** Version **0.2.0** passed a limited set of Windows 11 and real-domain tests on one machine. Those older results do **not** certify this release candidate or commercial multi-tenant hosting. Real reboot, clean-machine installation, independent external-network verification, and customer workload isolation are still outstanding. See [docs/VERIFICATION.md](docs/VERIFICATION.md).

## Install

1. Run `Hostify_0.2.0_x64-setup.exe` (per-user install, no administrator rights). Windows SmartScreen will warn about an unknown publisher because the build is unsigned: choose *More info → Run anyway*.
2. Open Hostify and follow the first-run steps. Hostify downloads Caddy (web server) and cloudflared (tunnel) from their official GitHub releases and installs each only if it matches the checksum the publisher ships.
3. **Add website** → choose a folder or ZIP → preview it.

Closing the window hides Hostify to the tray; hosting continues. The engine restarts at every Windows sign-in (per-user scheduled task), so after a restart your websites return once you sign in. Uninstalling keeps your website data unless you tick *Delete my website data*.

## Put a site on your domain (needs Cloudflare)

Requirements: a domain whose DNS is already on Cloudflare (free plan works).

1. Cloudflare dashboard → *My Profile → API Tokens → Create Token → Custom token*.
2. Permissions: **Account · Cloudflare Tunnel · Edit**, **Zone · DNS · Edit**, **Zone · Zone · Read**. Limit *Zone Resources* to the one domain and *Account Resources* to your account.
3. In Hostify: **Domains → Connect Cloudflare**, paste the token (stored in Windows Credential Manager, never in the database).
4. **Publish website** → enter the hostname (e.g. `www.example.com`) → review the exact DNS record Hostify will add → tick consent → publish. Hostify reports **Published** only after fetching a one-time marker from your website over validated HTTPS, then your home page. Waiting for DNS or HTTPS never deletes anything: after 30 minutes publishing pauses, keeps everything, and offers **Check again** and **Unpublish**. If the hostname already has a DNS record, Hostify stops and shows it; it never overwrites records it did not create.

## Develop

Prerequisites: Node 22+, pnpm, Rust (MSVC toolchain), Visual Studio C++ Build Tools, WebView2.

```powershell
pnpm install
pwsh scripts/vendor.ps1        # optional: vendor crates for offline builds
pnpm tauri dev                 # UI + engine
cargo test --workspace         # Rust tests
pnpm lint                      # ESLint
pnpm build; pnpm exec vite preview --port 4173   # then, in another terminal:
pnpm test:ui                   # UI tests on the production frontend (Microsoft Edge, mock data)
cargo test -p hostify-core --lib -- --ignored live_read_only   # read-only checks against the real internet
pwsh scripts/smoke.ps1 -ComponentsDir <dir with caddy.exe>   # real local-hosting check
pnpm tauri build               # installer → target/release/bundle/nsis/
```

Layout: `src/` React UI (`src/lib/api.ts` is the only IPC caller; it uses an in-memory mock in a plain browser) · `src-tauri/` desktop shell · `crates/hostify-core` (SQLite, import, Caddyfile, Cloudflare client, deploy state machine) · `crates/hostify-engine` (supervisor, named-pipe control server, autostart) · `docs/` architecture, IPC contract, engine and installer notes.

## What's experimental in V2+

- **Fast Publish and Standard Publish** share guarded deployment logic. Untrusted app code, DNS changes and new database provisioning still require explicit consent. See [Fast and Standard Publish](docs/FAST_STANDARD_PUBLISH.md).
- **Guardian, Time Machine, App Catalog, Doctor and Security Center** provide local support, backups, templates and diagnostics. Security Center is read-only and does **not** prove Windows process isolation or public reachability.
- **GitHub push inbox** stores verified, opt-in review requests. It is not a public webhook endpoint or an automatic checkout/build/deployment pipeline.
- **Hostify Network** currently has a local-only development control plane for device enrollment and reserved subdomains. A reserved name is **not** publicly routable. No production-grade multi-tenant gateway, per-customer sandbox or billing service is shipped. See [Hostify Network](docs/HOSTIFY_NETWORK.md) and [local control plane](docs/NETWORK_CONTROL_PLANE.md).

Do **not** expose the local Hostify control plane through a tunnel or sell multi-tenant hosting from this candidate.

## Verification status

The table below records the **0.2.0 baseline**, not a complete acceptance test for `2.0.0-rc.1`.

| Area | Status |
|---|---|
| Static import (folder/ZIP), zip-slip, zip-bomb, link rejection | Unit-tested |
| Local hosting through real Caddy (add, start, HTTP preview, stop) | **Verified** (`scripts/smoke.ps1`) |
| Real Caddy / cloudflared download with checksum verification | **Verified** against Caddy v2.11.7 and cloudflared 2026.10.6 |
| NSIS install, engine autostart task, UI launch, uninstall cleanup, data retention | **Verified** on one Windows 11 machine (not a clean VM) |
| UI keeps hosting after the window is closed; engine restarts via logon task | **Verified** (task triggered manually) |
| Forced engine kill leaves no orphaned Caddy; upgrade and reinstall keep data | **Verified** |
| Cloudflare client and publish state machine | Unit-tested against mock servers |
| Public verifier's DNS, TLS and marker checks | **Verified** read-only against the real internet |
| **Publish to a real domain with HTTPS** | **Verified** on one domain, from the publishing PC; not yet from another network |
| Forced engine kill while published | **Verified**: public site back in 9.1 s |
| **Windows reboot / sign-in recovery** | Not verified with a real reboot |
| Clean-VM install, code signing | Not verified / not done |
| Network-loss and tunnel-reconnect behavior against real Cloudflare | Not verified |

The full report and the steps that finish verification are in [docs/VERIFICATION.md](docs/VERIFICATION.md).

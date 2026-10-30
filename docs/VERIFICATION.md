# Verification report, 0.2.0

Run on 2026-10-30 on one Windows 11 Pro machine that already had Hostify installed. This was not a clean VM.

## Bottom line

**Public HTTPS publishing works on a real domain.** `niga.online` reached **Published** through the new verifier, and a separate `curl` request received the full page over validated TLS. That request came from the publishing PC. A check from another network and a real Windows restart are still outstanding.

The first real publish found a bug no earlier test could: published sites were blank, because Caddy answered 200 with an empty body to any hostname other than `127.0.0.1`. The marker check refused to call that published. The real Caddy test now sends a public hostname.

## Tested

### Automated

| Check | Result |
|---|---|
| TypeScript, ESLint | Clean |
| `cargo fmt --check` | Clean |
| Clippy | 0 errors, style warnings remain |
| Rust unit and integration tests | 126 passed |
| Live read-only network tests (`--ignored`) | 3 of 3 passed |
| UI tests on the production frontend, mock data | 15 of 15 passed |
| `pnpm audit --prod` | No known vulnerabilities |
| `pnpm tauri build` | Produced `Hostify_0.2.0_x64-setup.exe` |

The UI tests prove interface behaviour only. They do not exercise the engine or Cloudflare.

### Live network, read-only

- Parsed real authoritative DNS answers and both DNS-over-HTTPS providers.
- Rejected the marker on an unrelated site that returned a non-matching response (cloudflare.com, 301).
- Refused wrong-host, expired and self-signed certificates (badssl.com).
- Reached a pinned address when the system resolver failed.

### Real publish, `niga.online`

| Check | Result |
|---|---|
| Zone, DNS record, connector (4 connections), public DNS | Passed |
| TLS, HTTP and exact one-time marker | Passed at 104.21.3.188 |
| Public page compared with the local page | Identical, 71,358 bytes |
| `/.env` on the public address | 404 |
| Site port listens only on loopback | `127.0.0.1:9001` |
| Marker removed after success | Passed |
| Connector reconnects after an upgrade while publishing is paused | Passed, 4 connections |
| Engine force-killed while published, restarted by its task | Public site back in 9.1 s; still published, same tunnel, 4 Caddy processes, no orphans |
| PC with no IPv6 | Passed over IPv4 |

### Installed app, real engine, real Caddy, real data

| Check | Result |
|---|---|
| Upgrade over the previous install | Passed |
| Database migration | Passed; 4 websites, deployments and activity kept |
| Cloudflare connection after upgrade | Still connected |
| All 4 websites respond on their local ports | HTTP 200 |
| `/.env` and unknown `.well-known` paths | 404 |
| Marker file served with exact body and `no-store`; marker folder not listable | Passed |
| Closing the window keeps hosting | HTTP 200 afterwards |
| Force-killing the engine leaves no orphaned Caddy | 4 before, 0 after |
| Logon task restarts the engine and websites | HTTP 200 afterwards; task run by hand, not by a real sign-in |
| Uninstall stops everything and removes the task and program | Passed; data kept |
| Reinstall onto kept data | All 4 websites back |
| Desktop window driven over a loopback debug port | Every page loaded real data, no console errors |
| Token-like strings in logs and database | None found |
| Idle use | Engine 16 MB, 0.5 CPU seconds per 20 seconds; Caddy about 39 MB per website |

## Not tested

- Opening the published site from another network.
- A real Windows restart and sign-in.
- Installation on a clean VM or a second machine.
- Losing and regaining the internet with a live connector.
- Unpublishing `niga.online` with this build. The earlier attempt was undone on the previous build.
- Delete-my-data on uninstall (the checkbox has no silent-mode path, so it needs a person).
- High contrast, screen readers and display scaling on real hardware.

## Blocked

- **Code signing:** needs a certificate, which is a paid service. Windows SmartScreen warns about the unsigned installer.

## Known limitations

- Hosting returns after a restart only once you sign in to Windows, because the engine is a logon task. Windows service mode is not offered, as a service can't read your Cloudflare token.
- The very first verification on a new hostname can pause if a resolver cached the earlier missing record. It keeps checking and deletes nothing.
- File updates replace the site with two renames, so a request can briefly see no site, and a power cut between them isn't recovered automatically.
- Clippy still reports style warnings, mostly nested `if` statements.

See [AUDIT.md](AUDIT.md) for the area-by-area audit.

## To finish verification

1. Open the HTTPS address from another network, such as a phone on mobile data.
2. Restart Windows, sign in without opening Hostify, and check the address again.

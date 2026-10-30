# Verification report, 0.2.0

Run on 2026-10-30 on one Windows 11 Pro machine that already had Hostify installed. This was not a clean VM.

## Bottom line

**Public HTTPS publishing has not been proven with the new verifier.** Every layer below it has been tested, but a real publish has not succeeded yet. It needs one person to press Publish on a real domain, because automated changes to DNS were blocked in the test environment.

## Tested

### Automated

| Check | Result |
|---|---|
| TypeScript, ESLint | Clean |
| `cargo fmt --check` | Clean |
| Clippy | 0 errors, style warnings remain |
| Rust unit and integration tests | 123 passed |
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

- A successful public HTTPS publish with the new verifier.
- A real Windows restart and sign-in.
- Installation on a clean VM or a second machine.
- Losing and regaining the internet with a live connector.
- An engine crash while a live tunnel is connected.
- Delete-my-data on uninstall (the checkbox has no silent-mode path, so it needs a person).
- High contrast, screen readers and display scaling on real hardware.

## Blocked

- **Real publish:** changing real DNS requires a person to press Publish.
- **Code signing:** needs a certificate, which is a paid service. Windows SmartScreen warns about the unsigned installer.

## Known limitations

- Hosting returns after a restart only once you sign in to Windows, because the engine is a logon task. Windows service mode is not offered, as a service can't read your Cloudflare token.
- The very first verification on a new hostname can pause if a resolver cached the earlier missing record. It keeps checking and deletes nothing.
- File updates replace the site with two renames, so a request can briefly see no site, and a power cut between them isn't recovered automatically.
- Clippy still reports style warnings, mostly nested `if` statements.

See [AUDIT.md](AUDIT.md) for the area-by-area audit.

## To finish verification

1. Open Hostify, go to **Websites**, choose a website and select **Publish this website**.
2. Enter your hostname, review the DNS record, tick consent and publish.
3. Wait for **Published**, then open the HTTPS address from another network, such as a phone on mobile data.
4. Restart Windows, sign in without opening Hostify, and check the address again.

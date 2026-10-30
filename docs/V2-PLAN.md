# Hostify V2, Developer Edition: architecture and progress

Baseline: tag `v0.2.0`, branch `v2/developer-edition`. V1 static hosting and publishing must keep working at every phase.

## Decisions

| Topic | Decision | Reason |
|---|---|---|
| Caddy | Keep one Caddy per site on its own loopback port, admin off | It is proven on a real domain and isolates sites. Enabling the admin API for hot reload adds a management surface |
| Config updates | Write to a temp file, `caddy validate`, atomic rename, restart that site's Caddy; keep the old file if validation fails | Restart causes a brief gap for that one site, so this is **not** zero-downtime. Document it |
| Tunnel | Keep one tunnel per site pointing at the site's Caddy port | No Cloudflare logic changes. Static deployments keep working untouched |
| Node | Caddy `reverse_proxy` to a separate internal loopback port. The app gets `PORT` and `HOST=127.0.0.1` | Caddy keeps the marker route, so public verification works the same for every kind. WebSockets pass through `reverse_proxy` |
| PHP | A small pool of `php-cgi.exe -b 127.0.0.1:<port>` processes behind Caddy `php_fastcgi` | php-cgi on Windows handles one request per process, and PHP-FPM does not exist on Windows |
| MySQL | One `mysqld` from the official no-install ZIP, run under the engine as the signed-in user, `bind-address=127.0.0.1`, X plugin off | A Windows service needs administrator rights and runs as another identity, which cannot read the user's Credential Manager. This differs from the brief's "Windows service lifecycle"; report it |
| Admin DB secret | Generated root password in Credential Manager. Each website gets its own user with rights on its own database only | Least privilege |
| App secrets | Windows DPAPI (`CryptProtectData`, user scope) stored in SQLite as ciphertext | Credential Manager limits blob size. No plaintext at rest |
| IPC and secrets | Secret values never appear in list responses. One explicit `reveal_env_var` command returns one value | Keeps secrets out of UI state and logs |
| Runtime versions | Compiled-in manifest pinned by SHA-256, taken from publisher data. Updating means a Hostify release adds a pin | Same trust model as Caddy and cloudflared. A compromised index cannot substitute a build |
| Trust | The engine refuses to run imported code without `trustAcknowledged`. npm uses `--ignore-scripts` unless `allowInstallScripts` is set | Enforced in the engine, not only in the UI. This is **not** a sandbox and must never be described as one |
| Child processes | `env_clear`, then a minimal allowlist plus the site's variables. No shell from Hostify | Prevents leaking tokens and the user's environment |
| Privilege | Everything runs as the signed-in user. Nothing requires elevation | |

## Pinned runtimes

| Runtime | Version | URL | SHA-256 | Evidence |
|---|---|---|---|---|
| Node.js | 24.22.2 (LTS Krypton) | `https://nodejs.org/dist/v24.22.2/node-v24.22.2-win-x64.zip` | `ca1bff5d8ead13c9f2ea404a176db46bd430704d5b5113734049fa365e7799e2` | `SHASUMS256.txt` |
| Node.js | 26.13.0 (LTS Lithium) | `https://nodejs.org/dist/v26.13.0/node-v26.13.0-win-x64.zip` | `dcc60d59a0746999f99064a01c020445d54d84228c691cdbf25b49ac3b59150a` | `SHASUMS256.txt` |
| PHP NTS x64 | 8.4.27-1 | `https://windows.php.net/downloads/releases/php-8.4.27-1-nts-Win32-vs17-x64.zip` | `dbae154a0a89791f36c3d48e68bae797878333e7d6cb5721e74617de59b63ab7` | `releases.json` |
| PHP NTS x64 | 8.5.12-1 | `https://windows.php.net/downloads/releases/php-8.5.12-1-nts-Win32-vs17-x64.zip` | `2a36a2edcb7a5b9252f638fd5e4cf84d7f6e066092654220dc0b63e5cf4e9723` | `releases.json` |
| MySQL | 8.4.10 | `https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-winx64.zip` | `3b950db31c33fb59252568c012bd9ee5fac50811e778ca7c8f1a0dc91686cd6f` | Good GPG signature, key `BCA4 3417 C3B4 85DD 128E C6D4 B7B3 B788 A8D3 785C` |

PHP moves superseded builds to `/downloads/releases/archives/`, so try that URL next. MySQL does the same under `https://cdn.mysql.com/archives/mysql-8.4/`.

## Threat model

Imported Node and PHP projects are trusted-user code. They run as the signed-in user and can read that user's files. A Windows Job Object gives process-tree cleanup, not isolation. Hostify must not be described as secure multi-tenant hosting. See `docs/SECURITY.md`.

## Phases

| # | Phase | Owner | Status |
|---|---|---|---|
| 1 | Audit, V1 baseline, plan | Claude | Done: tag `v0.2.0` |
| 2 | Runtime manager | Codex task 11 | |
| 6 | Environment variables and DPAPI vault | Codex task 11 | |
| 3 | Node.js hosting | Codex task 12 | |
| 7 | Routing by website kind | Codex tasks 12, 13 | |
| 4 | PHP hosting | Codex task 13 | |
| 5 | MySQL | Codex task 14 | |
| 8 | UI | Claude | |
| 9 | Reliability and security audit | Claude review | |
| 10 | Sample apps, real end-to-end tests, installer, QA report | Claude | |

Each phase ends with real tests run and a Claude review before the next starts. Phases 2 and 6 are done first because every later phase depends on them.

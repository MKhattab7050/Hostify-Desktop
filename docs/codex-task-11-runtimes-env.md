# Codex Task 11 - runtime manager and environment variables

Repo D:\Hostify-Desktop, branch `v2/developer-edition`. Read `docs/V2-PLAN.md`, `docs/IPC.md`, `docs/ENGINE.md` first. `%USERPROFILE%\.cargo\bin` on PATH, always `cargo ... --offline`. Your sandbox has no network and no new crates. You may add features to the existing `windows-sys` dependency and add it to `hostify-core` under `[target.'cfg(windows)'.dependencies]`.

## FILE OWNERSHIP
Edit only `crates/**`, `src-tauri/src/lib.rs` (forward macro and handler list only), and append sections to `docs/IPC.md` and `docs/ENGINE.md`. Never delete existing documentation text. Do not touch `src/**`, `tests/ui/**`, `crates/hostify-core/src/cloudflare.rs` or `verify.rs`. Existing IPC shapes stay compatible. V1 static hosting must behave exactly as before.

No cosmetic refactors or reformatting of existing code. Do not start or stop any real Hostify engine or touch `%LOCALAPPDATA%\Hostify`: a real installation is running on this PC. Tests use temp directories.

## A. Database migration v4 (`crates/hostify-core/src/db.rs`)
Additive only, in one transaction, following the existing pattern and raising the version guard. Existing rows keep working.

```sql
ALTER TABLE websites ADD COLUMN kind TEXT NOT NULL DEFAULT 'static';
ALTER TABLE websites ADD COLUMN runtime_version TEXT;
ALTER TABLE websites ADD COLUMN app_config_json TEXT NOT NULL DEFAULT '{}';
CREATE TABLE env_vars (
  site_id TEXT NOT NULL REFERENCES websites(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  secret INTEGER NOT NULL DEFAULT 0,
  plain_value TEXT,
  sealed_value BLOB,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (site_id, name)
);
```
Exactly one of `plain_value` and `sealed_value` is set. Test: a v3 database with sites and deployments migrates with data intact and `kind='static'`. Expose `kind`, `runtimeVersion` on the `Site` JSON. `foreign_keys` may be enabled only after migrations, as now, so `remove_site` must delete `env_vars` rows explicitly as well.

## B. Runtime manager (new `crates/hostify-core/src/runtimes.rs`)
Do not change `components.rs` behaviour for Caddy and cloudflared.

Manifest: a `const` table holding exactly the five pinned entries in `docs/V2-PLAN.md`, with `kind` (`node`, `php`, `mysql`), `version`, ordered candidate URLs (the primary plus the archive URL where the plan names one; the manifest URL host must be one of `nodejs.org`, `windows.php.net`, `cdn.mysql.com`), `sha256`, the expected executable relative path (`node.exe`, `php-cgi.exe`, `bin/mysqld.exe`), and an approximate installed size in bytes for the free-space check.

Install to `<data>/runtimes/<kind>/<version>/`:
1. Refuse an unknown kind or version. The version comes from IPC, so it is matched against the table and never joined into a path before that.
2. Check free disk space on the data volume against download size plus installed size with a margin. Fail early with a plain sentence that states how much is needed and how much is free. Use `sysinfo::Disks` or Win32, whichever is available to `hostify-core`.
3. Stream the download to `<data>/runtimes/.partial/<kind>-<version>.zip` over HTTPS with a redirect policy that allows HTTPS only. Do not buffer the file in memory (MySQL is 280 MB). Bound the size by a per-entry limit. Hash while streaming. Report progress through a callback with bytes done and total.
4. Compare SHA-256 before reading the archive. On mismatch delete the file and report that the download failed its integrity check. Never fall through to the next URL after a hash mismatch; only try the next URL on a 404 or transport error.
5. Extract to `<data>/runtimes/.partial/<kind>-<version>/` by streaming, using the same entry-name safety rules as `import.rs` (share the validation function rather than copying it) with limits suited to runtimes. Strip the single top-level directory.
6. Check that the expected executable exists, run a version probe through the existing `ProbeRunner` style (`node --version`, `php-cgi -v`, `mysqld --version`, no window) and require that the output contains the pinned version number (`8.4.27` for `8.4.27-1`). A probe failing with exit code `0xC0000135`/`-1073741515` or a missing `VCRUNTIME140` means the Microsoft Visual C++ runtime is absent: report exactly that, with Microsoft's redistributable as the fix. Do not download it.
7. Atomically rename into place. A leftover final directory for the same version is replaced only after the new one validated.
8. At engine startup remove anything under `.partial`, which makes an interrupted install recoverable.

Also provide: `list()` returning, for every manifest entry, `{kind, version, installed, ready, error?, sizeBytes, path?, usedBy: [siteId]}`; installed sizes computed off the async executor and cached for 60 s; `remove(kind, version)` which refuses when a website uses that version or, for MySQL, when a data directory exists, naming why; `resolve(kind, version) -> PathBuf` for later tasks, failing with a plain sentence when not installed. Serialise installs per runtime so a double click cannot run two.

Real size note: do not fabricate figures. `sizeBytes` is measured or absent.

IPC: `list_runtimes`, `install_runtime {kind, version}`, `remove_runtime {kind, version}`. Event `runtime-progress {kind, version, phase: "download"|"verify"|"extract"|"check", done, total}`. Long installs must not block other requests, and must not hold `operations` or `publishing`.

Tests with `wiremock` or a local server: good install from a small fixture ZIP through an injectable manifest and HTTP client; wrong hash leaves nothing behind and does not try another URL; 404 falls through to the archive URL; a truncated download fails; zip traversal rejected; an interrupted `.partial` is cleaned; `remove` refuses when in use; unknown version refused; low disk refusal through an injectable free-space function.

## C. Environment variables
New `crates/hostify-core/src/vault.rs`: `seal(&[u8]) -> Vec<u8>` and `open(&[u8]) -> Vec<u8>` using DPAPI `CryptProtectData` / `CryptUnprotectData` at user scope with `CRYPTPROTECT_UI_FORBIDDEN` and a fixed Hostify entropy value. Free returned buffers with `LocalFree`. Zero the plaintext copies you own where practical. Provide a non-Windows stub returning an error, as the shipping target is Windows only. Test a real round trip and that tampered ciphertext fails.

New `crates/hostify-core/src/env_vars.rs`:
- Name rule: `^[A-Za-z_][A-Za-z0-9_]{0,127}$`, refusing names starting `HOSTIFY_` and names Hostify controls (`PORT`, `HOST`). Value limit 64 KiB, and no NUL.
- `list(site)` returns `{name, secret, value?}` where `value` is only present for non-secret variables.
- `set(site, name, value, secret)`, `delete(site, name)`, `reveal(site, name)`.
- `import_dotenv(site, text, mark_secret_names)`: a real parser for `KEY=value`, `export KEY=value`, comments, single and double quotes, and `\n` in double quotes. Returns counts and per-line errors that give the line number and never the value. By default treat a name as secret when it contains `SECRET`, `TOKEN`, `PASSWORD`, `PASS`, `KEY`, `PRIVATE` or `CREDENTIAL` (case-insensitive).
- `export_nonsecret(site)`: secrets are omitted, and the text lists their names in a comment.
- `resolved(site) -> Vec<(String, Zeroizing-style String)>` for injection into processes, used only by the engine.
- `secret_values(site)` for log redaction.

IPC: `list_env_vars {siteId}`, `set_env_var {siteId,name,value,secret}`, `delete_env_var {siteId,name}`, `reveal_env_var {siteId,name}`, `import_env {siteId,text}`, `export_env {siteId}`. A change to a running non-static site returns `restartRequired: true`. A value that fails to decrypt (another user, a restored DB) returns a plain error naming the variable and does not crash.

Secrecy tests: after setting a secret, the raw SQLite file does not contain the value; `list` does not return it; activity rows and `pipe` errors do not contain it; a malformed `import_env` line does not echo its value. The engine must never log request params for `set_env_var`, `reveal_env_var` or `import_env`. Check `pipe.rs` and `src-tauri` for any request logging.

## D. Supervisor groundwork
Extend `Spec` in `supervisor.rs` with `cwd: Option<PathBuf>`, `env: Vec<(String,String)>` and `redact: Vec<String>`. For a Spec with a non-empty `env` or `cwd` (later tasks), the child starts from `env_clear()` plus an allowlist (`SystemRoot`, `SystemDrive`, `WINDIR`, `ComSpec`, `PATHEXT`, `TEMP`, `TMP`, `USERPROFILE`, `APPDATA`, `LOCALAPPDATA`, `PROGRAMDATA`, `NUMBER_OF_PROCESSORS`, `PROCESSOR_ARCHITECTURE`, `OS`), then the Spec's `env`. `pump` redacts all `redact` values alongside the token. Add crash-loop detection: a process that exits within 30 s of starting five times in a row stops being restarted, the site status becomes `error` with a plain message naming the last exit code, and activity records it. A manual start resets it. Caddy and cloudflared keep their present behaviour apart from gaining this limit only if it cannot strand a published site: exempt the connector from the limit, as it is expected to retry forever while offline.

Report exit codes in the status detail and the activity row. Test crash-loop detection with a real short-lived child process.

## Definition of done
Run `cargo fmt --all`, `cargo build --workspace --offline`, `cargo test --workspace --offline`, and `cargo clippy --workspace --all-targets --offline`, and paste the real summary lines. No new clippy errors. State plainly what you could not exercise, such as real downloads. Do not claim anything you did not run.

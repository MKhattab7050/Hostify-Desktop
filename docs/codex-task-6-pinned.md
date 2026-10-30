# Codex Task 6 — pinned component manifest (no dependence on release notes / GitHub API)

Repo D:\Hostify-Desktop. `%USERPROFILE%\.cargo\bin` on PATH, `cargo ... --offline`, no network, no new crates. Read `crates/hostify-core/src/components.rs` (+ its tests), `crates/hostify-engine/src/state.rs`, `docs/ENGINE.md`, `docs/INSTALLER.md`.

## FILE OWNERSHIP
Edit ONLY `crates/**`, `docs/ENGINE.md`, `docs/INSTALLER.md`. Do not touch `src/**` or `src-tauri/**`.

## Root cause (verified against the real internet by the orchestrator)
`components.rs` finds the cloudflared checksum by parsing the GitHub release *notes*. cloudflared 2026.10.7 shipped notes with NO checksum lines, so `install("cloudflared")` fails closed with "Missing or invalid checksum" for every user, and it will break again on any release that changes format. Also the `latest` lookup uses the unauthenticated GitHub API (rate-limited, 60/h).

## Change
1. Replace the "latest + parse notes/checksums.txt" flow with a **pinned manifest compiled into the binary** (one `const` table, easy to bump): per component: name, version, asset file name, download URL `https://github.com/<repo>/releases/download/<tag>/<asset>`, expected SHA-256 (lowercase hex), and for Caddy also the published SHA-512. Pins (each hash was computed independently AND matches the publisher's published value):
   - caddy `v2.11.7`, asset `caddy_2.11.7_windows_amd64.zip`, repo `caddyserver/caddy`,
     sha256 `0a1edc0b799512051c57071ce0e798d3f2cf67dc98171366f1d2b326072e3b06`,
     sha512 `c308154504e53755958ffc62d8a2657ab17eefd6dd25c45dbb2c3d93d29519971f77ca652f7d0f2bcb7f980b9ba3822478865e29969d0272d7613550a43152b1`
   - cloudflared `2026.10.6`, asset `cloudflared-windows-amd64.exe`, repo `cloudflare/cloudflared`,
     sha256 `d521b3bf68982165f0d06d76732f22d9a105ef057ff3ae4c0986f3a23190950f`
   No GitHub API call, no "latest", no parsing of release notes. Verify ALL listed hashes (both for Caddy) before anything is written; fail closed on any mismatch. Keep: size cap, temp-file + atomic rename, a failed/mismatched download never touches an existing working binary and leaves no temp file, only the single `caddy.exe` zip entry is extracted (never an archive-supplied path), friendly offline / HTTP error messages, progress events, `names` filter, repair of missing/zero-byte/non-running binaries. After install, run the version probe and report the installed version; the version reported by the binary should contain the pinned version string (warn in the error if it does not).
2. **Status cache**: key the ~2 s cache by file metadata (exists, length, modified time) for each exe so a file replaced, emptied or deleted outside Hostify is reflected on the very next `engine_status` call; still invalidate right after installs.
3. **Tests (offline, mock `Http`)**: install succeeds with correct pinned hashes (build a tiny fake zip/exe in the test and a test-only manifest override); wrong sha256 rejected; Caddy with correct sha256 but wrong sha512 rejected; no network call other than the pinned URL (assert the mock saw exactly that URL, and never a `/repos/.../releases` API URL); failed checksum leaves existing binary untouched; status reflects a file emptied between two immediate `engine_status` calls (no sleep); all previous component regression tests still pass (update them for the new manifest API, do not delete coverage).
4. Update `docs/ENGINE.md` and `docs/INSTALLER.md` (replace the "latest release / notes checksum" wording with: pinned versions + hashes in `components.rs`, bump procedure: download, verify against the publisher's list, update the table).

## Definition of done
`cargo test --workspace --offline` and `cargo build --workspace --offline` pass; paste real output. No partial work; be honest about what is not verified (the orchestrator will run the real-download end-to-end).

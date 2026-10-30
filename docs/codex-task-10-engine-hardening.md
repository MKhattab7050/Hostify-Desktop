# Codex Task 10 - engine reliability and security audit

Repo D:\Hostify-Desktop. `%USERPROFILE%\.cargo\bin` on PATH, always `cargo ... --offline`. No network, no new crates.

## FILE OWNERSHIP
Edit ONLY `crates/**`, `src-tauri/src/**`, `src-tauri/capabilities/**`, `src-tauri/installer-hooks.nsh`, `src-tauri/resources/*.ps1`, and create `docs/AUDIT.md`. Do NOT touch `src/**`, `tests/ui/**`, `docs/IPC.md`, `docs/ENGINE.md`. IPC JSON shapes stay as in `docs/IPC.md`.

Fix real defects with small changes. No cosmetic refactors or reformatting of existing code. Every fix gets a regression test where one is possible offline.

## A. Confirmed defects: fix all of these

1. **Startup blocks and can kill the engine.** In `crates/hostify-engine/src/cli.rs::run`, `engine.startup().await?` runs before `pipe::serve`. Offline at boot, each `start_tunnel` waits 60 s, so the UI reports "can't reach its hosting engine", and any startup error exits the engine and takes all hosting down. Serve the pipe immediately. Run startup work concurrently. A startup failure for one site, tunnel or verifier is logged to activity and must not stop the engine or the others (`start_verifier(...)?` currently fails when the Cloudflare token is absent, e.g. after `cf_disconnect` with a waiting deployment; that row must stay as it is and log a clear message). Local sites start first and must not wait on tunnels.
2. **Connector given up on at boot.** `Supervisor::start_tunnel` stops the worker if it is not connected within 60 s. During publish that is correct behaviour. During startup recovery of a retained or published deployment it leaves the site offline until the next restart. Add a startup path that launches the supervised connector and returns without stopping it, so it keeps retrying with the existing backoff. Publish behaviour stays as it is.
3. **Jargon and debug text reaching users.** In `deploy.rs` `save()` writes `Publishing: waiting_dns` to activity: use plain sentences, one per state, e.g. "Waiting for DNS for www.example.com" and "Published www.example.com". Remove the literal word `retry_verification` from every user-facing string in `deploy.rs` and `state.rs`: say "Check again" (update the assertion in `state.rs` tests that greps for it). The `publish_site` existing-deployment error must be a plain sentence naming the hostname and state in words, not an id. In `verify.rs::attempt`, `{result:?}` and `{other:?}` put Rust debug text in `VerifyCheck.detail`: format these as e.g. `luke.ns.cloudflare.com: 2 addresses`, `This PC: no answer yet`, `Connector: not connected`.
4. **`system_stats` is expensive.** `state.rs` builds `sysinfo::System::new_all()` and walks the entire data directory on every call, every 3 s. Keep one `System` in the engine refreshing only CPU and memory, and cache `directory_bytes` and disk figures for 60 s. Note the first CPU reading after creation is meaningless in sysinfo: return a real one.
5. **Unbounded growth.** `activity_logs` and old `deployments` rows are never pruned. Prune activity to the newest 5000 rows, and for each site keep the 20 newest deployments plus any row holding resources or `rollback_pending`. Never prune a row whose resources are still needed. Prune at startup and after a deployment ends.
6. **Log secrecy.** `pump` in `supervisor.rs` redacts only the exact tunnel token string. Confirm nothing else can write a token: Cloudflare API errors, engine activity, `tunnel-*.log`. Test with a token inside a realistic cloudflared error line.

## B. Audit: read the code, report in `docs/AUDIT.md`, fix what is a real defect

For each area write: what you checked, what you found, what you fixed (with test name), what remains. Do not write "looks fine" without saying what you looked at.

- ZIP import: traversal, absolute paths, Windows device names, ADS (`file.txt:stream`), reserved names, symlinks, zip bombs (size and ratio limits), duplicate names differing by case, long paths.
- Directory import: symlinks and junctions must not escape the source; partial copy failure leaves the current site intact.
- Atomic swap in `update_site_files`, with Caddy running and holding files open on Windows; release retention keeps 3.
- `remove_site` path safety: the id is always our UUID, never reaches outside `sites/`.
- Process execution: no shell, no user input in arguments, absolute executable paths, no search-path lookup.
- Caddyfile generation: can a website name, path or port inject directives? Admin endpoint stays off.
- Named pipe: DACL, instance limits, message size limits (a client must not exhaust memory with an endless line), malformed JSON, slow clients and event backpressure.
- Tauri: capabilities least privilege, `tauri.conf.json` CSP, `openUrl` scheme limits.
- Process ownership: no orphan Caddy or cloudflared after engine crash, restart or kill; port collision with a foreign process; a Caddy that did not exit on restart.
- Component upgrade: replacing a binary while a site uses it must not interrupt the running site or leave a half-written exe. Checksums verified before the swap.
- Windows service mode: data dir, SID-scoped pipe name, whether a service-mode engine is reachable by the desktop user at all. State honestly what is untested.
- Uninstall: stops the engine, removes the task, keeps user data, and leaves nothing running.
- Races and invalid transitions: stop_site, restart_site or update_site_files during publish or verification; two clients; `unpublish_site` during provisioning. Any mutex held across an `.await`; lock order between `db` and `statuses`.
- Network calls with no timeout.
- `unwrap`/`expect` reachable from external input, including `DeploymentState::from` on an unknown string from an old or future database row: it must not panic the engine.

## Definition of done
Run `cargo build --workspace --offline` and `cargo test --workspace --offline`, paste the real summary lines. State plainly what you could not exercise (reboot, real Windows service, live network). Do not claim anything you did not run.

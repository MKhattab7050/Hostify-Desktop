# Codex Task 13 - PHP hosting

Repo D:\Hostify-Desktop, branch `v2/developer-edition`. Read `docs/V2-PLAN.md`, `docs/IPC.md`, `docs/ENGINE.md` and `docs/SECURITY.md` first, then the Node code this builds on: `crates/hostify-engine/src/supervisor.rs`, `build.rs`, `crates/hostify-core/src/app_config.rs`, `project.rs`, `caddy.rs`. `%USERPROFILE%\.cargo\bin` on PATH, always `cargo ... --offline`. No network, no new crates. This PC is short of memory: `CARGO_BUILD_JOBS=4` is set, so keep it, use targeted `cargo check` and `cargo test -p` while iterating, and run the full workspace commands once at the end.

## FILE OWNERSHIP
Edit only `crates/**`, `src-tauri/src/lib.rs` and `src-tauri/src/engine.rs` (forwarding only), and append sections to `docs/IPC.md` and `docs/ENGINE.md`. Do not touch `src/**`, `tests/ui/**`, `samples/**`, `docs/SECURITY.md`, `cloudflare.rs` or `verify.rs`. Static and Node behaviour must not change, and static Caddyfiles stay byte-identical. Reuse the Node paths (releases, trust, Job Objects, status aggregation, rollback) by generalising them, not by copying them.

Do not start or stop any real Hostify engine or touch `%LOCALAPPDATA%\Hostify` except reading `bin\caddy.exe`. A real installation is running on this PC. Do not commit.

## Facts you must handle
- Linux PHP-FPM does not exist on Windows. `php-cgi.exe -b 127.0.0.1:<port>` serves one request at a time and `PHP_FCGI_CHILDREN` is not supported, so concurrency requires several processes on separate ports.
- `php-cgi` exits on its own after `PHP_FCGI_MAX_REQUESTS` requests with exit code 0. Under load that happens many times within 30 s, and it must not count toward crash-loop detection. Restart it immediately. Set `PHP_FCGI_MAX_REQUESTS=10000`.
- The official ZIP ships no `php.ini` and extensions are off by default. `openssl`, `curl` and others need DLLs from the PHP directory, so that directory must be first on `PATH`.
- `AppConfig::validate` requires `package.json` scripts because of the `Start` default, which must not apply to PHP.
- NTFS and Windows path rules can expose source code: `/index.php::$DATA`, `/index.php.`, `/index.php%20` and `/INDEX.PHP`. A generic static route must never return PHP source. See the required test in F.

## A. Configuration
Extend `AppConfig` with `#[serde(default)]` fields, keeping Node JSON compatible:
- `documentRoot`, relative, default `.`, same validation as `workingDir`. Inspection suggests `public` when `public/index.php` exists and there is no root `index.php`.
- `phpWorkers`, 1 to 16, default 4.
- `phpExtensions`: names from an allowlist that must also exist in the runtime's `ext` folder, default `mbstring`, `openssl`, `curl`, `fileinfo`, `mysqli`, `pdo_mysql`. Allowlist also includes `gd`, `intl`, `exif`, `zip`, `soap`, `sodium`, `pdo_sqlite`, `sqlite3`.
- `phpSettings`: only `memory_limit`, `upload_max_filesize`, `post_max_size`, `max_execution_time`, each validated (`^\d+[KMG]?$`, seconds for the last). Values are never written to the ini file unvalidated, so a newline cannot inject a directive.
- `persistentPaths`: relative paths inside the release copied from the active release into a new one before it starts, so uploads survive updates. Same path validation, and a copy failure fails the update while the old release keeps running. Inspection suggests `wp-content` and `wp-config.php` for a WordPress project. Applies to Node too.

`set_app_config` accepts `phpVersion` for PHP sites, checked with `runtimes::is_pinned`. `project::inspect` recommends the newest pinned PHP satisfying a simple `composer.json` `require.php`, otherwise the newest, and reports `framework: "wordpress"` when `wp-config-sample.php` exists plus `usesDatabase` from `wp-config-sample.php` or `mysqli`/`pdo_mysql` use in a root `composer.json`. `composer install` is not run: report a warning when `composer.json` has dependencies and there is no `vendor` folder.

Remove the refusal in `add_site`. `update_site_files` and promotion follow the Node path.

## B. Trust and activation
`build_app` works for PHP without install or build steps. It requires `trustAcknowledged: true` like Node, marks the release built and trusted, and emits the same `app-build` events. Imported PHP runs as the signed-in user: nothing here is a sandbox and no message may say so.

## C. Per-site php.ini
Write `<data>/run/php/<id>/php.ini`, generated from validated data only, atomically (temp plus rename), and pass it with `-c <path>`. Set `PHP_INI_SCAN_DIR` to empty. Paths in the file use forward slashes and refuse `"` and control characters.

Minimum content: `extension_dir` set to the runtime's `ext`, one `extension=` per enabled extension, `cgi.fix_pathinfo=0`, `cgi.force_redirect=0`, `fastcgi.logging=0`, `expose_php=Off`, `display_errors=Off`, `display_startup_errors=Off`, `log_errors=On`, `error_log=<data>/logs/<id>.php-error.log`, `allow_url_include=Off`, `allow_url_fopen=Off`, `session.cookie_httponly=1`, `session.use_strict_mode=1`, `session.save_path=<data>/run/php/<id>/sessions`, `upload_tmp_dir=<data>/run/php/<id>/tmp`, `sys_temp_dir` the same, `user_ini.filename=` (empty), `date.timezone=UTC`, `variables_order=GPCS`, and the validated `phpSettings`.

Do not use `open_basedir` or `disable_functions`, and do not describe the file as a security boundary. Create those directories before starting.

## D. Process pool
For a site with N workers, allocate N separate internal ports (stored in `phpPorts`, never colliding with `reserved_ports`, which must now include them) and run N workers keyed `php:<id>:<n>`, each with its own per-app Job Object, `role: App`, logging to `<data>/logs/<id>.app.log`, working directory the document root.

Environment: the allowlist, `PATH` with the PHP directory first then `%SystemRoot%\System32`, `PHP_FCGI_MAX_REQUESTS=10000`, `PHP_INI_SCAN_DIR=`, the site's variables redacted through `secret_values`. Do not set `PORT` or `HOST`.

Add `Health::TcpConnect` for workers. A clean exit (code 0) restarts immediately without counting as a crash, whatever `restart` says unless `never`, and logs nothing to the activity feed. A non-zero exit follows the existing crash-loop rule, and a single worker's crash loop makes the site `error`. Report a missing Visual C++ runtime using the existing runtime wording when `php-cgi` exits with `0xC0000135`.

`app_status` aggregates over all workers: `processCount`, `memoryBytes`, `cpuPercent`, `restarts`, `lastExitCode`, a `workers` array of `{port, pid, health}`, and `activeRelease`. Measure on request only. Health over HTTP for a PHP site is only through Caddy, at `healthPath`, at the same reduced frequency as a Node app.

Start order: workers, wait for TCP on each, then validated Caddy, then a real request through Caddy with a foreign `Host` that must answer below 500. Promotion, rollback and stop use the Node logic. `stop_site` stops Caddy first and then the workers. Each worker takes the graceful path already in place, shortened to 3 s, because `php-cgi` has no useful Ctrl+C handling.

## E. Caddy
Add `Backend::Php { root, upstreams: Vec<u16> }` keeping the marker route, the Caddy-only health route, logging, and the `X-Hostify-Verify` header. Inside `handle`, before PHP handling:
- `@hidden path_regexp (^|/)\.` responds 404.
- A second matcher responds 404 for any path containing `::` or `:$`, ending in `.` or a space (after Caddy's decoding), or naming `composer.json`, `composer.lock`, `*.ini`, `*.log` at any depth, and `vendor/` at the root.
- `root * <root>`, `php_fastcgi 127.0.0.1:<a> 127.0.0.1:<b> ... { lb_policy least_conn, lb_try_duration 30s, unhealthy_request_count 1 }` so a busy single-request worker is skipped rather than queued behind, and `file_server` with no browse.

If real Caddy rejects any subdirective, use what it accepts and prove the behaviour with the concurrency test below. Do not weaken the source-disclosure matcher to make something pass.

## F. Tests
Unit tests: ini generation (a newline in a setting is refused, extension outside the allowlist refused, Windows path quoting), config validation, inspection (plain PHP, `public/`, WordPress, composer without vendor, composer `require.php`), Caddy output with `caddy validate` against the real binary, clean exit not counting toward crash detection.

Real tests are `#[ignore]`d in a new `crates/hostify-engine/tests/php_live.rs`, using `HOSTIFY_LIVE_RUNTIMES` (PHP 8.4.27-1 and 8.5.12-1 exist under `D:\Hostify-Desktop\target\live-runtimes`) and the real Caddy path, modelled on `tests/node_policies_live.rs`:
1. `samples/php-basic` copied from the repo: `/health` through Caddy with a foreign `Host` returns `"sample":"php"` and `PHP_SAPI` `cgi-fcgi`; a set variable reports as set.
2. Both PHP versions at the same time, on two sites, each reporting its own version.
3. Source disclosure: with a `secret.php` holding a unique string in code never echoed, GET `/secret.php`, `/SECRET.PHP`, `/secret.php::$DATA`, `/secret.php.`, `/secret.php%20`, `/secret.php%00`, `/secret.php/`, `/.env`, `/composer.json`, `/php.ini` never return that string or the file contents.
4. Concurrency: with 4 workers, 4 requests to a script sleeping 2 s finish in under 4 s together. With one worker they take about 8 s. Print the real times.
5. Worker recycling: with `PHP_FCGI_MAX_REQUESTS` overridden low for the test, sustained requests never see a 5xx for long, the site stays running, and crash-loop detection does not fire.
6. Crash handling: kill one worker's process; it comes back, the site recovers, and the others kept serving.
7. An extension is really loaded: `extension_loaded('mbstring')`, `pdo_mysql` and `openssl` are true.
8. Persistent paths: a file written to a configured path survives `update_site_files` plus promotion. An update whose health check fails rolls back to the previous release.
9. A PHP site, a Node site and a static site running at once with independent restarts.
10. Refusal without trust, and `documentRoot` traversal refused, including through a junction.
11. `.user.ini` in the project does not change settings.
12. Stop leaves no `php-cgi` from that site and frees its ports.

## Definition of done
Run `cargo fmt --all`, `cargo build --workspace --offline`, `cargo test --workspace --offline --no-fail-fast`, `cargo clippy --workspace --all-targets --offline`, and the ignored PHP tests with `HOSTIFY_LIVE_RUNTIMES` set, and paste the real summary lines. DPAPI fails with error 2 in your sandbox, so name any test affected and it will be rerun outside. No new clippy errors. State plainly what failed or could not be exercised. Do not claim anything you did not run.

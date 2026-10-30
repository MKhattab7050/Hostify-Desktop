# Codex Task 14 - MySQL management

Repo D:\Hostify-Desktop, branch `v2/developer-edition`. Read `docs/V2-PLAN.md`, `docs/IPC.md`, `docs/ENGINE.md`, `docs/SECURITY.md`, then `crates/hostify-engine/src/supervisor.rs`, `php.rs`, `crates/hostify-core/src/runtimes.rs`, `env_vars.rs`, `vault.rs`, `secrets.rs`, `db.rs`. `%USERPROFILE%\.cargo\bin` on PATH, always `cargo ... --offline`. No network, no new crates. This PC is short of memory: `CARGO_BUILD_JOBS=4` is set, so keep it, use targeted `cargo check` and `cargo test -p` while iterating, and run the full workspace commands once at the end.

## FILE OWNERSHIP
Edit only `crates/**`, `src-tauri/src/lib.rs` and `src-tauri/src/engine.rs` (forwarding only), and append sections to `docs/IPC.md` and `docs/ENGINE.md`. Do not touch `src/**`, `tests/ui/**`, `samples/**`, `docs/SECURITY.md`, `cloudflare.rs` or `verify.rs`. Static, Node and PHP behaviour must not change.

Do not start or stop any real Hostify engine or touch `%LOCALAPPDATA%\Hostify` except reading `bin\caddy.exe`. A real installation is running on this PC. Do not commit.

## Decisions already made
- **Client:** Hostify talks to MySQL through the official `mysql.exe`, `mysqldump.exe` and `mysqladmin.exe` in the pinned, hash-checked ZIP. There is no Rust driver. No text from a user reaches SQL unless it has passed a strict validator, and SQL goes through stdin, never through arguments.
- **Process:** one `mysqld` run by the engine as the signed-in user. It is not a Windows service. Do not add one.
- **Credentials on disk:** passwords reach the client tools only through a `--defaults-extra-file` written to `<data>/run/` with a unique name, restricted to the current user with a protected ACL, and deleted in a guard that also runs on error. Never on a command line, never in `MYSQL_PWD`, never in logs, activity, events or error text. Redact them from captured output and from any error returned.

## A. Migration v5
Additive only, in one transaction, following the existing pattern:
```sql
CREATE TABLE managed_databases (
  name TEXT PRIMARY KEY,
  site_id TEXT,
  created_at TEXT NOT NULL,
  last_backup_at TEXT,
  last_backup_path TEXT,
  last_backup_bytes INTEGER
);
CREATE TABLE managed_db_users (
  user_name TEXT PRIMARY KEY,
  database_name TEXT NOT NULL REFERENCES managed_databases(name) ON DELETE CASCADE,
  sealed_password BLOB NOT NULL,
  created_at TEXT NOT NULL
);
```
`remove_site` sets `site_id` to NULL. It never drops a database, and it removes that site's injected variables with the rest of its variables.

## B. Manager (`crates/hostify-engine/src/mysql.rs`, new)
Inject the sealing function through a small `Sealer` trait (production calls `vault::seal`/`open`) and the root password through `SecretStore`, so tests can run without DPAPI. Production has no way to select a plaintext sealer: the stub lives only in test code.

**Setup,** `mysql_setup {version}`, off the async executor, requires the runtime installed:
1. Generate a 32-character root password from a CSPRNG, with `[A-Za-z0-9]` only. Store it in Credential Manager as `mysql-root` before anything else.
2. Create `<data>/mysql/data` with an ACL granting only the current user and SYSTEM, inheritance removed, and apply the same to `<data>/mysql` and `<data>/backups/mysql`. Verify by reading the ACL back in a test.
3. Write `<data>/mysql/my.ini`, atomically, from validated data, forward slashes, values quoted: `basedir`, `datadir`, `port`, `bind-address=127.0.0.1`, `mysqlx=OFF`, `local_infile=OFF`, `skip-name-resolve=ON`, `log-error=<data>/logs/mysql-error.log`, `enable-named-pipe=OFF`, `shared-memory=OFF`, `innodb_buffer_pool_size=128M`, `performance_schema=OFF`, `max_connections=100`, `character-set-server=utf8mb4`, `secure-file-priv=NULL`. The port is the first free port from 3307, because 3306 may belong to another MySQL on this PC.
4. Run `mysqld --defaults-file=... --initialize --init-file=<restricted temp file>`, where the file holds `ALTER USER 'root'@'localhost' IDENTIFIED BY '...'`, so no moment exists with an empty or logged password. Delete the file afterward. Check `mysql-error.log` does not contain the password, in a test.
5. Write `<data>/mysql/.hostify-initialized` last. A data directory without it and with no user databases is cleared and retried. One with user databases is never cleared: report it plainly and stop.

**Lifecycle.** A supervisor worker keyed `mysql` with `Role::Service`, its own Job Object, `Health::TcpConnect`, no site. Restarts with crash-loop protection and the real exit code and last lines of the error log in the message. `mysql_start`, `mysql_stop`, `mysql_restart`.

Graceful stop runs `mysqladmin shutdown` with the root credentials and waits up to 60 s, terminating the job only after that. InnoDB recovery after a forced kill must be visible as a slow start, so the start timeout is 180 s. Engine shutdown stops sites first and MySQL last. Engine startup starts MySQL before any site, if it has been set up and either has any database or is flagged enabled. `start_site` on a site with a linked database starts MySQL first, and a MySQL failure is reported on that site by name.

**Refuse with a plain sentence** when the data directory's server version is newer than the runtime in use, based on the error log's real message.

**Name rules.** Database: `^[a-z][a-z0-9_]{0,47}$`, refusing `mysql`, `sys`, `information_schema`, `performance_schema`. User: same pattern, at most 32 characters. Identifier quoting with backticks applies only after validation. Passwords are 32 characters from the CSPRNG and `[A-Za-z0-9]`, never user-chosen, so nothing needs escaping.

## C. IPC
None of these hold `operations` or `publishing` during long work. Long operations emit `mysql-progress {operation, database, state, detail}`, forwarded in `src-tauri/src/engine.rs`.

- `mysql_status`: `{setUp, version, running, port, pid, memoryBytes, dataBytes, freeDiskBytes, lastError, healthy}`. `healthy` is the result of a real `SELECT 1` as root at request time. Sizes are measured. Absent means unknown; never invent a value.
- `list_databases`: `{name, siteId, siteName, sizeBytes, users:[name], connectionOk, lastBackupAt, lastBackupBytes}`. `sizeBytes` comes from `information_schema.TABLES`. `connectionOk` is a real connection using that database's user, and it is null when MySQL is stopped.
- `create_database {name, siteId?, injectEnv?}`: creates the database with `utf8mb4`, then a user `'<name>_app'@'127.0.0.1'` with a generated password and `SELECT, INSERT, UPDATE, DELETE, CREATE, ALTER, INDEX, DROP, REFERENCES, CREATE TEMPORARY TABLES, LOCK TABLES, EXECUTE, CREATE VIEW, SHOW VIEW, CREATE ROUTINE, ALTER ROUTINE, TRIGGER` on that database only. No global privileges and no `GRANT OPTION`. Partial failure undoes what it created. With `injectEnv`, set `DB_HOST=127.0.0.1`, `DB_PORT`, `DB_NAME`, `DB_USER`, and `DB_PASSWORD` as a secret. If any of those names already holds a different value, change nothing and report which names conflict, unless `replaceEnv: true`. Returns `restartRequired`.
- `delete_database {name, confirmName}`: `confirmName` must equal `name`. Refused if the database is not in `managed_databases`, so a database Hostify did not create is never dropped. Drops its users, then the database, then the rows. Removes injected variables only if they still equal what was injected. Backups stay on disk.
- `create_db_user {database, user}`, `delete_db_user {user}`, `rotate_db_password {user}` (updates injected variables and returns `restartRequired`), `reveal_db_password {user}` (explicit only, never logged).
- `database_connection {name}`: `{host, port, database, user}`, with no password.
- `test_connection {name}`: a real connection as the database's user, returning `{ok, detail, elapsedMs}`.
- `backup_database {name}`: `mysqldump --single-transaction --routines --triggers --events --no-tablespaces` as root, streamed to a temp file in `<data>/backups/mysql/<name>/` then renamed to `<timestamp>.sql`. Checks free space first against the database size. A non-zero exit deletes the partial file and reports the real error. Checks the dump ends with mysqldump's completion comment, so a truncated file is never recorded as a backup. Updates the table.
- `list_backups {name}`: real files, with size and time. `delete_backup {name, file}`: `file` must be a plain filename from that listing, refusing any path separator.
- `restore_database {name, file, confirmName}`: takes a safety backup first and reports its name. Stops linked sites, restores through `mysql.exe` with the file on stdin, and restarts those it stopped. A failed restore leaves the safety backup, restores from it, and reports both outcomes. A backup file can contain any SQL, so accept only files from this database's own backup folder, and refuse any path outside it, including through a junction.
- `link_database {name, siteId, injectEnv}` and `unlink_database {name}`.

`remove_runtime` for MySQL: refuse while `<data>/mysql/data` exists, naming it. Uninstalling preserves data.

## D. Tests
Unit tests with stubs: name validators (quotes, backticks, `;`, spaces, Unicode, length, reserved), `my.ini` generation, conflict handling in `injectEnv`, `delete_backup` traversal, refusal of unmanaged databases, migration v4 to v5 with data intact.

Real tests are `#[ignore]`d in `crates/hostify-engine/tests/mysql_live.rs`, using MySQL 8.4.10 from `HOSTIFY_LIVE_RUNTIMES` = `D:\Hostify-Desktop\target\live-runtimes`. That folder is 1.2 GB, so link it into the test's data directory with a junction rather than copying. Each test uses its own data directory and port.
1. Setup: running, `127.0.0.2:<port>` refused, a connection using the machine's non-loopback address refused. `SELECT user,host FROM mysql.user` has no `root` other than `localhost` and no `%` host. `mysqlx` port closed. The root password absent from `my.ini`, the error log, the engine log, activity and the database file.
2. Least privilege: two databases. The first user cannot read the second database, cannot see `mysql.user`, and cannot `CREATE USER` or `GRANT`.
3. Backup and restore round trip with real rows. A truncated backup is refused, a failed restore is recovered from the safety backup, and rows are intact.
4. Durability: stop gracefully and restart with data intact; kill the job with force and confirm it recovers with data intact.
5. Crash loop: a corrupt `my.ini` stops after the limit with the real error shown.
6. `delete_database` with a wrong `confirmName` refused and data intact. A database created outside Hostify cannot be deleted through it.
7. Real measurements printed: memory and data size after setup.
8. `samples/php-mysql` through real PHP and Caddy using injected variables: `/health` reports `ok`, and a note posted to `/notes` is read back. This proves PDO authentication against MySQL 8.4's default `caching_sha2_password`. If it fails, report the real error rather than weakening authentication.
9. `samples/node-mysql` the same way. It needs the npm registry, so it will fail in your sandbox; write it anyway, say so, and it will be run outside.

## Definition of done
Run `cargo fmt --all`, `cargo build --workspace --offline`, `cargo test --workspace --offline --no-fail-fast`, `cargo clippy --workspace --all-targets --offline`, and the ignored MySQL tests with `HOSTIFY_LIVE_RUNTIMES` set, and paste the real summary lines. DPAPI fails with error 2 in your sandbox, so name any test affected and it will be rerun outside. No new clippy errors. State plainly what failed or could not be exercised. Do not claim anything you did not run.

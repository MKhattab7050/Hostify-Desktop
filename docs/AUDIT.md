# Engine reliability and security audit

Audit date: 2026-10-30. Scope: core/engine Rust, desktop bridge, capabilities/configuration and Windows installation scripts. No network access or new crates. IPC method names and JSON shapes are unchanged. The pre-existing change to `src-tauri/tauri.conf.json` was inspected and left untouched.

## Startup, connectors and user messages

Checked `cli::run`, desired-state reconciliation, interrupted recovery, verifier registration, connector readiness and user-facing activity/evidence.

Fixed startup blocking the pipe: production serves requests concurrently with a cancellable startup task and logs startup errors without terminating hosting. Local auto-start and retained sites start before connector/remote recovery work. Individual site, connector and verifier failures are isolated. Missing account credentials preserve waiting snapshots and log an actionable message. Malformed retained journals are logged/skipped; malformed interrupted rows do not block recovery of other rows. Startup serializes with mutations/provisioning and reconciles auto-start again after rollback, which can stop a local server.

Startup uses a separate connector resume path that returns immediately and retains the supervised worker/backoff while offline. Publish still waits up to 60 seconds and stops a newly started connector on failure. Activity uses sentences with hostnames; Check-again guidance no longer exposes an IPC method name. Existing-publication errors describe hostname/state rather than deployment ID. DNS and connector details no longer expose Rust debug formatting.

Tests: `pipe_answers_while_startup_is_blocked`, `startup_connector_returns_offline_and_keeps_supervised_worker`, `startup_without_account_preserves_waiting_and_starts_other_sites`, `startup_resumes_waiting_without_rollback_and_retry_wakes_one_task`, `malformed_recovery_row_does_not_block_other_rows`, `readable_evidence_and_states`, `attempts_do_not_repeat_transition_activity_and_snapshots_surface_pending_domain`.

Remaining: boot/reboot and real offline-to-online connector recovery were not exercised. A failed initial spawn, such as a missing executable, is logged and creates no worker. Damaged journals remain for manual attention.

## Statistics and retention

Checked every statistics refresh, data-size traversal, disk enumeration, activity writer, terminal deployment save and database startup.

Fixed repeated process/system discovery: one persistent `System` refreshes only CPU usage and memory on a blocking worker. The first CPU delta waits at least sysinfo's minimum sampling interval. Disk/data-size figures are cached for 60 seconds. Activity keeps the newest 5,000 rows. Startup and deployment completion remove old failed/rolled-back deployments outside each site's newest 20 only when the journal proves resources are unnecessary. Active, resource-bearing, rollback-pending, unknown and malformed journals are conservatively retained. Ordinary engine activity insertion also trims activity.

Tests: `stats_cache_disk_and_prime_cpu`, `retention_keeps_newest_and_resource_journals`.

Remaining: resource-bearing rows can exceed 20 intentionally. Pruning does not VACUUM the SQLite file. The first statistics request can wait for CPU sampling, outside the async executor.

## ZIP import

Checked path validation, metadata scanning, root-folder flattening, extraction, entry/byte limits, ratios, actual streamed byte counts and case-normalized duplicates.

Existing defenses reject traversal, drive/UNC/absolute paths, ADS/colons, trailing dots/spaces, ordinary DOS device names, invalid/control characters, Unix links/special files, duplicate case variants and expanded-size mismatch. Limits: 50,000 files, 100,000 archive entries, 2 GiB expanded file content and 200:1 per-entry compression ratio. Reads stop at declared size plus one byte. Fixed missing device aliases (`CONIN$`, `CONOUT$`, COM/LPT superscript digits) and added a conservative 240 UTF-16-unit relative-path limit. Longer absolute staging paths can still fail through the filesystem before current is replaced.

Tests: `rejects_unsafe_names`, `rejects_zip_slip`, `rejects_compression_bomb`, `rejects_device_aliases_and_long_paths`, `case_duplicate_zip_preserves_current`.

Remaining: no exhaustive Unicode/NTFS alias fuzzing or multi-gigabyte allocation test. Aggregate expansion is checked during extraction rather than rejecting the whole metadata sum in advance; written file content remains bounded.

## Directory import

Checked recursion, symlink metadata, Windows reparse attributes, leaf-handle validation, directory guards and staging-before-swap ordering.

Files/directories reject symlinks, junctions and reparse points. Windows directory guards deny write/delete sharing while traversing; leaf handles open the reparse point itself and validate regular-file metadata. Copy failure leaves current intact. Fixed recursive self-import when the destination lies inside the source. Directory-relative paths receive the same length cap.

Tests: `rejects_destination_inside_source`, `rejects_directory_junction_and_destination_junction`, `rejects_file_symlink_swapped_after_scan`, `held_source_handle_blocks_replacement_and_growth`, `rejects_size_change_after_scan`, staging-preservation tests above.

Remaining: Windows symlink/junction tests explicitly report if creation is unavailable. This does not establish equivalent adversarial race protection on Unix; the shipping target is Windows. Empty directories do not contribute to reported file count/expanded-byte limits.

## File swap and releases

Checked update operation/provisioning guards, same-volume staging, current-to-release rename, failed-swap restoration and pruning.

The swap uses two renames, not an OS transaction. A restrictive Windows handle makes the update fail with old current intact. If stage-to-current fails, restoration is attempted and restoration failure is reported. Fixed cleanup failure after commit being reported as failed import: it now returns a warning, allowing database metadata to describe the committed update. Normal retention keeps the three newest old releases.

Tests: `locked_current_preserves_site_and_cleans_stage`, `cleanup_failure_does_not_report_committed_update_as_failed`, `keeps_only_newest_three_releases`. Extended `real_caddy_marker_and_hidden_paths` to update five times under running Caddy, verify new responses and check three retained releases.

Remaining: requests can encounter the brief gap between renames. Power loss between renames is not transactionally recovered. Unsafe/locked release entries are preserved with a cleanup warning.

## Site removal

Checked IPC IDs, database existence checks, UUID creation, joins under `sites/`, deletion order and resource journals.

Creation generates UUIDs. Removal now validates UUIDs before file deletion/database mutation, preventing a corrupt/legacy ID from traversing outside sites. Fixed removal discarding failed pending-cleanup journals: all site journals are checked and pending, malformed or resource-bearing rows block removal, rather than checking only waiting/published deployments.

Tests: `remove_rejects_non_uuid_before_deleting_row_or_files`, `remove_keeps_failed_cleanup_and_malformed_resource_journals`, `pipe_lifecycle_with_fake_runtime`.

Remaining: managed storage is trusted against same-user concurrent database/ancestor-directory replacement. Database/file deletion is not one transaction; filesystem failure can leave unreferenced files.

## Execution and Caddyfile

Checked production `Command` calls, executable roots, argument/environment handling, Caddy path quoting, listener and admin configuration.

Caddy/cloudflared and the desktop sidecar use explicit executable paths, separate arguments and no shell. Option names are fixed; user paths are arguments and tokens use environment values. Fixed `schtasks.exe` PATH lookup by resolving beneath Windows System32, and installer PowerShell lookup by using the explicit system-directory host. Installer PowerShell is an explicit script-host boundary, with quoted paths and installation-path ownership checks. Website names never enter Caddyfiles, ports are `u16`, paths are quoted and quotes/braces/newlines rejected. Caddy admin remains off and HTTP binds loopback only.

Tests: `scheduler_uses_absolute_system_executable`, `config_is_loopback_and_static`, `service_and_task_preserve_same_data_root`, `xml_escape`, `real_caddy_marker_and_hidden_paths`, existing component probes. NSIS packaging itself was not run.

Remaining: installation/data directories and the OS environment must be trusted. A port can be taken between check and bind; launch/health failure is reported. Website content is not executed.

## Named pipe

Checked DACL, owner SID, remote-client rejection, first-instance reservation, session creation, framing/JSON, writes and event queues.

The protected DACL grants SYSTEM and selected owner; remote clients are rejected. First-instance reservation prevents a second engine listener. Existing framing rejects over-1-MiB and truncated/malformed requests before unbounded growth. Fixed unlimited sessions: 32 connected sessions plus one pending OS instance. Added a 60-second frame-read deadline. Existing writes time out after 10 seconds, broadcast retains 512 events and lagging clients disconnect. Oversized responses return a bounded error.

Tests: `endless_line_is_bounded_and_truncation_rejected`, `validation`, `pipe_answers_while_startup_is_blocked`, `pipe_lifecycle_with_fake_runtime`.

Remaining: cross-account DACL denial, 32-client saturation and a full 60-second slow-client deadline were not integration-tested. The selected owner can deliberately occupy all slots. Authorized long-running mutations have operation lifetimes separate from frame-read deadlines.

## Tauri

Checked capability declarations, command registration/forwarding, native picker, bounded event reader and the current CSP.

The main-window capability exposes core, open dialog, events and HTTP/HTTPS URL opening; no filesystem/shell plugin grant. URL schemes exclude file, javascript and custom schemes. CSP limits default/scripts to self and connect sources to IPC; inline styles and data images/fonts remain allowed. Picker validates directory/ZIP kind. Bridge frame reads are bounded at 4 MiB. No permission/configuration change was needed.

Validation: desktop workspace compilation and existing bridge-facing pipe tests. No frontend/UI file was changed.

Remaining: WebView enforcement/opener behavior was inspected in configuration, not exercised through the desktop UI. The pre-existing CSP edit was left untouched.

## Process ownership

Checked suspended creation, Job assignment/resume failures, kill-on-close, restart/stop/wait ordering, single-instance mutex and port checks.

Children are suspended until assigned to a KILL_ON_JOB_CLOSE Job. Assignment/resume failure kills/waits. The same Job covers restarts. Stop kills/waits before worker completion; restart awaits stop. Foreign port owners are reported, never killed. Engine termination closes the Job handle, covering interrupted workers.

Tests: `closing_job_kills_child_and_foreign_port_is_preserved` launches a real managed child, checks foreign-port rejection, closes the Job and observes exit; `startup_connector_returns_offline_and_keeps_supervised_worker`, `port_conflict`, `backoff`.

Remaining: actual engine crash/forced-kill/reboot with live Caddy/cloudflared was not exercised end to end. Direct Job-close testing covers the mechanism. Blocking kill/wait assumes normal Windows termination completes.

## Component upgrades

Checked digest pins, bounded downloads/extraction, temporary files, replacement/cleanup, version probes and ensure-component serialization.

SHA-256, plus SHA-512 for Caddy, is verified before replacement. Downloads/expanded binaries are bounded at 256 MiB. Only the expected Caddy executable is extracted. A same-directory rename replaces the completed temporary binary. Windows rejects replacing a locked running binary, preserving the existing binary/process, removing the temporary file and reporting stop-and-retry guidance. No upgrade change was necessary.

Tests: `wrong_sha256_preserves_existing_binary_even_with_correct_sha512`, `failed_checksum_preserves_existing_binary_without_temp_files`, `locked_target_returns_action_and_cleans_temporary_file`, `zip_traversal_entries_only_extract_expected_caddy`, `offline_download_preserves_existing_binary_without_temp_files`, `post_install_probe_failure_and_unexpected_version_are_errors`.

Remaining: restrictive Windows handles were tested, not a live production upgrade. Post-replacement version failure reports an error without restoring the previous binary. Publisher pin freshness was not checked offline; no downloads were made.

## Service mode

Checked SCM creation/account/arguments, data-root resolution, owner-SID restriction, global mutex, pipe ACL and credential-store identity.

Installation persists the desktop user's explicit root and owner SID. SYSTEM may select that SID; another ordinary caller cannot select a foreign owner. Desktop/service derive the same pipe name and the ACL permits the desktop owner, despite the service running as SYSTEM. `service_and_task_preserve_same_data_root` verifies root/arguments.

Remaining: no real service was installed/started; cross-session reachability is untested. SID selection changes pipe ownership, not the process identity: LocalSystem does not automatically obtain the desktop user's Credential Manager entries. Cloudflare publishing/recovery in service mode remains an unresolved credential-identity limitation. Service-manager privileges/elevation were not exercised. User/service engines intentionally cannot hold the same per-owner mutex concurrently.

## Uninstall

Checked hooks, task deletion, process-path matching, service cleanup, wait behavior and data-deletion defaults.

Fixed leaving an automatic service registered after engine removal. Cleanup stops the matching installation's service via SCM, waits for Stopped, and removes it on uninstall before terminating residual matching processes. Uninstall removes the task before stopping hosting to avoid an automatic relaunch. Other installation paths are untouched. Failures abort uninstall; the unchecked data-deletion option keeps website data by default. No real service/task was changed during the audit.

Test: `uninstall_stops_and_removes_only_matching_service` runs the actual PowerShell script using mocked CIM functions for matching/foreign services, with installer-equivalent execution-policy arguments.

Remaining: actual NSIS packaging/uninstall, task deletion, SCM lifecycle and elevation failures were not exercised. Service removal may need an elevated retry. Opt-in data deletion targets only default user site data; custom roots remain.

## Races, transitions and locks

Checked concurrent dispatch, operation/provisioning guards, publish task registration, verifier cancellation, unpublish row selection, startup/retry cleanup and DB/status lock order.

Start/stop/restart/update/remove serialize through `operations` and reject provisioning. Public verification intentionally releases the publishing guard; local stop/update is permitted and verification does not overwrite stopped server status. Fixed unpublish selecting no rows during provisioning and then clearing the domain: it now obtains the provisioning guard before selection and rejects a busy operation. Cancel aborts/awaits provisioning then serializes cleanup with publishing. Startup holds both guards; cleanup retry holds publishing only.

No synchronous DB/status/verifier-registry guard was found spanning an await. `sites` takes DB then statuses; `status` releases statuses before `site`, avoiding reverse order. Tokio operation/provisioning/cache locks intentionally span awaits. Supervisor releases the worker-map guard before awaiting worker termination. Statistics holds its mutex only on a blocking thread.

Tests: `unpublish_during_provisioning_leaves_rows_unchanged`, `cancelling_provisioning_aborts_task_then_rolls_back_its_local_start`, `startup_resumes_waiting_without_rollback_and_retry_wakes_one_task`, existing rollback/concurrent-ingress tests.

Remaining: two-client interleavings were checked through shared guards, not stress-tested with multiple desktop processes. Mutations can wait for startup recovery; ping/status/statistics remain available.

## Timeouts and input panics

Checked Cloudflare, downloads, health/metrics, DNS/UDP/DoH, pinned HTTPS, DNS parsing, database decoding and production unwrap/expect sites.

Cloudflare's client timeout is 20 seconds; added an explicit request timeout so fallback client construction cannot remove it. Components use 120 seconds; health/metrics use 2. Public observations are bounded at 10 seconds; DoH/HTTPS use 4, authoritative nameserver lookup 2 and UDP receive 1. Transport/body failures return errors.

Fixed unknown `DeploymentState::from` strings panicking: conversion yields recovery-required. Unknown serialized snapshots remain decoding errors and are retained rather than rewritten. Fixed verification indexing truncated stored steps to return an error. Recovery isolates malformed rows. Other production unwraps were traced: DNS conversions follow exact length checks; rollback options/arrays are validated first; scheduling unwraps consume an internally constructed observation; component names are fixed/validated. Poisoned locks/task panics are not ordinary external input.

Tests: `unknown_state_does_not_panic`, `truncated_deployment_steps_return_error_without_panicking`, `malformed_recovery_row_does_not_block_other_rows`, `dns_a_aaaa_cname_compression_and_malformed_packets`, malformed Cloudflare tests.

Remaining: no live Internet timeout injection or exhaustive fuzzing. Mock HTTP uses loopback only. OS resolver work may outlive future cancellation internally; observations still time out.

## Log secrecy

Checked API logging/errors/token responses, serde errors, deployment/resource serialization, Credential Manager, activity forwarding and cloudflared stdout/stderr.

Tokens do not enter connector arguments, deployment JSON or normal activity. API logs contain metadata rather than request/response bodies; known API/tunnel tokens are redacted before logging/returning errors. Fixed malformed-response serde errors echoing tokens. Engine API clients also register previously stored tunnel tokens for redaction after restart. The pump removes the exact connector token from realistic cloudflared errors and avoids empty-string replacement. Only the intentional last-four-character account hint is exposed; Caddy receives no connector token.

Tests: `api_log_redacts_api_and_tunnel_tokens_including_provider_errors`, `malformed_zone_error_redacts_token_before_activity_can_log_it`, `realistic_cloudflared_error_redacts_token`.

Remaining: redaction handles known literal strings, not arbitrary encodings/fragments. Request bodies are not logged. This does not prevent malicious same-user processes reading environment/credentials. Existing private log files were not scanned or rewritten.

## Verification record

Ran `cargo build --workspace --offline` successfully:

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.45s
```

Ran `cargo test --workspace --offline` successfully. Nonempty suites, in Cargo output order (core, desktop library, engine, pipe integration):

```text
test result: ok. 94 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.71s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.40s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.12s
```

Both binary test targets and all three documentation-test targets also returned:

```text
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Total: 122 passing tests; no failing or ignored tests. Individual privilege-sensitive tests can return early with a skip message as described above; a Cargo pass alone does not establish those privileges. Real Caddy was available locally for the running-server update test. The Job test used a real Windows child; service uninstall used mocked SCM/CIM functions. `git diff --check` passed. Live network, reboot, real Windows service operation, actual engine crash with live hosting, real NSIS uninstall and desktop WebView interactions were not exercised. The service credential-identity limitation remains open.

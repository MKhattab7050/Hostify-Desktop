# Codex Task 4 — harden `hostify-core` deploy / import (correctness over features)

Repo D:\Hostify-Desktop. Read `docs/ARCHITECTURE.md`, `docs/ENGINE.md` (esp. its "not yet verified / known gaps" list from the previous pass) and `crates/hostify-core/src/{deploy,cloudflare,import,caddy,db}.rs`. Always: `%USERPROFILE%\.cargo\bin` on PATH, `cargo ... --offline`, no network, no new crates. MSVC linker now works.

## FILE OWNERSHIP
Edit ONLY `crates/hostify-core/**` and `docs/ENGINE.md`. Keep public APIs used by `crates/hostify-engine` compiling (`cargo build --workspace --offline` must pass; do not edit the engine — if a signature must change, add a compatibility shim and note it in `docs/CONTRACT-REQUESTS.md`).

## Fix, in priority order
1. **Deploy resource tracking / crash safety.** Persist *intent* BEFORE each Cloudflare create call and the resulting id immediately after, so an interruption between "create" and "save" cannot leave an untracked resource. `recover_interrupted()` must reconcile by looking up what exists in Cloudflare (tunnel by name, DNS record by hostname + `managed-by=hostify` tag) and either resume or roll back exactly what this deployment created. Never delete a DNS record or tunnel that this deployment did not create.
2. **Rollback correctness.** Reused tunnel: snapshot its previous ingress config before `put_tunnel_ingress` and restore it on failure. Newly created tunnel: delete only if this deployment created it AND it has no other ingress entries. DNS: delete only our tagged record via `delete_dns_if_ours`. Stop connector/local site only if this deployment started them.
3. **Tests (all must pass, no network, fake `SiteRuntime` + wiremock Cloudflare):** full success path ending `live`; failure injected at EACH of the 7 steps with an assertion of exactly which resources were rolled back and that unrelated DNS records/tunnels were untouched; no-consent refusal makes zero Cloudflare calls; DNS conflict aborts without writes; "DNS has not propagated" ends `failed` (never `live`); interrupted-then-recovered; idempotent re-publish of the same site/hostname creates nothing twice.
4. **Import hardening.** Prune old `releases/` (keep newest 3). Reduce check-then-use races: on Windows open source files with `FILE_FLAG_OPEN_REPARSE_POINT` semantics (or re-verify via the opened handle's metadata) so a link swapped in after the directory scan is rejected; re-check size after open. Add tests (Windows junction/symlink creation may need privileges — skip gracefully with a clear message if it can't be created).
5. Update `docs/ENGINE.md`: what is now covered by tests vs. still unverified against real Cloudflare.

## Definition of done
`cargo test -p hostify-core --offline` AND `cargo build --workspace --offline` pass; paste the real output including the test list. Work in small compile-test steps; do not stop with partial work. Be honest about anything not done.

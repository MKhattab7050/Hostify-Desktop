# Codex Task 8 — remove Cloudflare DNS record tags (error 9300), ownership via local DB + ids

Repo D:\Hostify-Desktop. `%USERPROFILE%\.cargo\bin` on PATH, `cargo ... --offline`, no network, no new crates. Be FAST: small, surgical change. Do NOT write new tests. Only touch existing tests that break because they assert `tags`, and keep the suite compiling and passing (run `cargo build --workspace --offline` and `cargo test --workspace --offline` once at the end).

## FILE OWNERSHIP
Edit ONLY `crates/**` and `docs/ENGINE.md` (one short paragraph). Not `src/**`, not `src-tauri/**`.

## Bug (real, from the user's account)
Publishing fails at "Add DNS record": HTTP 400, code 9300 "DNS record has 1 tags, exceeding the quota of 0." Cloudflare Free accounts allow zero DNS record tags. In `crates/hostify-core/src/cloudflare.rs` DNS create sends `"tags":["hostify-deployment:<owner>"]`, the owner check on delete/lookup requires that tag, and the update path copies a `tags` key from the existing record.

## Fix
1. **Never send `tags`** in any DNS create or update request (no empty array either), and never copy a `tags` key on update. Search the whole workspace for `tags`.
2. **No ownership logic based on tags (or on the comment).** Ownership = Hostify's local SQLite state + Cloudflare ids/identity:
   - A record is OURS for rollback/delete only if its id is the one journaled for this deployment (`dns_id` in the deployment's created-resources record). Keep the "journal intent before create, id right after" scheme; for the crash window between create and saving the id, reconcile by looking up the exact hostname for a `CNAME` whose content is exactly `<tunnel-id>.cfargotunnel.com` for the tunnel THIS deployment created (that identity is unique to us) and adopt that id.
   - `delete_dns_if_ours` / rollback: re-GET the record by id and delete it only if (a) the id is journaled for this deployment AND (b) it is still a CNAME pointing at that deployment's tunnel. Anything else ⇒ leave it, report cleanly. Missing record ⇒ success (idempotent).
3. **Pre-create check** for the exact hostname: list A, AAAA, CNAME (and conflicting TXT) records. Decision (`plan_dns`):
   - none ⇒ `create`;
   - CNAME pointing at the tunnel being used ⇒ reuse, no write (`update_own` is not needed);
   - CNAME pointing at some `<id>.cfargotunnel.com` where `<id>` is a tunnel Hostify created per the local DB (pass a set of Hostify-owned tunnel ids into the planner from the deploy layer; add a compat shim so existing callers compile) ⇒ `update_own` (repoint, requires the consent flag, remember previous content for rollback restore);
   - anything else (any A/AAAA, other CNAME, TXT) ⇒ `conflict`, zero writes, shown to the user. Never overwrite or delete unrelated records.
4. The informational `comment` ("managed-by=hostify") may stay purely as a human-readable label in the Cloudflare dashboard, but NOTHING may read it to make a decision. If you prefer to drop it entirely, that is also fine — choose the lower-risk option.
5. Idempotent publish (no duplicate tunnel / DNS record) and safe rollback must keep working with the new ownership rules.

## Definition of done
`cargo build --workspace --offline` and the existing `cargo test --workspace --offline` pass (paste the summary lines). Report exactly which files/functions changed and anything you could not verify. No UI changes, no new features.

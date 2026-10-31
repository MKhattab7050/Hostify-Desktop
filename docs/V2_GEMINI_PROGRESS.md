# Hostify Desktop V2 — Gemini Completion Progress Tracker

## Baseline & Forensics
- **Date & Time**: 2026-10-31T14:04:00+02:00
- **Base Version / Tag**: `v0.2.0` (f3ad9bf)
- **Handoff WIP Commit**: `f7aad60` (WIP: save unfinished MySQL backend and V2 screens)
- **Active Branch**: `feature/v2-gemini-completion`
- **Application Data Backup**: `%LOCALAPPDATA%\Hostify\hostify.db.v2-baseline-backup` created
- **Baseline Verification**:
  - `cargo check --workspace --offline`: PASSED (1 minor unreachable pattern warning in engine.rs)
  - `cargo test --workspace --offline`: PASSED (131 core, 35 engine, 9 pipe integration, 2 node offline = 177 tests passed, live tests ignored)
  - `pnpm lint`: PASSED (0 errors)
  - `pnpm build`: PASSED (Vite + Tailwind production bundle created in dist/)

---

## Task Matrix & Status

| Phase / Feature | Status | Tests Executed | Notes / Evidence |
|---|---|---|---|
| **Phase 0: Repos Forensics & Baseline** | VERIFIED | `cargo check`, `cargo test`, `pnpm lint`, `pnpm build` | All baseline tests pass. Local db backed up. |
| **Fix: API / Engine Field Mismatches** | NOT STARTED | - | MySQL backup `time` vs `createdAt`, duplicate event in engine.rs |
| **Priority 1: 8-Step Creation Wizard** | NOT STARTED | - | Full Node.js, PHP, and Static import wizard |
| **Priority 1.1: Websites Page V2 Integration** | NOT STARTED | - | Wire `AppDetails` for dynamic apps |
| **Priority 2: MySQL Management & Tests** | NOT STARTED | - | Test setup, lifecycle, security audit, write `mysql_live.rs` |
| **Priority 3: V2 Screens Polish & Wiring** | NOT STARTED | - | Runtimes, Databases, Deployments, Settings |
| **Priority 4: UI/UX Transformation** | NOT STARTED | - | Polish design, animations, theme contrast, error states |
| **Priority 5: Cloudflare Publishing Reliability**| NOT STARTED | - | Verification and retry logic check |
| **Priority 6: Windows Reliability & Supervisor**| NOT STARTED | - | Background hosting, recovery, job objects |
| **Priority 7: Security Audit** | NOT STARTED | - | Threat model, DPAPI, least-privilege, sanitization |
| **Priority 8: End-to-End Sample Apps Verification** | NOT STARTED | - | Static, Express, Socket.IO, PHP, PHP+MySQL, Node+MySQL |
| **Release Candidate Build & Packaging** | NOT STARTED | - | Tauri NSIS bundle, version bump, licenses |

---

## Known Risks & Blockers
1. **Live Runtimes Dependency**: Live tests require `HOSTIFY_LIVE_RUNTIMES` pointed at `D:\Hostify-Desktop\target\live-runtimes`. Verified: runtimes (Node 24/26, PHP 8.4/8.5, MySQL 8.4.10) exist locally in `target\live-runtimes\runtimes`.
2. **DPAPI in Sandbox**: Windows DPAPI (`CryptProtectData`) requires user profile access; unit tests use test sealers where possible.
3. **Database Backup Contract Mismatch**: Engine returns `time`, UI expects `createdAt`. Must unify on `createdAt` (or return both).
4. **Import Wizard Gap**: Adding dynamic sites (Node/PHP) currently cannot be done from UI because `PublishWizard.tsx` only calls `api.addSite` without kind detection or config.

---

## Git Commits Log
- `f7aad60` - Baseline WIP commit
- *(New commits will be logged here)*

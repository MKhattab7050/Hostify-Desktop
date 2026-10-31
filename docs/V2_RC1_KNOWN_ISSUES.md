# Hostify Desktop V2 — RC1 Known Issues & Architectural Boundaries

**Release Candidate**: Hostify Desktop V2 RC1 (`2.0.0-rc.1`)  
**Installer**: `D:\Hostify-Desktop\target\release\bundle\nsis\Hostify_2.0.0-rc.1_x64-setup.exe`  
**SHA-256**: `8B3941F790AB38BD252DE50B5D623963826A523609466C42A9C55CF3F00684C1`  

---

## 1. Architectural Boundaries (By Design)

### Boundary 1: Single-Tenant Windows User Execution
- **Severity**: Architectural Limitation / Documented Boundary
- **Description**: Hosted Node.js and PHP applications execute under the current Windows user profile without administrator rights.
- **Impact**: Code hosted inside Hostify has the same filesystem and network permissions as any program executed by the logged-in user. It is not an OS-level multi-tenant sandbox (such as Docker or Hyper-V).
- **Mitigation**: Users must only host applications they trust. Security documentation in `docs/SECURITY.md` clearly outlines this boundary.

### Boundary 2: Loopback Database Access
- **Severity**: Low / Security Feature
- **Description**: MySQL binds strictly to `127.0.0.1:3307`. It cannot be reached from other devices on the local area network (LAN) or over the internet.
- **Impact**: Remote database management tools (e.g. DBeaver, TablePlus on another PC) cannot connect directly unless running on the same Windows PC or via SSH port forwarding.
- **Mitigation**: Built-in Hostify Database Manager provides connection testing, credential reveal, backup, and restore on the host machine.

---

## 2. Unresolved Minor Issues & Limitations

### Issue 1: Real Reboot Recovery Unverified on Host PC
- **Severity**: Medium
- **Status**: `BLOCKED` (By design safety rule)
- **Description**: Windows reboot recovery relies on the Scheduled Task configured by `hostify-engine install-task`. While process recovery and graceful shutdown were fully verified in test suites, a physical restart of the user's host workstation was not executed to avoid interrupting active user workflows.
- **Mitigation**: Verify on a dedicated test machine or VM before general availability.

### Issue 2: Live Public HTTPS Tunnel Test on Production Domain
- **Severity**: Low
- **Status**: `BLOCKED` (Requires user-provided Cloudflare domain)
- **Description**: Cloudflare tunnel creation, ingress generation, and public HTTPS verification logic passed all unit and UI verification checks, but no real production Cloudflare domain was modified during testing.
- **Mitigation**: Conduct manual test with an authorized sandbox domain during limited testing.

### Issue 3: Large Dependency Trees on Cold Install
- **Severity**: Low
- **Description**: When importing a Node.js project that requires a fresh `npm install` with native modules (e.g. `bcrypt`, `sharp`), build times can extend beyond 60 seconds depending on disk speed and network bandwidth.
- **Mitigation**: The build panel displays live streaming output so users have continuous visual feedback during long compilation steps.

---

## 3. Clean-Machine Verification Checklist

For QA engineers validating on a clean Windows 10/11 VM:
- [ ] Run `Hostify_2.0.0-rc.1_x64-setup.exe` without admin rights.
- [ ] Verify onboarding flow checks Caddy and cloudflared components.
- [ ] Install Node.js 24 and PHP 8.4 from the Runtimes tab.
- [ ] Import `samples/express-basic` and verify local HTTP response.
- [ ] Create a database in the Databases tab and initialize MySQL.
- [ ] Import `samples/php-mysql` and test CRUD note creation.
- [ ] Uninstall Hostify and verify user data is retained unless checkbox is ticked.

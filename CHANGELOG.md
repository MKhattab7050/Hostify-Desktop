# Changelog

## 0.2.0 (2026-10-30)

### Publishing

- Fixed the "Verify HTTPS" failure. The old check asked this PC's DNS resolver, reported "DNS has not propagated" after 180 seconds without evidence, then deleted the DNS record and tunnel. That deletion re-armed negative DNS caching and caused the next failure.
- Verification now records evidence for each layer: Cloudflare zone, DNS record, connector, public DNS, TLS, HTTP and content. It reports what failed rather than guessing.
- Proof of deployment is a one-time marker served only by this website, fetched over validated HTTPS. An unrelated HTTP 200 is not accepted, and TLS checks are never weakened.
- Waiting never deletes anything. After 30 minutes without success, publishing pauses as "recovery required", keeps every resource, and offers Check again and Unpublish.
- Publishing has explicit, saved stages and resumes after a restart. Only an interrupted provisioning step is rolled back, and only the resources that attempt created.
- Added retry and safe cancel.
- Fixed published websites showing a blank page. Each site answered only to `127.0.0.1`, so requests arriving through the tunnel with your real hostname got an empty 200 response. Sites now answer whatever the hostname and still listen only on this PC.
- A failed IPv6 connection no longer hides the real result from IPv4 in the diagnostics.

### Interface

- Redesigned Dashboard, Websites, Publishing Wizard, Domains, Activity & Logs and Settings, in dark and light themes, with keyboard support and reduced-motion support.
- Added first-run onboarding: system check, components, first website, preview, Cloudflare, publish.
- Activity now uses plain sentences. Routine starts no longer fill the feed; older entries are rewritten on upgrade.
- The Dashboard warns when the disk Hostify uses has under 5 GB free.
- Removed the Windows service switch. A service runs as SYSTEM and cannot read the Cloudflare token in your Credential Manager.

### Engine and security

- The control pipe answers during startup recovery; startup no longer blocks on the connector.
- A process gets 15 seconds to start before a failed health check counts, which removes a false warning at every start.
- Added limits on control-pipe sessions and read time, statistics caching, and retention for activity and deployment history.
- Added stricter import checks: Windows device names, long paths and importing a folder into itself.
- Removing a website refuses non-UUID ids and keeps deployments whose Cloudflare cleanup is unfinished.
- Unpublishing during provisioning no longer clears the domain.
- Unknown saved states no longer crash the engine; they become "recovery required".

### Packaging

- Added third-party licence notices to the installer, and version metadata.
- Uninstall also stops and removes a matching service if one exists.

### Upgrade notes

The database moves to schema 3 automatically. Websites, settings and your Cloudflare connection are kept.

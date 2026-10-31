# Isolated Windows Live Verification

Run from **PowerShell 7** on a **dedicated test Windows machine** with Caddy and a disposable Hostify website already installed.

```powershell
pwsh -NoProfile -File scripts/verify-hosting-windows.ps1 `
    -LocalPort 18080 `
    -AllowedHostname test.example.test `
    -HealthPath /
```

The script only **reads** local Caddy and site health. It checks that the expected Host header is accepted (HTTP 2xx/3xx) and that an untrusted foreign Host receives **421**. It reports missing managed Caddy executable, failed connections and local route failures rather than hiding them.

For optional public HTTPS checks, use a **disposable domain you control** and a unique body marker:

```powershell
pwsh -NoProfile -File scripts/verify-hosting-windows.ps1 `
    -LocalPort 18080 -AllowedHostname staging.example.test `
    -CheckPublicHttps -ExpectedPublicMarker 'UNIQUE-STAGING-NONCE' `
    -OutputJson 'C:\\Temp\\hostify-verification.json'
```

Public HTTPS only passes when TLS succeeds and the response includes a marker unique to the intended staging deployment, not when a domain merely returns HTTP 200. The output always includes `productionReady: false`, because this script cannot prove MySQL backup integrity, OS isolation or customer data safety.

**Never use production niga.online for these tests.** The script never changes DNS, Tunnels, credentials, databases, power settings or websites. It refuses to overwrite an existing report. Tests requiring actual installed Windows binaries remain unverified on a clean CI runner, not silently skipped.

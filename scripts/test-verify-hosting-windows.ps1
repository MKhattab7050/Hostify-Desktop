# Runs on a clean GitHub Windows runner; no real hosting binaries required.
# Negative smoke test must report a failure, not a fabricated positive result.
$ErrorActionPreference='Stop'
# The negative smoke check intentionally exits 1; keep PowerShell from
# converting that expected native exit into a terminating exception.
$PSNativeCommandUseErrorActionPreference = $false
$script=Join-Path $PSScriptRoot 'verify-hosting-windows.ps1'
$parseErrors=$null
$tokens=$null
[System.Management.Automation.Language.Parser]::ParseFile($script,[ref]$tokens,[ref]$parseErrors)|Out-Null
if ($parseErrors.Count -ne 0) { throw "Verification script has PowerShell parse errors: $parseErrors" }
$result = & pwsh -NoProfile -File $script -LocalPort 1 -AllowedHostname staging.example.test -HealthPath /
$exit=$LASTEXITCODE
if ($exit -ne 1) { throw "Expected failure for unavailable local test site; got $exit" }
$report=$result | ConvertFrom-Json
if ($report.productionReady -ne $false) { throw "Verifier must never claim production readiness" }
if ($report.fail -lt 1 -or $report.unverified -lt 1) {
    throw "Missing failed local and unverified external checks"
}
if (@($report.checks | Where-Object { $_.check -eq 'foreign_host_isolation' -and $_.status -eq 'pass' }).Count -gt 0) {
    throw "A non-existent Caddy server cannot pass foreign-host isolation"
}
# A syntactically invalid hostname must be rejected *before* an HTTP
# attempt. We deliberately include bad middle labels that were accepted by
# the earlier first/last-character-only guard.
foreach ($badHostname in @(
    'staging..example.test',
    'staging.-example.test',
    'staging.example-.test',
    'staging._private.test',
    '-staging.example.test',
    'staging.example.test.'
)) {
    $errorOutput = & pwsh -NoProfile -File $script -LocalPort 1 -AllowedHostname $badHostname -HealthPath / 2>&1
    if ($LASTEXITCODE -eq 0) {
        throw "Invalid hostname was accepted: $badHostname"
    }
    if (($errorOutput | Out-String) -notmatch 'AllowedHostname must be a valid DNS hostname') {
        throw "Invalid hostname did not hit the DNS syntax guard: $badHostname"
    }
}
Write-Host 'PASS: negative Windows integration verification fails closed on absent components'

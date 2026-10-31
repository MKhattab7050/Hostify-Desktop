# Read-only Hostify native Windows publishing verifier.
# Does NOT install binaries, start processes, modify DNS or touch user data.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][ValidateRange(1,65535)][int]$LocalPort,
    [Parameter(Mandatory=$true)][ValidateNotNullOrEmpty()][string]$AllowedHostname,
    [string]$HealthPath = '/',
    [string]$HostifyRoot = (Join-Path $env:LOCALAPPDATA 'Hostify'),
    [switch]$CheckPublicHttps,
    [string]$ExpectedPublicMarker,
    [string]$OutputJson
)
$ErrorActionPreference = 'Stop'
$checks = [System.Collections.Generic.List[object]]::new()
function Add-Check([string]$Name,[string]$Status,[string]$Detail) {
    $checks.Add([pscustomobject]@{ check=$Name; status=$Status; detail=$Detail })
}
function Test-Http([int]$Port,[string]$Path,[string]$HostHeader) {
    $handler = [System.Net.Http.HttpClientHandler]::new()
    $handler.AllowAutoRedirect = $false
    $client = [System.Net.Http.HttpClient]::new($handler)
    $client.Timeout = [TimeSpan]::FromSeconds(5)
    try {
        $url = "http://127.0.0.1:$Port$Path"
        $request = [System.Net.Http.HttpRequestMessage]::new([System.Net.Http.HttpMethod]::Get,$url)
        $request.Headers.Host = $HostHeader
        $response = $client.SendAsync($request).GetAwaiter().GetResult()
        try { return [int]$response.StatusCode }
        finally { $response.Dispose() }
    } finally { $client.Dispose(); $handler.Dispose() }
}
if (-not $HealthPath.StartsWith('/') -or $HealthPath.Contains([char]13) -or $HealthPath.Contains([char]10)) {
    throw 'HealthPath must start with / and cannot contain control newlines.'
}
# Validate every DNS label, not only the first/last characters of the
# complete hostname. This rejects malformed inputs like site.-tenant.test
# and site.tenant-.test before making any HTTP requests.
$hostnameLabels = $AllowedHostname.Split('.')
$invalidLabels = @($hostnameLabels | Where-Object {
    $_.Length -lt 1 -or $_.Length -gt 63 -or
    $_ -cnotmatch '^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$'
})
if ($AllowedHostname.Length -gt 253 -or $hostnameLabels.Count -lt 2 -or $invalidLabels.Count -gt 0) {
    throw 'AllowedHostname must be a valid DNS hostname.'
}
$binary = Join-Path $HostifyRoot 'bin/caddy.exe'
if (Test-Path -LiteralPath $binary -PathType Leaf) {
    Add-Check 'caddy_binary' 'pass' 'The native Caddy executable is present.'
} else {
    Add-Check 'caddy_binary' 'fail' 'Caddy is not installed at the managed Hostify path.'
}
try {
    $status = Test-Http -Port $LocalPort -Path '/.well-known/hostify-caddy-ok' -HostHeader $AllowedHostname
    if ($status -ge 200 -and $status -lt 300) {
        Add-Check 'caddy_readiness' 'pass' "Readiness endpoint returned HTTP $status."
    } else { Add-Check 'caddy_readiness' 'fail' "Readiness returned HTTP $status; expected 2xx." }
} catch { Add-Check 'caddy_readiness' 'fail' 'Could not connect to the Caddy readiness endpoint.' }
try {
    $status = Test-Http -Port $LocalPort -Path $HealthPath -HostHeader $AllowedHostname
    if ($status -ge 200 -and $status -lt 400) {
        Add-Check 'allowed_host_health' 'pass' "Allowed Host returned HTTP $status."
    } else { Add-Check 'allowed_host_health' 'fail' "Allowed Host returned HTTP $status; expected 2xx/3xx." }
} catch { Add-Check 'allowed_host_health' 'fail' 'Allowed Host check failed; check the application and Caddy logs.' }
try {
    $status = Test-Http -Port $LocalPort -Path $HealthPath -HostHeader 'foreign.invalid'
    if ($status -eq 421) {
        Add-Check 'foreign_host_isolation' 'pass' 'Foreign Host was rejected with HTTP 421.'
    } else {
        Add-Check 'foreign_host_isolation' 'fail' "Foreign Host returned HTTP $status; expected 421. Cross-site isolation is not confirmed."
    }
} catch { Add-Check 'foreign_host_isolation' 'fail' 'Foreign Host probe failed; cannot confirm virtual host isolation.' }
if ($CheckPublicHttps) {
    try {
        $uri = "https://$AllowedHostname$HealthPath"
        $handler = [System.Net.Http.HttpClientHandler]::new()
        $handler.AllowAutoRedirect = $false
        $client = [System.Net.Http.HttpClient]::new($handler)
        $client.Timeout = [TimeSpan]::FromSeconds(10)
        try {
            $response = $client.GetAsync($uri).GetAwaiter().GetResult()
            try {
                $status = [int]$response.StatusCode
                $body = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
                if ($status -ge 200 -and $status -lt 400 -and
                    -not [string]::IsNullOrWhiteSpace($ExpectedPublicMarker) -and
                    $body.Contains($ExpectedPublicMarker)) {
                    Add-Check 'public_https' 'pass' 'Public TLS validation, HTTP status, and unique application marker all matched.'
                } else {
                    Add-Check 'public_https' 'unverified' 'Public HTTPS cannot be attributed to the correct deployment without a matching unique marker and correct response.'
                }
            } finally { $response.Dispose() }
        } finally { $client.Dispose();$handler.Dispose() }
    } catch { Add-Check 'public_https' 'fail' 'Public HTTPS/TLS validation failed. No DNS changes were attempted.' }
} else {
    Add-Check 'public_https' 'unverified' 'Public HTTPS not checked. Enable CheckPublicHttps with a unique ExpectedPublicMarker to verify.'
}
Add-Check 'database_integrity' 'unverified' 'This read-only verifier does not test live MySQL backups or restores.'
Add-Check 'windows_process_isolation' 'unverified' 'Restricted Windows execution identity requires independent service/security testing.'
Add-Check 'tunnel_connector' 'unverified' 'This script does not authenticate or mutate Cloudflare Tunnel.'
$summary = [pscustomobject]@{
    checkedAt = [DateTimeOffset]::UtcNow.ToString('o')
    target = "127.0.0.1:$LocalPort"
    hostname = $AllowedHostname
    pass = @($checks | Where-Object status -eq 'pass').Count
    fail = @($checks | Where-Object status -eq 'fail').Count
    unverified = @($checks | Where-Object status -eq 'unverified').Count
    productionReady = $false
    checks = @($checks)
}
$json = $summary | ConvertTo-Json -Depth 6
if ($OutputJson) {
    $dest = [System.IO.Path]::GetFullPath($OutputJson)
    if (Test-Path -LiteralPath $dest) { throw 'Refusing to overwrite an existing verification report.' }
    [System.IO.File]::WriteAllText($dest,$json,[System.Text.UTF8Encoding]::new($false))
}
Write-Output $json
if ($summary.fail -gt 0) { exit 1 }
exit 0

# Real end-to-end check of component detection, install/repair, local hosting without cloudflared,
# engine restart and child-process recovery. Uses the network (official Caddy/cloudflared releases)
# and a fresh temporary data dir. It refuses to run if an engine is already running for this user,
# because the control pipe is per user.
param(
    [string]$EnginePath = (Join-Path (Split-Path $PSScriptRoot -Parent) 'target\debug\hostify-engine.exe'),
    [string]$DataDir = (Join-Path ([System.IO.Path]::GetTempPath()) ('Hostify-e2e-' + [guid]::NewGuid().ToString('N')))
)
$ErrorActionPreference = 'Stop'
$sample = Join-Path $PSScriptRoot 'sample-site'
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$script:id = 0; $script:fail = 0; $script:engine = $null
function Check($name, [bool]$ok, $detail = '') {
    if ($ok) { Write-Host "  PASS  $name" -ForegroundColor Green } else { Write-Host "  FAIL  $name  $detail" -ForegroundColor Red; $script:fail++ }
}
function Connect-Pipe {
    $c = [System.IO.Pipes.NamedPipeClientStream]::new('.', "hostify-engine-$sid", [System.IO.Pipes.PipeDirection]::InOut, [System.IO.Pipes.PipeOptions]::Asynchronous)
    $c.Connect(15000); $e = [System.Text.UTF8Encoding]::new($false)
    $script:pipe = $c
    $script:w = [System.IO.StreamWriter]::new($c, $e, 4096, $true); $script:w.AutoFlush = $true
    $script:r = [System.IO.StreamReader]::new($c, $e, $false, 4096, $true)
}
function Call($method, $params = @{}, $ms = 30000) {
    $script:id++
    $script:w.WriteLine((@{ id = $script:id; method = $method; params = $params } | ConvertTo-Json -Compress -Depth 9))
    $t = $script:r.ReadLineAsync(); if (-not $t.Wait($ms)) { throw "timeout calling $method" }
    $o = $t.Result | ConvertFrom-Json
    if ($o.PSObject.Properties.Name -contains 'error') { return [pscustomobject]@{ Error = [string]$o.error } }
    return [pscustomobject]@{ Result = $o.result }
}
function Start-Engine {
    $script:engine = Start-Process -FilePath $EnginePath -ArgumentList @('--data-dir', "`"$DataDir`"") -PassThru -WindowStyle Hidden
    Connect-Pipe
    $null = Call 'ping'
}
function Stop-Engine {
    if ($script:pipe) { $script:pipe.Dispose(); $script:pipe = $null }
    if ($script:engine -and -not $script:engine.HasExited) { Stop-Process -Id $script:engine.Id -Force; $null = $script:engine.WaitForExit(15000) }
    Start-Sleep 2   # let the Job Object close managed children
}
function Wait-Http($url, $secs = 20) {
    $until = (Get-Date).AddSeconds($secs)
    while ((Get-Date) -lt $until) { try { $b = (Invoke-WebRequest -UseBasicParsing -TimeoutSec 3 $url).Content; if ($b -match 'hostify-smoke-ok') { return $true } } catch {}; Start-Sleep 1 }
    return $false
}

if (Get-Process hostify-engine -ErrorAction SilentlyContinue) { throw 'An engine is already running for this user. Quit it first (the control pipe is per user).' }
if (-not (Test-Path $EnginePath)) { throw "Engine not found: $EnginePath (run cargo build -p hostify-engine)" }
New-Item -ItemType Directory -Force $DataDir | Out-Null
Write-Host "Data dir: $DataDir"
try {
    Write-Host "`n[A] Empty data dir: status and actionable start error"
    Start-Engine
    $s = (Call 'engine_status').Result
    Check 'caddy reported not installed' ($s.caddy.installed -eq $false)
    Check 'cloudflared reported not installed' ($s.cloudflared.installed -eq $false)
    Check 'localHostingReady is false' ($s.localHostingReady -eq $false)
    Check 'status reports this data dir' ($s.dataDir -ieq $DataDir) $s.dataDir
    Check 'status reports bin under data dir' ($s.binDir -ieq (Join-Path $DataDir 'bin')) $s.binDir
    $site = (Call 'add_site' @{ name = 'E2E site'; sourcePath = $sample; kind = 'directory' }).Result
    $st = Call 'start_site' @{ id = $site.id }
    Check 'start fails when caddy missing' ($null -ne $st.Error)
    Check 'error is actionable (mentions Install components)' ($st.Error -match 'Install components') $st.Error
    Check 'error is not a raw os error 2' ($st.Error -notmatch 'os error|cannot find the file') $st.Error
    Check 'error does not blame cloudflared' ($st.Error -notmatch 'cloudflared') $st.Error
    $listed = (Call 'list_sites').Result | Where-Object { $_.id -eq $site.id }
    Check 'site statusDetail carries the message' ($listed.statusDetail -match 'Install components') $listed.statusDetail

    Write-Host "`n[B] Install Caddy only: local hosting works without cloudflared"
    $s = (Call 'ensure_components' @{ names = @('caddy') } 240000).Result
    Check 'caddy ready after install' ($s.caddy.ready -eq $true) ($s.caddy | ConvertTo-Json -Compress)
    Check 'caddy has a version' ([bool]$s.caddy.version)
    Check 'cloudflared still not installed' ($s.cloudflared.installed -eq $false)
    Check 'localHostingReady true' ($s.localHostingReady -eq $true)
    Check 'componentsReady (publish) still false' ($s.componentsReady -eq $false)
    $s2 = (Call 'engine_status').Result
    Check 're-detected without restart' ($s2.localHostingReady -eq $true)
    $st = Call 'start_site' @{ id = $site.id }
    Check 'site starts with cloudflared absent' ($null -ne $st.Result -and $st.Result.status -eq 'running') ($st | ConvertTo-Json -Compress -Depth 4)
    $url = (Call 'preview_url' @{ id = $site.id }).Result
    Check 'real HTTP preview served by Caddy' (Wait-Http $url) $url
    $pub = Call 'publish_site' @{ siteId = $site.id; hostname = 'test.example.com'; consentDns = $true }
    Check 'publish preflight names cloudflared only' ($pub.Error -match 'cloudflared' -and $pub.Error -notmatch 'Caddy') $pub.Error

    Write-Host "`n[C] Broken binary is detected and repaired"
    $cf = Join-Path $DataDir 'bin\cloudflared.exe'
    New-Item -ItemType File -Force $cf | Out-Null   # zero-byte stand-in for a corrupt/quarantined file
    $s = (Call 'engine_status').Result
    Check 'zero-byte cloudflared: installed but not ready' ($s.cloudflared.installed -eq $true -and $s.cloudflared.ready -eq $false)
    $s = (Call 'ensure_components' @{ names = @('cloudflared') } 240000).Result
    Check 'cloudflared repaired and ready' ($s.cloudflared.ready -eq $true) ($s.cloudflared | ConvertTo-Json -Compress)
    Check 'componentsReady now true' ($s.componentsReady -eq $true)
    Check 'no temp files left in bin' (-not (Get-ChildItem (Join-Path $DataDir 'bin') -Force | Where-Object { $_.Name -like '.*tmp' }))

    Write-Host "`n[D] Engine restart: components and site come back"
    Stop-Engine
    Check 'site no longer served while engine is down' (-not (Wait-Http $url 3))
    Start-Engine
    $s = (Call 'engine_status').Result
    Check 'components still ready after restart' ($s.componentsReady -eq $true)
    Check 'site auto-started after restart' (Wait-Http $url 25) $url

    Write-Host "`n[E] Child recovery: kill Caddy, engine restarts it"
    $caddy = Get-CimInstance Win32_Process -Filter "Name='caddy.exe'" | Where-Object { $_.ExecutablePath -like "$DataDir*" }
    Check 'found managed caddy process' ($null -ne $caddy)
    $caddy | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
    Check 'site recovered after caddy was killed' (Wait-Http $url 45) $url

    Write-Host "`n[F] Stop"
    $null = Call 'stop_site' @{ id = $site.id }
    Check 'site stopped' (-not (Wait-Http $url 3))
}
finally {
    Stop-Engine
}
if ($script:fail -eq 0) { Write-Host "`nPASS: all component/hosting checks" -ForegroundColor Green; exit 0 } else { Write-Host "`nFAIL: $($script:fail) check(s) failed. Data dir kept: $DataDir" -ForegroundColor Red; exit 1 }

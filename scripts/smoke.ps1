param([string]$ComponentsDir = "$env:LOCALAPPDATA\Hostify\bin")
$ErrorActionPreference = 'Stop'
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$env:CARGO_BUILD_JOBS = '1'
$repo = Split-Path $PSScriptRoot -Parent
$pipe = $null; $reader = $null; $writer = $null; $engine = $null; $siteId = $null
$script:requestId = 0
$passed = $false
$dataDir = Join-Path ([System.IO.Path]::GetTempPath()) ('Hostify-smoke-' + [guid]::NewGuid().ToString('N'))
function Invoke-Engine([string]$Method, [hashtable]$Params = @{}) {
    $script:requestId++
    $id = $script:requestId
    $writer.WriteLine((@{id=$id;method=$Method;params=$Params} | ConvertTo-Json -Depth 20 -Compress))
    $lineTask = $reader.ReadLineAsync()
    if (!$lineTask.Wait(30000)) { throw "Engine request timed out: $Method" }
    $line = $lineTask.Result
    if ($null -eq $line) { throw 'Engine disconnected' }
    $reply = $line | ConvertFrom-Json
    if ($reply.id -ne $id) { throw "Unexpected response id for $Method" }
    if ($reply.PSObject.Properties.Name -contains 'error') { throw [string]$reply.error }
    if (!($reply.PSObject.Properties.Name -contains 'result')) { throw 'Missing engine result' }
    return $reply.result
}
Push-Location $repo
try {
    cargo build -p hostify-engine -p hostify-desktop --offline
    if ($LASTEXITCODE -ne 0) { throw 'Offline build failed' }
    $sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $pipeName = "hostify-engine-$sid"
    # Never exercise or shut down the user's existing engine.
    $probe = [System.IO.Pipes.NamedPipeClientStream]::new('.', $pipeName, [System.IO.Pipes.PipeDirection]::InOut)
    try {
        try { $probe.Connect(250) } catch [System.TimeoutException] { }
        if ($probe.IsConnected) { throw 'An engine is already running. Quit it before this isolated smoke test.' }
    } finally { $probe.Dispose() }
    New-Item -ItemType Directory -Force -Path (Join-Path $dataDir 'bin') | Out-Null
    # Reuse already downloaded components; this script never calls ensure_components or downloads.
    foreach ($component in @('caddy.exe','cloudflared.exe')) {
        $source = Join-Path $ComponentsDir $component
        if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination (Join-Path $dataDir 'bin') }
    }
    $targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
    $engineExe = Join-Path $targetRoot 'debug/hostify-engine.exe'
    $engine = Start-Process -FilePath $engineExe -ArgumentList @('--data-dir', ('"' + $dataDir + '"')) -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $dataDir 'stdout.log') -RedirectStandardError (Join-Path $dataDir 'stderr.log')
    $pipe = [System.IO.Pipes.NamedPipeClientStream]::new('.', $pipeName, [System.IO.Pipes.PipeDirection]::InOut, [System.IO.Pipes.PipeOptions]::Asynchronous)
    $pipe.Connect(10000)
    if ($engine.HasExited) { throw "Isolated engine exited with code $($engine.ExitCode)" }
    $encoding = [System.Text.UTF8Encoding]::new($false)
    $reader = [System.IO.StreamReader]::new($pipe, $encoding, $false, 4096, $true)
    $writer = [System.IO.StreamWriter]::new($pipe, $encoding, 4096, $true)
    $writer.AutoFlush = $true
    if ((Invoke-Engine 'ping') -ne 'pong') { throw 'Engine ping failed' }
    $site = Invoke-Engine 'add_site' @{name='Smoke site';sourcePath=(Join-Path $PSScriptRoot 'sample-site');kind='directory'}
    $siteId = $site.id
    if (!$siteId) { throw 'add_site returned no id' }
    $null = Invoke-Engine 'start_site' @{id=$siteId}
    $url = Invoke-Engine 'preview_url' @{id=$siteId}
    $uri = [uri]$url
    if ($uri.Scheme -ne 'http' -or $uri.Host -ne '127.0.0.1') { throw "Unexpected preview URL: $url" }
    $body = & curl.exe --fail --silent --show-error --max-time 10 --retry 5 --retry-delay 1 --retry-connrefused $url
    if ($LASTEXITCODE -ne 0 -or ($body -join "`n") -notmatch 'hostify-smoke-ok') { throw 'Local preview did not serve sample content' }
    $stopped = Invoke-Engine 'stop_site' @{id=$siteId}
    if ($stopped.status -ne 'stopped') { throw 'stop_site did not return stopped status' }
    & curl.exe --silent --output NUL --max-time 3 $url
    if ($LASTEXITCODE -eq 0) { throw 'Preview still responds after stop_site' }
    $null = Invoke-Engine 'remove_site' @{id=$siteId;deleteFiles=$true}
    if (@(Invoke-Engine 'list_sites' | Where-Object { $_.id -eq $siteId }).Count -ne 0) { throw 'Removed site is still listed' }
    $siteId = $null
    $passed = $true
} catch {
    Write-Host "FAIL: $($_.Exception.Message)" -ForegroundColor Red
} finally {
    if ($siteId -and $writer) {
        try { $null = Invoke-Engine 'stop_site' @{id=$siteId} } catch { $passed=$false; Write-Host "FAIL: cleanup stop_site: $($_.Exception.Message)" }
    }
    if ($writer) { $writer.Dispose() }; if ($reader) { $reader.Dispose() }; if ($pipe) { $pipe.Dispose() }
    # Terminate only the process launched above. Its Job Object closes supervised children.
    if ($engine -and !$engine.HasExited) {
        try { Stop-Process -Id $engine.Id -ErrorAction Stop; $engine.WaitForExit(10000) | Out-Null }
        catch { $passed=$false; Write-Host "FAIL: engine cleanup: $($_.Exception.Message)" }
    }
    Pop-Location
    Write-Host "Smoke artifacts: $dataDir"
}
if ($passed) { Write-Host 'PASS: offline build, engine ping, add/start, real HTTP preview, stop and remove' -ForegroundColor Green; exit 0 }
exit 1

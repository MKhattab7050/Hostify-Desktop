param([Parameter(Mandatory=$true)][string]$EnginePath, [switch]$RemoveService)
$ErrorActionPreference = 'Stop'
$expected = [System.IO.Path]::GetFullPath($EnginePath)
# Stop the matching service through SCM first; killing its process alone leaves
# an automatic-start service registered against an executable being removed.
$service = Get-CimInstance -ClassName Win32_Service -Filter "Name='HostifyEngine'"
if ($service -and $service.PathName -match '^(?:"([^"]+)"|([^ ]+))') {
    $serviceExe = if ($Matches[1]) { $Matches[1] } else { $Matches[2] }
    if ([string]::Equals([System.IO.Path]::GetFullPath($serviceExe), $expected, [StringComparison]::OrdinalIgnoreCase)) {
        if ($service.State -ne 'Stopped') {
            $result = Invoke-CimMethod -InputObject $service -MethodName StopService
            if ($result.ReturnValue -notin @(0,6)) { throw "Cannot stop Hostify service: $($result.ReturnValue)" }
            $deadline = [DateTime]::UtcNow.AddSeconds(30)
            do {
                $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='HostifyEngine'"
                if ($service.State -eq 'Stopped') { break }
                if ([DateTime]::UtcNow -ge $deadline) { throw 'Hostify service did not stop' }
                Start-Sleep -Milliseconds 200
            } while ($true)
        }
        if ($RemoveService) {
            $result = Invoke-CimMethod -InputObject $service -MethodName Delete
            if ($result.ReturnValue -ne 0) { throw "Cannot remove Hostify service: $($result.ReturnValue)" }
        }
    }
}
# Only terminate engines from this installation; their Job Objects close child processes.
# CIM ExecutablePath is used because the NSIS installer is a 32-bit process, and 32-bit
# PowerShell's Get-Process cannot read the path of a 64-bit process (it returns nothing).
$running = @(Get-CimInstance -ClassName Win32_Process -Filter "Name='hostify-engine.exe'" |
    Where-Object { $_.ExecutablePath -and [string]::Equals([System.IO.Path]::GetFullPath($_.ExecutablePath), $expected, [StringComparison]::OrdinalIgnoreCase) })
foreach ($process in $running) {
    try {
        Stop-Process -Id $process.ProcessId -ErrorAction Stop
        Wait-Process -Id $process.ProcessId -Timeout 10 -ErrorAction Stop
    } catch {
        # A fast exit between Stop-Process and Wait-Process is success.
        # Preserve genuine errors when this installation's process still exists.
        $remaining = Get-CimInstance -ClassName Win32_Process -Filter "ProcessId=$($process.ProcessId)" -ErrorAction Stop
        if ($remaining -and $remaining.ExecutablePath -and
            [string]::Equals([System.IO.Path]::GetFullPath($remaining.ExecutablePath), $expected, [StringComparison]::OrdinalIgnoreCase)) { throw }
        if ($remaining -and !$remaining.ExecutablePath) { throw }
    }
}
exit 0

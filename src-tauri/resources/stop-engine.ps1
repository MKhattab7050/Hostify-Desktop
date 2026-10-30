param([Parameter(Mandatory=$true)][string]$EnginePath)
$ErrorActionPreference = 'Stop'
$expected = [System.IO.Path]::GetFullPath($EnginePath)
# Only terminate engines from this installation; their Job Objects close child processes.
$running = @(Get-Process -Name 'hostify-engine' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and [string]::Equals([System.IO.Path]::GetFullPath($_.Path), $expected, [StringComparison]::OrdinalIgnoreCase) })
foreach ($process in $running) {
    Stop-Process -Id $process.Id -ErrorAction Stop
    if (!$process.WaitForExit(10000)) { throw "Engine $($process.Id) did not exit" }
}
exit 0

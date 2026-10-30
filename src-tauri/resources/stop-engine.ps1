param([Parameter(Mandatory=$true)][string]$EnginePath)
$ErrorActionPreference = 'Stop'
$expected = [System.IO.Path]::GetFullPath($EnginePath)
# Only terminate engines from this installation; their Job Objects close child processes.
# CIM ExecutablePath is used because the NSIS installer is a 32-bit process, and 32-bit
# PowerShell's Get-Process cannot read the path of a 64-bit process (it returns nothing).
$running = @(Get-CimInstance -ClassName Win32_Process -Filter "Name='hostify-engine.exe'" |
    Where-Object { $_.ExecutablePath -and [string]::Equals([System.IO.Path]::GetFullPath($_.ExecutablePath), $expected, [StringComparison]::OrdinalIgnoreCase) })
foreach ($process in $running) {
    Stop-Process -Id $process.ProcessId -ErrorAction Stop
    Wait-Process -Id $process.ProcessId -Timeout 10 -ErrorAction Stop
}
exit 0

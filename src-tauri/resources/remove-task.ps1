param([Parameter(Mandatory=$true)][string]$EnginePath)
$ErrorActionPreference = 'Stop'
& $EnginePath uninstall-task
if ($LASTEXITCODE -eq 0) { exit 0 }
# A missing task is already uninstalled, including after a failed install-task.
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$scheduler = New-Object -ComObject 'Schedule.Service'
$scheduler.Connect()
try {
    $null = $scheduler.GetFolder('\').GetTask("HostifyEngine-$sid")
} catch {
    $failure = $_.Exception
    while ($failure) {
        if ($failure.HResult -eq -2147024894) { exit 0 } # ERROR_FILE_NOT_FOUND
        $failure = $failure.InnerException
    }
    throw
}
throw 'The Hostify startup task still exists and could not be removed'

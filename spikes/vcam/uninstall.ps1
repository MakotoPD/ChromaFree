param([switch] $RemoveLogs)

$ErrorActionPreference = 'Stop'

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script from an elevated PowerShell (Run as administrator).'
}

$installDir = Join-Path $env:ProgramFiles 'bgcam-spike'
$logDir = Join-Path $env:ProgramData 'bgcam-spike'
$dll = Join-Path $installDir 'vcam-spike.dll'

if (Test-Path $dll) {
    $process = Start-Process -FilePath regsvr32.exe -ArgumentList '/u', '/s', "`"$dll`"" -Wait -PassThru
    if ($process.ExitCode -ne 0) { Write-Warning "regsvr32 /u exited with code $($process.ExitCode)" }
}

try {
    Remove-Item -Recurse -Force $installDir -ErrorAction Stop
    Write-Host "Removed $installDir"
}
catch {
    Write-Warning "Could not remove $installDir (the DLL is probably still loaded by Frame Server). Run: Restart-Service FrameServer -Force, then this script again."
}

if ($RemoveLogs -and (Test-Path $logDir)) {
    Remove-Item -Recurse -Force $logDir
    Write-Host "Removed $logDir"
}

$ErrorActionPreference = 'Stop'

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script from an elevated PowerShell (Run as administrator).'
}

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$buildDir = Join-Path $repoRoot 'build\vcam-spike\Release'
$installDir = Join-Path $env:ProgramFiles 'chromafree-spike'
$logDir = Join-Path $env:ProgramData 'chromafree-spike'
$dll = Join-Path $installDir 'vcam-spike.dll'

foreach ($file in 'vcam-spike.dll', 'vcam-spike-register.exe') {
    if (-not (Test-Path (Join-Path $buildDir $file))) { throw "Missing $file in $buildDir - build the spike first." }
}

New-Item -ItemType Directory -Force -Path $installDir, $logDir | Out-Null
Copy-Item (Join-Path $buildDir 'vcam-spike.dll'), (Join-Path $buildDir 'vcam-spike-register.exe') $installDir -Force

icacls $logDir /grant '*S-1-5-19:(OI)(CI)M' '*S-1-5-18:(OI)(CI)M' '*S-1-5-4:(OI)(CI)M' | Out-Null
if ($LASTEXITCODE -ne 0) { throw "icacls failed with exit code $LASTEXITCODE" }

$process = Start-Process -FilePath regsvr32.exe -ArgumentList '/s', "`"$dll`"" -Wait -PassThru
if ($process.ExitCode -ne 0) { throw "regsvr32 failed with exit code $($process.ExitCode)" }

Write-Host "Installed to $installDir"
Write-Host "Logs will be written to $logDir"
Write-Host "Registered CLSID {8532B28A-EC5D-4C4C-8322-85C5E431F947} in HKLM"

$ErrorActionPreference = 'Stop'

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script from an elevated PowerShell (Run as administrator).'
}

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
$built = Join-Path $repoRoot 'build\vcam\Release\vcam-source.dll'
$installDir = Join-Path $env:ProgramFiles 'bgcam'
$dll = Join-Path $installDir 'vcam-source.dll'

if (-not (Test-Path $built)) { throw "Missing $built - build vcam-source first." }

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Copy-Item $built $installDir -Force

$process = Start-Process -FilePath regsvr32.exe -ArgumentList '/s', "`"$dll`"" -Wait -PassThru
if ($process.ExitCode -ne 0) { throw "regsvr32 failed with exit code $($process.ExitCode)" }

Write-Host "Installed $dll"
Write-Host "Registered CLSID {4525794B-703E-444B-A81D-2B278B2AD0E4} in HKLM"

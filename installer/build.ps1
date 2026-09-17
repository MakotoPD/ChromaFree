param([switch] $SkipBuild)

$ErrorActionPreference = 'Stop'

$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$package = Join-Path $root 'build\package'
$output = Join-Path $root 'build\installer'
$modelsSource = Join-Path $root 'models'
$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value

function Invoke-Checked([string] $Description, [scriptblock] $Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Description failed with exit code $LASTEXITCODE" }
}

function Enter-DeveloperShell {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $installation = if (Test-Path $vswhere) { & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.CMake.Project -property installationPath } else { $null }
    if (-not $installation) { throw 'Visual Studio Build Tools with the C++ CMake component were not found.' }
    Import-Module (Join-Path $installation 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
    Enter-VsDevShell -VsInstallPath $installation -SkipAutomaticLocation -DevCmdArguments '-arch=x64' | Out-Null
}

function Find-InnoSetup {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
    )
    $found = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    if ($found) { return $found }
    $command = Get-Command iscc.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    throw 'Inno Setup 6 was not found. Install it with: winget install JRSoftware.InnoSetup'
}

if (-not $SkipBuild) {
    Push-Location $root
    try {
        Invoke-Checked 'cargo build' { cargo build --release -p chromafree-app }
        Enter-DeveloperShell
        Invoke-Checked 'cmake configure' { cmake -S vcam-source -B build/vcam -A x64 }
        Invoke-Checked 'cmake build' { cmake --build build/vcam --config Release }
    }
    finally {
        Pop-Location
    }
}

$models = @(Get-ChildItem $modelsSource -Filter 'rvm_mobilenetv3_fp16_*_static.onnx') +
    @(Get-ChildItem $modelsSource -Filter 'selfie_segmenter*.onnx')
if (-not ($models | Where-Object Name -eq 'rvm_mobilenetv3_fp16_1280x720_static.onnx') -or -not ($models | Where-Object Name -eq 'selfie_segmenter.onnx')) {
    throw 'Models are missing. Run: powershell -ExecutionPolicy Bypass -File models/download.ps1'
}

if (Test-Path $package) { Remove-Item -Recurse -Force $package }
New-Item -ItemType Directory -Force -Path (Join-Path $package 'models'), $output | Out-Null

$files = @{
    'chromafree.exe'      = Join-Path $root 'target\release\chromafree.exe'
    'DirectML.dll'        = Join-Path $root 'target\release\DirectML.dll'
    'vcam-source.dll'     = Join-Path $root 'build\vcam\Release\vcam-source.dll'
    'LICENSE'             = Join-Path $root 'LICENSE'
    'THIRD_PARTY.md'      = Join-Path $root 'THIRD_PARTY.md'
    'VCAM_THIRD_PARTY.md' = Join-Path $root 'vcam-source\THIRD_PARTY.md'
    'offline.png'         = Join-Path $root 'vcam-source\assets\offline.png'
}
foreach ($entry in $files.GetEnumerator()) {
    if (-not (Test-Path $entry.Value)) { throw "Missing $($entry.Value)" }
    Copy-Item $entry.Value (Join-Path $package $entry.Key)
}
$models | Copy-Item -Destination (Join-Path $package 'models')

$size = (Get-ChildItem $package -Recurse | Measure-Object -Property Length -Sum).Sum / 1MB
Write-Host ("Staged ChromaFree {0} in {1} ({2:N0} MB, {3} models)" -f $version, $package, $size, $models.Count)

$iscc = Find-InnoSetup
$env:CHROMAFREE_VERSION = $version
$env:CHROMAFREE_PACKAGE_DIR = $package
$env:CHROMAFREE_INSTALLER_DIR = $output
Invoke-Checked 'Inno Setup' { & $iscc (Join-Path $PSScriptRoot 'chromafree.iss') }
Write-Host "Installer: $(Join-Path $output "ChromaFree-Setup-$version.exe")"

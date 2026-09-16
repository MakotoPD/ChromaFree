$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Set-Location $PSScriptRoot

$expected = @{}
foreach ($line in Get-Content sha256.txt) {
    $hash, $name = $line -split '\s+', 2
    $expected[$name] = $hash
}

function Test-ModelHash([string] $name) {
    (Test-Path $name) -and ((Get-FileHash $name -Algorithm SHA256).Hash.ToLower() -eq $expected[$name])
}

foreach ($line in Get-Content sources.txt) {
    $name, $url = $line -split '\s+', 2
    if (Test-ModelHash $name) { Write-Host "ok        $name"; continue }
    Write-Host "download  $name"
    Invoke-WebRequest -Uri $url -OutFile $name
    if (-not (Test-ModelHash $name)) {
        Remove-Item $name
        throw "SHA-256 mismatch for $name"
    }
}

$converted = 'selfie_segmenter.onnx', 'selfie_segmenter_landscape.onnx'
if (($converted | Where-Object { -not (Test-Path $_) }).Count -gt 0) {
    if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
        throw 'MediaPipe models require conversion; install uv (https://docs.astral.sh/uv/)'
    }
    Write-Host 'convert   MediaPipe tflite -> onnx'
    $ErrorActionPreference = 'Continue'
    uv run --no-project --python 3.11 `
        --with tensorflow==2.15.1 --with tf2onnx==1.16.1 --with onnx==1.17.0 --with 'numpy<2' --with 'protobuf<4' `
        python convert_mediapipe.py 2>$null
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -ne 0) { throw 'conversion failed' }
    foreach ($name in $converted) {
        if (-not (Test-Path $name)) { throw "conversion did not produce $name" }
        Write-Host "ok        $name"
    }
}

$staticPrecision = 'fp16'
$staticResolutions = @(
    '640x360', '960x540', '1024x576', '1280x720', '1600x900', '1920x1080', '2560x1440',
    '640x400', '960x600', '1280x800', '1440x900', '1680x1050', '1920x1200', '2560x1600',
    '640x480', '800x600', '960x720', '1024x768', '1280x960', '1440x1080', '1600x1200', '1920x1440', '2560x1920'
)
$missing = $staticResolutions | Where-Object { -not (Test-Path "rvm_mobilenetv3_${staticPrecision}_${_}_static.onnx") }
foreach ($resolution in $staticResolutions | Where-Object { $missing -notcontains $_ }) {
    Write-Host "ok        rvm_mobilenetv3_${staticPrecision}_${resolution}_static.onnx"
}
if ($missing.Count -gt 0) {
    if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
        throw 'Static RVM variants require uv (https://docs.astral.sh/uv/)'
    }
    Write-Host "freeze    RVM $staticPrecision $($missing -join ', ')"
    $ErrorActionPreference = 'Continue'
    uv run --no-project --python 3.11 `
        --with onnx==1.17.0 --with onnxsim==0.7.3 --with onnxruntime --with 'numpy<2' `
        python make_rvm_static.py $staticPrecision @missing
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -ne 0) { throw 'freezing static RVM variants failed' }
}

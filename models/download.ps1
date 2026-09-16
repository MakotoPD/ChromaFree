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

$staticVariants = @(
    @{ Precision = 'fp16'; Width = 1280; Height = 720; Ratio = '0.25' },
    @{ Precision = 'fp16'; Width = 640; Height = 360; Ratio = '0.5' },
    @{ Precision = 'fp32'; Width = 1280; Height = 720; Ratio = '0.25' }
)
foreach ($variant in $staticVariants) {
    $name = "rvm_mobilenetv3_$($variant.Precision)_$($variant.Width)x$($variant.Height)_static.onnx"
    if (Test-Path $name) { Write-Host "ok        $name"; continue }
    if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
        throw 'Static RVM variants require uv (https://docs.astral.sh/uv/)'
    }
    Write-Host "freeze    $name"
    $ErrorActionPreference = 'Continue'
    uv run --no-project --python 3.11 `
        --with onnx==1.17.0 --with onnxsim==0.7.3 --with onnxruntime --with 'numpy<2' `
        python make_rvm_static.py $variant.Precision $variant.Width $variant.Height $variant.Ratio 2>$null
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -ne 0) { throw "freezing $name failed" }
}

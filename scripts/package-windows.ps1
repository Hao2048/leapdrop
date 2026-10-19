# 在 Windows PowerShell 中，从已提交的源码构建免安装 x64 版本。
$ErrorActionPreference = "Stop"
$projectRoot = Split-Path $PSScriptRoot -Parent
Set-Location $projectRoot
$version = (Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version
cargo build -p leapdrop --release --target x86_64-pc-windows-msvc --locked
if ($LASTEXITCODE -ne 0) { throw "Windows build failed" }
$artifactDir = Join-Path $projectRoot "target/release-artifacts"
$stage = Join-Path $projectRoot ("target/windows-package-" + [guid]::NewGuid().ToString())
New-Item -ItemType Directory -Force $artifactDir, $stage | Out-Null
try {
Copy-Item target/x86_64-pc-windows-msvc/release/leapdrop.exe $stage -Force
Copy-Item docs/quick-start.txt "$stage/使用说明.txt" -Force
Copy-Item LICENSE "$stage/LICENSE" -Force
Compress-Archive -Path "$stage/*" -DestinationPath "$artifactDir/LeapDrop_${version}_Windows_x64.zip" -Force
} finally {
    Remove-Item $stage -Recurse -Force
}
Write-Output "Package: $artifactDir/LeapDrop_${version}_Windows_x64.zip"

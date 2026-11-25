# Builds dist\windows\studio-record.exe (release).
# Recording: put ffmpeg.exe next to studio-record.exe, or use the in-app "Set up recording" button.
# Call camera: install OBS Studio once (it registers "OBS Virtual Camera"); Studio Record drives it.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")
cargo build --release
New-Item -ItemType Directory -Force -Path dist\windows | Out-Null
Copy-Item target\release\studio-record.exe dist\windows\
Write-Host "Built dist\windows\studio-record.exe"

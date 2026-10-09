# Windows equivalent of prepare-sidecar.sh.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..\..")
cargo build --release -p mist-app-core --bin mist
$triple = ((rustc -vV) | Select-String "^host: ").ToString().Substring(6).Trim()
New-Item -ItemType Directory -Force "apps\desktop\src-tauri\binaries" | Out-Null
Copy-Item "target\release\mist.exe" "apps\desktop\src-tauri\binaries\mist-$triple.exe"
Write-Host "sidecar: apps\desktop\src-tauri\binaries\mist-$triple.exe"

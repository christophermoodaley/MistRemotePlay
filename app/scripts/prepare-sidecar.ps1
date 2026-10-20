# Builds the `mist` CLI and copies it to src-tauri/binaries, where Tauri
# expects its sidecar (needed before `npx tauri dev` or `tauri build`).
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")
cargo build --release -p mist-core --bin mist
New-Item -ItemType Directory -Force "app\src-tauri\binaries" | Out-Null
Copy-Item "target\release\mist.exe" "app\src-tauri\binaries\mist-x86_64-pc-windows-msvc.exe"
Write-Host "sidecar: app\src-tauri\binaries\mist-x86_64-pc-windows-msvc.exe"

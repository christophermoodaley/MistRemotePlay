# Copies the GStreamer MSVC runtime into apps/desktop/src-tauri/gstreamer so
# the Windows installer can ship it (see tauri.windows-installer.conf.json):
#   bin/       core DLLs, gst-plugin-scanner.exe and the VC++ runtime -> next to the app
#   plugins/   GStreamer plugins                                       -> gstreamer-1.0\
#   licenses/  GStreamer's licence texts
# Usage: stage-gstreamer.ps1 -Root "C:\Program Files\gstreamer\1.0\msvc_x86_64"
param([Parameter(Mandatory = $true)][string]$Root)
$ErrorActionPreference = "Stop"
$dest = Join-Path $PSScriptRoot "..\src-tauri\gstreamer"
if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
$bin = New-Item -ItemType Directory -Force (Join-Path $dest "bin")
$plugins = New-Item -ItemType Directory -Force (Join-Path $dest "plugins")
$licenses = New-Item -ItemType Directory -Force (Join-Path $dest "licenses")

Copy-Item (Join-Path $Root "bin\*.dll") $bin
$scanner = Get-ChildItem -Recurse -Filter "gst-plugin-scanner.exe" (Join-Path $Root "libexec") | Select-Object -First 1
if (-not $scanner) { throw "gst-plugin-scanner.exe not found under $Root\libexec" }
Copy-Item $scanner.FullName $bin
Copy-Item (Join-Path $Root "lib\gstreamer-1.0\*.dll") $plugins

# App-local VC++ runtime, so a fresh Windows install needs no redistributable.
foreach ($dll in "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll", "msvcp140_1.dll", "msvcp140_2.dll", "concrt140.dll") {
  $src = Join-Path $env:SystemRoot "System32\$dll"
  if ((Test-Path $src) -and -not (Test-Path (Join-Path $bin $dll))) { Copy-Item $src $bin }
}

$lic = Join-Path $Root "share\licenses"
if (Test-Path $lic) {
  Get-ChildItem -Recurse -File $lic | ForEach-Object {
    $rel = $_.FullName.Substring($lic.Length).TrimStart("\").Replace("\", "_")
    Copy-Item $_.FullName (Join-Path $licenses $rel)
  }
}
"GStreamer is distributed under the LGPL; some plugins (e.g. x264) under the GPL.",
"Sources: https://gstreamer.freedesktop.org/src/ (same version as these binaries)." |
  Set-Content (Join-Path $licenses "README.txt")

$size = (Get-ChildItem -Recurse -File $dest | Measure-Object -Sum Length).Sum / 1MB
"staged GStreamer: {0} DLLs, {1} plugins, {2:N0} MB" -f (Get-ChildItem $bin).Count, (Get-ChildItem $plugins).Count, $size

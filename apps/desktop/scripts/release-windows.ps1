# Builds the Windows installer on this PC and, with -Publish, publishes it as
# a GitHub pre-release that the in-app updater offers (replaces the CI
# installer job, which takes ~25 minutes on GitHub's runners).
#   1. sidecar (mist.exe) + GStreamer staging + `tauri build` (NSIS installer)
#   2. dist\: Mist.Remote.Play_<version>_x64-setup.exe, SHA256SUMS.txt, release notes
#   3. -Publish: `gh release create v<version>` at the pushed HEAD commit
# The version comes from src-tauri/tauri.conf.json; bump it (and Cargo.toml,
# package.json) and push before publishing. The Linux signaling-server archive
# cannot be built here; the newest one already released is carried over.
# Usage: release-windows.ps1 [-Publish] [-Root <GStreamer MSVC x86_64 root>]
param([switch]$Publish, [string]$Root)
$ErrorActionPreference = "Stop"

# Runs a native command whose failure is an answer, not an error (Windows
# PowerShell turns redirected stderr into terminating errors under "Stop").
function Try-Native([scriptblock]$cmd) {
  $ErrorActionPreference = "Continue"
  & $cmd 2>$null
}
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$desktop = Join-Path $repo "apps\desktop"
Set-Location $repo

$version = (Get-Content (Join-Path $desktop "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
$tag = "v$version"
"Mist Remote Play $version"

if ($Publish) {
  # The release must point at a commit GitHub has, with exactly these sources.
  if (git status --porcelain --untracked-files=no) { throw "uncommitted changes; commit them first" }
  git fetch -q origin
  $head = (git rev-parse HEAD).Trim()
  if (-not (git branch -r --contains $head)) { throw "HEAD $($head.Substring(0, 7)) is not pushed; push it first" }
  $existing = Try-Native { gh release view $tag --json tagName }
  if ($existing) { throw "release $tag already exists; bump the version first" }
}

if (-not $Root) {
  $candidates = @(
    [Environment]::GetEnvironmentVariable("GSTREAMER_1_0_ROOT_MSVC_X86_64", "Machine"),
    "C:\Program Files\gstreamer\1.0\msvc_x86_64",
    "C:\gstreamer\1.0\msvc_x86_64"
  )
  $Root = $candidates | Where-Object { $_ -and (Test-Path (Join-Path $_ "lib\pkgconfig")) } | Select-Object -First 1
  if (-not $Root) { throw "GStreamer MSVC x86_64 development files not found (choco install gstreamer gstreamer-devel pkgconfiglite)" }
}
$Root = $Root.TrimEnd("\")
$env:GSTREAMER_1_0_ROOT_MSVC_X86_64 = "$Root\"
$env:PKG_CONFIG_PATH = Join-Path $Root "lib\pkgconfig"
$env:PATH = (Join-Path $Root "bin") + ";" + $env:PATH

cargo build --release -p mist-app-core --bin mist
if ($LASTEXITCODE -ne 0) { throw "building mist.exe failed" }
New-Item -ItemType Directory -Force (Join-Path $desktop "src-tauri\binaries") | Out-Null
Copy-Item target\release\mist.exe (Join-Path $desktop "src-tauri\binaries\mist-x86_64-pc-windows-msvc.exe")
& (Join-Path $PSScriptRoot "stage-gstreamer.ps1") -Root $Root

Push-Location $desktop
try {
  npm ci --no-audit --no-fund
  if ($LASTEXITCODE -ne 0) { throw "npm ci failed" }
  npx tauri build --config src-tauri/tauri.windows-installer.conf.json
  if ($LASTEXITCODE -ne 0) { throw "tauri build failed" }
} finally { Pop-Location }

$installer = Get-Item (Join-Path $repo "target\release\bundle\nsis\Mist Remote Play_${version}_x64-setup.exe")
$dist = Join-Path $repo "target\release\dist"
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory $dist | Out-Null
# GitHub replaces spaces in asset names with dots; name it that way up front.
$asset = Join-Path $dist $installer.Name.Replace(" ", ".")
Copy-Item $installer.FullName $asset
if ($Publish) {
  $previous = gh release list --limit 10 --json tagName -q ".[].tagName" | Where-Object { $_ -ne $tag }
  foreach ($t in $previous) {
    Try-Native { gh release download $t --pattern "mist-signaling-server-linux-x86_64.tar.gz" --dir $dist } | Out-Null
    if (Test-Path (Join-Path $dist "mist-signaling-server-linux-x86_64.tar.gz")) { "signaling server archive carried over from $t"; break }
  }
}
Get-ChildItem $dist -File | ForEach-Object { "{0}  {1}" -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower(), $_.Name } |
  Set-Content (Join-Path $dist "SHA256SUMS.txt") -Encoding ascii
# Explicit UTF-8 (no BOM): Windows PowerShell would read the template as ANSI.
$utf8 = New-Object System.Text.UTF8Encoding $false
$notes = [IO.File]::ReadAllText((Join-Path $repo ".github\release-notes.md"), $utf8)
[IO.File]::WriteAllText((Join-Path $dist "release-notes.md"), $notes.Replace("{{VERSION}}", $version).Replace("{{TAG}}", $tag), $utf8)
Get-Content (Join-Path $dist "SHA256SUMS.txt")

if (-not $Publish) {
  "built $asset ({0:N0} MB); run with -Publish to release it" -f ((Get-Item $asset).Length / 1MB)
  return
}
$files = Get-ChildItem $dist -File | Where-Object Name -ne "release-notes.md" | ForEach-Object FullName
gh release create $tag @files --target $head --prerelease --title "Mist Remote Play $tag" --notes-file (Join-Path $dist "release-notes.md")
if ($LASTEXITCODE -ne 0) { throw "gh release create failed" }
"published $tag"

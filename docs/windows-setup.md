# Windows setup

> **To just use Mist, install it:** see [install-windows.md](install-windows.md).
> The installer bundles everything (GStreamer, codecs, the VC++ runtime)
> and needs no compiling. This page covers details and building from source.

**Status.** The installer is built on GitHub's Windows runners, installed
silently there, and the installed copy is checked with the system GStreamer
hidden: it must find working encoders/decoders and stream a test pattern
end to end. Real screen capture, audio loopback, SendInput, ViGEm and the
streaming window have not yet been run on a physical Windows PC; treat
Windows as **experimental** until the compatibility matrix says otherwise.

## Building from source

Requirements:

* **GStreamer 1.22+ MSVC x86_64** — the *runtime* and *development*
  installers from <https://gstreamer.freedesktop.org/download/> with the
  **Complete** feature set (or `choco install gstreamer gstreamer-devel`).
* Environment:
  ```powershell
  $env:GSTREAMER_1_0_ROOT_MSVC_X86_64 = "C:\Program Files\gstreamer\1.0\msvc_x86_64\"  # older installers: C:\gstreamer\1.0\msvc_x86_64\
  $env:PKG_CONFIG_PATH = "$env:GSTREAMER_1_0_ROOT_MSVC_X86_64\lib\pkgconfig"
  $env:PATH = "$env:GSTREAMER_1_0_ROOT_MSVC_X86_64\bin;$env:PATH"
  ```
  plus `pkg-config` (e.g. `choco install pkgconfiglite`).
* Rust 1.96+ (stable, MSVC), Visual Studio Build Tools, Node.js 20+.

Build the installer exactly as CI does (`.github/workflows/release.yml`):

```powershell
cargo build --release -p mist-app-core --bin mist
New-Item -ItemType Directory -Force apps\desktop\src-tauri\binaries
Copy-Item target\release\mist.exe apps\desktop\src-tauri\binaries\mist-x86_64-pc-windows-msvc.exe
apps\desktop\scripts\stage-gstreamer.ps1 -Root $env:GSTREAMER_1_0_ROOT_MSVC_X86_64.TrimEnd("\")
cd apps\desktop; npm ci
npx tauri build --config src-tauri/tauri.windows-installer.conf.json
# -> target\release\bundle\nsis\Mist Remote Play_<version>_x64-setup.exe
```

`apps\desktop\scripts\release-windows.ps1` runs these steps; with
`-Publish` it also publishes the installer as a GitHub pre-release for the
in-app updater (bump and push the version first).

`stage-gstreamer.ps1` copies the GStreamer DLLs, `gst-plugin-scanner.exe`
and the VC++ runtime next to the app and the plugins into
`gstreamer-1.0\`. At startup Mist sees that folder and uses only the bundled
runtime (its plugins, scanner and a private registry). A plain
`npm run tauri build` without the extra config builds an installer that
expects GStreamer on `PATH`.

The installer is per-machine (administrator prompt) and adds inbound
firewall rules for the app and the streaming client.

## Capture and encoding

* Capture: `d3d11screencapturesrc` (DXGI Desktop Duplication) selected by
  monitor handle. With a D3D11-capable encoder (NVENC D3D11, QSV, AMF, Media
  Foundation) frames stay in GPU memory; otherwise they are downloaded and
  converted on the CPU.
* Encoder order: NVENC → NVENC (D3D11) → QSV → AMF → Media Foundation →
  x264 → OpenH264, each verified with a test encode.
* Audio: `wasapi2src loopback=true` on the default render device.

## Input and integrity levels

* Keyboard and mouse use `SendInput` with scan codes (layout independent).
* **UIPI**: a normal-user Mist host cannot inject into elevated windows
  (Task Manager, installers, UAC prompts) or the secure desktop. This is a
  Windows security boundary; we recommend not running the host elevated.
  SendInput failures are reported in the host statistics as backend errors.
* Windows does not autorepeat injected keys, so the client's autorepeat
  events are forwarded on Windows hosts.

## Virtual controllers

Install **ViGEmBus** (<https://github.com/nefarius/ViGEmBus/releases>) once as
an administrator. Mist then creates virtual Xbox 360 controllers. XInput
limits triggers to 8 bits, so trigger precision is reduced on Windows hosts.
Rumble that games send to the virtual controller (large/small motor) is
forwarded to the player's controller (compiled, not run).
Without ViGEmBus, controllers are reported as unavailable and keyboard/mouse
still work. Note: the ViGEmBus project has been retired by its author; it
still installs and works on current Windows versions, but a long-term
replacement may be needed.

## Streaming window

Windows clients use the software renderer (CPU scaling and colour
conversion). The GPU renderer used on X11 is not enabled on Windows: the
Direct3D 11 sink draws through a child window that would capture mouse
input. For the lowest latency, keep the stream resolution equal to the
window (or fullscreen) size so no scaling is needed.

## Firewall

The installer adds inbound rules for Mist (all network profiles) so other
PCs can reach the host's built-in server (TCP 47900) and the WebRTC media
ports. Every session is still authenticated with paired keys and needs the
host's approval. Hosting over the internet also needs the TCP port
forwarded on the router (see install-windows.md).

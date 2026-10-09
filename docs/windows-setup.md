# Windows setup

> **Status: compiles, not yet run on Windows.** All Windows-specific code
> (DXGI monitor enumeration, `d3d11screencapturesrc` capture, WASAPI
> loopback, SendInput, ViGEm gamepads, DPAPI secret storage) type-checks with
> `cargo check --target x86_64-pc-windows-gnu`, but no Windows machine was
> available to run it. Treat Windows as **experimental** until the
> compatibility matrix says otherwise.

## Requirements

* Windows 10 21H2+ or Windows 11, 64-bit.
* **GStreamer 1.22+ MSVC x86_64** — install both the *runtime* and
  *development* installers from <https://gstreamer.freedesktop.org/download/>
  with the **Complete** feature set (it includes `d3d11`, `wasapi2`,
  `webrtc`, `nice`, `x264`, `openh264`, `libav`, `mediafoundation`, `nvcodec`,
  `qsv`, `amfcodec`).
* Environment for building:
  ```powershell
  $env:GSTREAMER_1_0_ROOT_MSVC_X86_64 = "C:\gstreamer\1.0\msvc_x86_64\"
  $env:PKG_CONFIG_PATH = "$env:GSTREAMER_1_0_ROOT_MSVC_X86_64\lib\pkgconfig"
  $env:PATH = "$env:GSTREAMER_1_0_ROOT_MSVC_X86_64\bin;$env:PATH"
  ```
  plus `pkg-config` (e.g. `choco install pkgconfiglite`).
* Rust 1.96 or newer (stable, MSVC toolchain), Visual Studio Build Tools, Node.js 20+,
  WebView2 runtime (preinstalled on Windows 11).

## Build

```powershell
cargo build --release -p mist-app-core -p mist-signaling-server
cd apps\desktop; npm install
.\scripts\prepare-sidecar.ps1
npm run tauri build      # MSI and NSIS installers in src-tauri\target\release\bundle
```

The installer does not bundle GStreamer; install the GStreamer runtime first
(or set `PATH` to it). Bundling GStreamer is a TASKS.md item.

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
Without ViGEmBus, controllers are reported as unavailable and keyboard/mouse
still work. Note: the ViGEmBus project has been retired by its author; it
still installs and works on current Windows versions, but a long-term
replacement may be needed.

## Firewall

Allow `mist.exe` (and `mist-desktop.exe`) on private networks for direct LAN
connections. Over the internet no inbound rules are needed (ICE + TURN).

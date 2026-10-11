# Compatibility matrix

Legend: **Verified** = run end to end in this project's tests ·
**Builds** = compiles/type-checks for the target, never run ·
**Implemented** = code exists and is unit-tested where possible, but the
platform feature was not exercised · **Unsupported** = not available.

Everything not marked *Verified* should be considered **experimental**.

## Host → client combinations

| Host \ Client | Linux client | Windows client |
|---|---|---|
| **Linux host (X11)** | **Verified** (processes, network namespaces, TLS, TURN, desktop app) | Builds — experimental |
| **Linux host (Wayland)** | Implemented — experimental | Implemented — experimental |
| **Windows host** | Builds — experimental | Builds — experimental |

The wire protocol is identical on both platforms (H.264 + Opus over WebRTC,
JSON control messages), so cross-OS sessions need no special handling; they
are untested only because no Windows machine was available.

## Components

| Component | Linux | Windows |
|---|---|---|
| Screen capture | X11 `ximagesrc`: **Verified**. Wayland portal + PipeWire: Implemented | DXGI `d3d11screencapturesrc`: Builds |
| System audio | PulseAudio monitor: **Verified** (PulseAudio 16). PipeWire-pulse: Implemented | WASAPI loopback: Builds |
| H.264 software encode | x264: **Verified**; OpenH264: probe-verified | Builds |
| H.264 hardware encode | NVENC, VA, VA-API: Implemented (runtime-probed) | NVENC, QSV, AMF, Media Foundation: Implemented |
| H.264 decode | FFmpeg: **Verified**; OpenH264: probe-verified; VA/NVDEC: Implemented | D3D11/D3D12/NVDEC/FFmpeg: Builds |
| Streaming window, software renderer | winit X11: **Verified** (Xvfb). Wayland: Implemented | Builds |
| Streaming window, GPU renderer (`glimagesink` embedded in the window) | X11: **Verified** functionally with software OpenGL (frames, stats overlay, keyboard/mouse pass-through); not benchmarked on a real GPU. Wayland: not offered (software renderer) | Not offered (the D3D11 sink's child window would capture mouse input); software renderer |
| Keyboard/mouse injection | XTEST: **Verified**. uinput: Implemented | SendInput: Builds |
| Controller input path (client → host) | **Verified** across processes with a scripted controller and the mock host backend (`e2e.py --gamepad-check`, also run in CI) | same code; Builds |
| Virtual gamepads | uinput: Implemented | ViGEmBus: Builds |
| Rumble (game → virtual pad → client controller) | Protocol and forwarding **Verified** across processes (mock backend echoes triggers as rumble). uinput force-feedback handling: Implemented. Client rumble via gilrs: Implemented | ViGEm notifications: Builds |
| Physical controllers (client) | gilrs/evdev: Implemented (no controller attached during tests) | gilrs/WGI: Builds |
| Desktop app | WebKitGTK 2.52: **Verified** (Xvfb) | WebView2: Builds |
| Secret storage | 0600 files: **Verified** (unit test) | DPAPI: Builds |
| Signaling server | **Verified** | Builds |
| Packaging | `.deb`: **Verified** (built, installed, launches). AppImage: not built (needs network access to linuxdeploy) | MSI/NSIS: not built |

## GPU configurations

| GPU / API | Status |
|---|---|
| None (software) | **Verified** |
| NVIDIA NVENC/NVDEC | Implemented, untested |
| Intel/AMD VA-API (`va`, `vaapi`) | Implemented, untested |
| Intel Quick Sync (Windows) | Implemented, untested |
| AMD AMF (Windows) | Implemented, untested |
| Media Foundation (Windows) | Implemented, untested |

## Display servers

| Session | Capture | Input |
|---|---|---|
| Xorg / Xvfb | **Verified** | XTEST **Verified** |
| GNOME Wayland | Implemented (portal) | uinput (implemented) |
| KDE Plasma Wayland | Implemented (portal) | uinput (implemented) |
| wlroots (Sway, Hyprland) | Implemented (portal-wlr / hyprland) | uinput (implemented) |
| XWayland only | not offered (would capture XWayland windows only) | – |

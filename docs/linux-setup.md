# Linux setup

Tested on Ubuntu 24.04 (GStreamer 1.24.2). Debian 12+ and Fedora 39+ should
work with their package names; GStreamer **1.22 or newer** is required.

## Runtime packages

Ubuntu / Debian:

```bash
sudo apt install gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav \
  gstreamer1.0-nice gstreamer1.0-x gstreamer1.0-pulseaudio gstreamer1.0-pipewire
# Hardware encoding/decoding (Intel/AMD): gstreamer1.0-vaapi or the `va` plugin in plugins-bad + mesa-va-drivers / intel-media-va-driver
```

Fedora: `gstreamer1-plugins-{base,good,bad-free,ugly-free}`, `gstreamer1-libav`
(RPM Fusion), `libnice-gstreamer1`, `pipewire-gstreamer`. x264 comes from
RPM Fusion (`gstreamer1-plugins-ugly`); without it OpenH264 is used.

Check what Mist can use: `mist diagnose`.

## Build dependencies

```bash
sudo apt install build-essential pkg-config libssl-dev libudev-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libgstreamer-plugins-bad1.0-dev
# desktop app:
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev
```

Rust 1.96 or newer (`rustup`, stable) and Node.js 20+ (desktop app).

## Screen capture

Mist detects the session type at runtime:

* **X11** (`DISPLAY` set, no `WAYLAND_DISPLAY`): monitors from RandR, capture
  with `ximagesrc`. No setup needed. *Tested.*
* **Wayland**: capture goes through **xdg-desktop-portal** ScreenCast and
  PipeWire (`gstreamer1.0-pipewire`). The desktop shows its own dialog to pick
  the screen when a session starts; with portal v4+ the choice is remembered
  (restore token) where the compositor supports it. Requires a portal backend
  (`xdg-desktop-portal-gnome`, `-kde`, `-wlr`, `-hyprland`). *Implemented,
  not yet run* (no Wayland compositor in the test environment).
  X11 capture under XWayland is deliberately not offered because it only sees
  XWayland windows.

## System audio

Mist captures the **monitor of the default output** (`@DEFAULT_MONITOR@`)
through PulseAudio or PipeWire's PulseAudio server (`pipewire-pulse`). It
never opens the microphone. Specific outputs can be chosen in Settings →
Audio. If the device disappears the host keeps streaming video and retries
audio with backoff. *Tested with PulseAudio 16.*

## Input injection

| Backend | When | Permissions |
|---|---|---|
| XTEST | X11 sessions (default there) | none — *tested* |
| uinput | Wayland, consoles, and all virtual gamepads | write access to `/dev/uinput` — *implemented, not run* |

Grant uinput access to a dedicated group instead of running as root:

```bash
sudo groupadd --system mist-input
sudo usermod -aG mist-input "$USER"          # log out and in again
sudo cp deploy/linux/99-mist-uinput.rules /etc/udev/rules.d/
echo uinput | sudo tee /etc/modules-load.d/mist-uinput.conf
sudo modprobe uinput && sudo udevadm control --reload && sudo udevadm trigger
```

Security note: any program run by a member of that group can create input
devices. On X11 hosts you can skip this and use XTEST (keyboard/mouse only).

Virtual gamepads are uinput devices using the Linux gamepad conventions and
the USB IDs of a common wired Xbox-style controller, so SDL and Steam apply
their standard mapping. Compatibility with every game is **not** verified.

uinput absolute pointer positioning maps across the compositor's whole
output layout; on multi-monitor Wayland hosts use relative mouse mode
(Ctrl+Alt+Shift+M in the client) for accurate pointing.

## Controllers on the client

Physical controllers are read with gilrs (evdev). Your user needs read
access to `/dev/input/event*` for gamepads, which desktop distributions grant
through `uaccess` for the logged-in user.

## Hardware encoding

Mist tries, in order: NVENC (`nvh264enc`), VA (`vah264lpenc`, `vah264enc`),
legacy VA-API (`vaapih264enc`), then x264 and OpenH264. Each is checked with a
short real encode at startup; failures (missing driver, unsupported GPU) are
shown in Settings → Diagnostics and the next candidate is used. Hardware
paths are implemented but **not yet run** (no GPU in the development VM).

## Running headless hosts

A host needs a graphical session to capture. For tests, `Xvfb :99` plus
`DISPLAY=:99` works (this is how the project's end-to-end tests run).

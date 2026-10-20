# Mist Remote Play

Stream games between Windows and Linux PCs (including the Raspberry Pi 4 and
5), on your home network or over the internet. The **host** shares its screen, sound and controllers; up to 8
people can join, each either playing or watching. Connections are encrypted
end to end (WebRTC) and every device has to be paired and approved by the host.

## Use it

1. Download `Mist.Remote.Play_<version>_x64-setup.exe` from
   [Releases](https://github.com/christophermoodaley/MistRemotePlay/releases)
   and install it on both PCs. If Windows says *"Windows protected your PC"*,
   click **More info → Run anyway** (the installer isn't code-signed).
2. **Host PC:** choose **Share this PC**, name it and click **Start sharing**.
   A pairing code appears.
3. **Other PC:** choose **Play on this PC**, pick the host (same network) or
   type its address (internet), and enter the code. Approve the request on the
   host.
4. Next time, just click **Play** on the Home page.

**Over the internet:** Mist opens TCP port 47900 on the host's router
automatically (UPnP) and shows the address to share on Host this PC. If the
router doesn't allow that, forward that port to the host by hand.

**In a voice call with the people you stream to?** Settings → Sound → *System
audio source* → *Everything except one app* and pick Discord, so they don't hear
themselves. *Only chosen apps* streams just the game (or any apps you add).
Both need Windows 11.

**Controllers:** install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases)
on the host so friends' controllers show up as Xbox pads.

**Streaming window shortcuts:** hold `Ctrl` `Alt` `Shift` and press **Q**
disconnect · **F** fullscreen · **M** grab mouse · **S** stats · **A** mute.

The app updates itself on Windows (Settings → Updates).

## Linux and Raspberry Pi

Runs on 64-bit Linux with GStreamer 1.22 or newer: Raspberry Pi OS
(Bookworm or Trixie, 64-bit), Debian 12+, Ubuntu 24.04. There is no
prebuilt package yet; build the `.deb` on the machine itself (see Build) and
install it with `sudo apt install ./target/release/bundle/deb/*.deb`.

**Raspberry Pi** works best as the PC you play on. Stream at 1080p
(Settings → Stream):

* **Pi 4** decodes H.264 in hardware (V4L2) up to 1080p. Above that, Mist
  switches to software decoding, which is too slow there.
* **Pi 5** has no H.264 hardware decoder; its CPU decodes 1080p60 in
  software.
* Video is drawn with OpenGL (Settings → Stream → Renderer). On Raspberry Pi
  OS's Wayland desktop the streaming window opens through XWayland for this;
  choose *Software* to keep it a native Wayland window.
* Settings → Diagnostics (or `mist diagnose`) shows the decoder and renderer
  in use.

Hosting from Linux captures the screen through X11 or, on Wayland, the
system's screen-sharing dialog (xdg-desktop-portal + PipeWire), and system
sound through PulseAudio/PipeWire. A Pi can host, but only with software
encoding at low resolutions.

**Keyboard, mouse and controllers on a Linux host:** on X11 keyboard and
mouse work out of the box (XTEST). On Wayland, and for friends' controllers,
the host needs access to `/dev/uinput`:

```sh
sudo groupadd --system mist-input
sudo usermod -aG mist-input "$USER"          # log out and back in
sudo cp app/linux/99-mist-uinput.rules /etc/udev/rules.d/
echo uinput | sudo tee /etc/modules-load.d/mist-uinput.conf
sudo modprobe uinput && sudo udevadm control --reload && sudo udevadm trigger
```

Any program run by a member of that group can create input devices.

## Project layout

```
app/          desktop app: React UI (src/) and Tauri shell (src-tauri/)
  scripts/    build and release scripts
  linux/      udev rule for uinput (Linux hosts)
core/         Rust engine: capture, encoding, WebRTC, input, controllers,
              pairing, signaling server, host and client services
  src/bin/    `mist` CLI (the streaming window) and `mist-signaling` server
  tests/      integration tests
```

## Build

### Windows

Needs Rust (stable, MSVC), Visual Studio Build Tools (C++), Node.js 20+, and
GStreamer 1.22+ MSVC x86_64 runtime and development files
(`choco install gstreamer gstreamer-devel pkgconfiglite`).

```powershell
# Installer -> target\release\dist\
app\scripts\release-windows.ps1

# Publish it as a GitHub release (bump the version in Cargo.toml,
# app\package.json, app\package-lock.json and app\src-tauri\tauri.conf.json,
# commit and push first)
app\scripts\release-windows.ps1 -Publish
```

For development: run `app\scripts\prepare-sidecar.ps1` once, then
`npm ci` and `npx tauri dev` in `app\`. Checks: `cargo clippy --all-targets`,
`cargo test -p mist-core --lib`, and `npx tsc --noEmit` in `app\`.

### Linux and Raspberry Pi

Debian / Raspberry Pi OS packages:

```sh
sudo apt install build-essential pkg-config libudev-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libgstreamer-plugins-bad1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly gstreamer1.0-libav gstreamer1.0-nice gstreamer1.0-gl \
  gstreamer1.0-x gstreamer1.0-pulseaudio gstreamer1.0-pipewire \
  libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
  librsvg2-dev libayatana-appindicator3-dev libxdo-dev
```

plus Rust (rustup, stable) and Node.js 20+ (Bookworm's own `nodejs` is too
old; use NodeSource or nvm). Then:

```sh
app/scripts/build-linux.sh      # -> target/release/bundle/deb/*.deb
```

On a Pi, a first build takes a long while and wants 4 GB of RAM; with 2 GB,
run it as `CARGO_BUILD_JOBS=1 app/scripts/build-linux.sh`. To add the
package to a release: `gh release upload v<version> target/release/bundle/deb/*.deb`.

## License

MIT or Apache-2.0, at your option. The Windows installer bundles GStreamer
(LGPL) and its codecs (the Linux package uses the system's); H.264 may be subject to patent licensing in some countries.

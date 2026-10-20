# Mist Remote Play

Stream games from one Windows PC to another, on your home network or over the
internet. The **host** shares its screen, sound and controllers; up to 8
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

The app updates itself (Settings → Updates).

## Project layout

```
app/          desktop app: React UI (src/) and Tauri shell (src-tauri/)
  scripts/    build and release scripts
core/         Rust engine: capture, encoding, WebRTC, input, controllers,
              pairing, signaling server, host and client services
  src/bin/    `mist` CLI (the streaming window) and `mist-signaling` server
  tests/      integration tests
```

## Build

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

## License

MIT or Apache-2.0, at your option. The installer bundles GStreamer (LGPL)
and its codecs; H.264 may be subject to patent licensing in some countries.

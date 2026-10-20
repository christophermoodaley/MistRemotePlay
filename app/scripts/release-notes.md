Stream games from one PC to another — on your home network or over the internet.

> **Pre-release.** Built on a Windows gaming PC (RTX 4090), where hosting streams the real screen at 1080p60 (Media Foundation and NVENC). Several people at once has been tested within one PC (automated tests), not yet across several PCs, and streaming over the internet has not been confirmed yet. Please report what works and what doesn't.

## What's new in {{VERSION}}

- **Keep Discord out of the stream.** Friends in a voice call with the host heard themselves, because the host streamed all of its sound. **Settings → Audio → System audio source** now offers **Everything except one app** (Discord is filled in) and **Only chosen apps** (for example just the game). It follows the app if it starts, quits or restarts while you host. Needs Windows 11.
- **NVENC works again, and Media Foundation is the default encoder.** Hosting on NVIDIA PCs could end with *"Could not encode stream (nvh264enc…)"*. The cause: once Mist had tried the Media Foundation encoder after NVENC, NVENC stopped working until Mist restarted, and Mist's startup check did exactly that. **Automatic** now uses Media Foundation (it works on every GPU and still encodes on the graphics card), and NVENC can be chosen under **Settings → Host → Video encoder**. If you switch to NVENC after streaming with Media Foundation, restart Mist first; otherwise Mist keeps Media Foundation and tells you why.
- Linux support is back in the source (including the Raspberry Pi 4/5 as the PC you play on), built from source for now; see the README. The Windows installer is unchanged by it.
- PCs on 0.1.3 or newer can still connect to each other; nothing changes in how they talk.

**Also new since 0.1.6:** your public address and a **Test connection** button under **Host → How others connect**.

**Coming from 0.1.1 or 0.1.2:** their updater can't start the installer, so install this version by hand once (download below). From 0.1.3 on, updates install in-app.

## Install (both PCs)

1. Download **`Mist.Remote.Play_{{VERSION}}_x64-setup.exe`** below and run it.
2. Windows may say *"Windows protected your PC"* (the installer isn't code-signed yet): click **More info → Run anyway**, then approve the administrator prompt.

Everything is included — no GStreamer, codecs, runtimes or server to install separately.

## Updating

**From 0.1.1 on, Mist updates itself:** Settings → **Updates** shows new versions and installs them with one click (it checks at startup). Because the repository is private, Mist needs a read-only GitHub token once — the Updates tab explains how to create one (Contents: read-only, this repository only).

Coming from **0.1.0**? That version has no updater: install this one over it once (your pairings and settings are kept).

## First connection

**On the PC with the games (host):** open Mist → **Host** → **Start hosting** → **Create pairing code**.

**On the PC you play on:** open Mist → **Connect**:
- *Same network:* **Scan the network** → **Pair** next to the host.
- *Over the internet:* type the address shown on the host under **Host → How others connect** (Mist opens TCP port 47900 on the router by UPnP; otherwise forward it by hand).

Enter the pairing code, compare the fingerprint with the one shown on the host (Host → How others connect), then approve on the host. After that the host appears on **Home** — click **Connect**.

In the stream window, hold **Ctrl + Alt + Shift** and press **Q** to disconnect, **F** for fullscreen, **M** to capture the mouse, **S** for statistics.

**Controllers:** controllers on the player PC are forwarded automatically. On the host, install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) once so games see them.

Full guide and troubleshooting: [README](https://github.com/christophermoodaley/MistRemotePlay/blob/{{TAG}}/README.md)

## Files

| File | For |
|---|---|
| `Mist.Remote.Play_{{VERSION}}_x64-setup.exe` | Windows 10/11 64-bit installer (host and player) |
| `SHA256SUMS.txt` | Checksums of the files above |

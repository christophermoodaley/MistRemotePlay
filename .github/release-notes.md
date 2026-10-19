Stream games from one Windows PC to another — on your home network or over the internet.

> **Pre-release.** Built on a Windows gaming PC (RTX 4090), where hosting streams the real screen at 1080p60 with the Media Foundation encoder. Several people at once has been tested within one PC (automated tests), not yet across several PCs, and streaming over the internet has not been confirmed yet. Please report what works and what doesn't.

## What's new in {{VERSION}}

- **Up to 8 people at once.** Everyone sees the host's screen. For each person the host chooses **Play** (their own controller) or **Watch** (video and audio only), when they ask to join and at any time under **Host → Connected people**. Keyboard and mouse go to the first player; the host can hand them to anyone else. The limit is under **Settings → Host → People at once**.
  - Everyone gets the same picture quality, set by the weakest connection.
  - The host uploads one copy of the stream per person: 4 people at 20 Mbit/s need about 80 Mbit/s of upload.
  - Windows games see at most 4 controllers, across all players.
  - Only the host needs this update; people on 0.1.3 and 0.1.4 can join as players or watchers.
- **Fixed (0.1.4):** streaming over the internet stayed at *"negotiating"* and then ended with *"ICE/DTLS connection failed"*.
- **Known issue:** on some NVIDIA PCs hosting ends with *"Could not encode stream (nvh264enc…)"*. Choose **Settings → Host → Video encoder → Media Foundation (hardware)**; it still encodes on the NVIDIA GPU.
- **Fixed (0.1.3):** installing an update from Settings → Updates failed with *"The requested operation requires elevation (os error 740)"*.
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
- *Over the internet:* type the host's public address (the host must forward **TCP port 47900** on its router to the host PC).

Enter the pairing code, compare the fingerprint with the one shown on the host (Host → How others connect), then approve on the host. After that the host appears on **Home** — click **Connect**.

In the stream window, hold **Ctrl + Alt + Shift** and press **Q** to disconnect, **F** for fullscreen, **M** to capture the mouse, **S** for statistics.

**Controllers:** controllers on the player PC are forwarded automatically. On the host, install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) once so games see them.

Full guide and troubleshooting: [docs/install-windows.md](https://github.com/christophermoodaley/MistRemotePlay/blob/{{TAG}}/docs/install-windows.md)

## If the stream never starts over the internet

Some networks (mobile, some ISPs, corporate) don't allow a direct connection between the PCs. Then you need a relay server on a machine with a public address (e.g. a small VPS). `mist-signaling-server-linux-x86_64.tar.gz` below contains the server and ready-made configs (systemd, TURN via coturn, TLS via Caddy); the Docker Compose setup is in the source under `deploy/`. See [networking.md](https://github.com/christophermoodaley/MistRemotePlay/blob/{{TAG}}/docs/networking.md#deployment-docker-compose).

## Files

| File | For |
|---|---|
| `Mist.Remote.Play_{{VERSION}}_x64-setup.exe` | Windows 10/11 64-bit installer (host and player) |
| `mist-signaling-server-linux-x86_64.tar.gz` | Optional relay/signaling server for a Linux VPS |
| `SHA256SUMS.txt` | Checksums of the files above |

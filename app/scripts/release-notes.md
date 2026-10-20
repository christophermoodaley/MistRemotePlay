Stream games from one Windows PC to another — on your home network or over the internet.

> **Pre-release.** Built on a Windows gaming PC (RTX 4090), where hosting streams the real screen at 1080p60 with the Media Foundation encoder. Several people at once has been tested within one PC (automated tests), not yet across several PCs, and streaming over the internet has not been confirmed yet. Please report what works and what doesn't.

## What's new in {{VERSION}}

- **Your public address and a connection test.** **Host → How others connect** now shows the address to give friends (your public IP, or the address where UPnP opened the port) instead of leading with local addresses. **Test connection** checks step by step: your public address, whether the router opened the port, whether the router and your internet traffic use the same address (two internet lines or a second router), and whether that address leads back to this PC.

**Also new since 0.1.5:**

- **Fullscreen works.** The **Fullscreen** button on the Streaming page used to put the stream fullscreen *behind* the Mist window; it now comes to the front, on the monitor it is on. `Ctrl` `Alt` `Shift` `F` still toggles it too.
- **See who is connected.** While someone is connected, the host shows a small notice at the top of its screen with their names and whether they play or watch. It is left out of the stream, so only the host sees it. Turn it off under **Settings → Host**.
- **No port forwarding (UPnP).** While hosting, Mist asks the router to open its port (TCP 47900) and shows the address to share under **Host → How others connect**. If the router doesn't allow it, the page says why and shows the manual steps. Turn it off under **Settings → Host**.
- **Controllers that connect late now work.** A controller (e.g. Bluetooth) switched on after the stream started was never forwarded. Update the **player** PCs for this one.
- Windows only from now on; Linux support has been removed.
- **Known issue:** on some NVIDIA PCs hosting ends with *"Could not encode stream (nvh264enc…)"*. Choose **Settings → Host → Video encoder → Media Foundation (hardware)**; it still encodes on the NVIDIA GPU.
- PCs on 0.1.3 or newer can still connect to each other; nothing changes in how they talk.

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

Full guide and troubleshooting: [README](https://github.com/christophermoodaley/MistRemotePlay/blob/{{TAG}}/README.md)

## If the stream never starts over the internet

Some networks (mobile, some ISPs, corporate) don't allow a direct connection between the PCs. Then you need a relay server on a machine with a public address (e.g. a small VPS). `mist-signaling-server-linux-x86_64.tar.gz` below contains the server and ready-made configs (systemd, TURN via coturn, TLS via Caddy); the Docker Compose setup is in the source under `deploy/`. See [networking.md](https://github.com/christophermoodaley/MistRemotePlay/blob/{{TAG}}/docs/networking.md#deployment-docker-compose).

## Files

| File | For |
|---|---|
| `Mist.Remote.Play_{{VERSION}}_x64-setup.exe` | Windows 10/11 64-bit installer (host and player) |
| `mist-signaling-server-linux-x86_64.tar.gz` | Optional relay/signaling server for a Linux VPS |
| `SHA256SUMS.txt` | Checksums of the files above |

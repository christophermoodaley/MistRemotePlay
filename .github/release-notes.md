Stream games from one Windows PC to another — on your home network or over the internet.

> **Pre-release.** The installer is built and tested automatically on GitHub's Windows machines (the installed app must find its own codecs and stream a test picture end to end), but it has not been tried on a physical gaming PC yet. Please report what works and what doesn't.

## Install (both PCs)

1. Download **`Mist.Remote.Play_0.1.0_x64-setup.exe`** below and run it.
2. Windows may say *"Windows protected your PC"* (the installer isn't code-signed yet): click **More info → Run anyway**, then approve the administrator prompt.

Everything is included — no GStreamer, codecs, runtimes or server to install separately.

## First connection

**On the PC with the games (host):** open Mist → **Host** → **Start hosting** → **Create pairing code**.

**On the PC you play on:** open Mist → **Connect**:
- *Same network:* **Scan the network** → **Pair** next to the host.
- *Over the internet:* type the host's public address (the host must forward **TCP port 47900** on its router to the host PC).

Enter the pairing code, compare the fingerprint with the one shown on the host (Host → How others connect), then approve on the host. After that the host appears on **Home** — click **Connect**.

In the stream window, hold **Ctrl + Alt + Shift** and press **Q** to disconnect, **F** for fullscreen, **M** to capture the mouse, **S** for statistics.

**Controllers:** controllers on the player PC are forwarded automatically. On the host, install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) once so games see them.

Full guide and troubleshooting: [docs/install-windows.md](https://github.com/christophermoodaley/MistRemotePlay/blob/v0.1.0/docs/install-windows.md)

## If the stream never starts over the internet

Some networks (mobile, some ISPs, corporate) don't allow a direct connection between the PCs. Then you need a relay server on a machine with a public address (e.g. a small VPS). `mist-signaling-server-linux-x86_64.tar.gz` below contains the server and ready-made configs (systemd, TURN via coturn, TLS via Caddy); the Docker Compose setup is in the source under `deploy/`. See [networking.md](https://github.com/christophermoodaley/MistRemotePlay/blob/v0.1.0/docs/networking.md#deployment-docker-compose).

## Files

| File | For |
|---|---|
| `Mist.Remote.Play_0.1.0_x64-setup.exe` | Windows 10/11 64-bit installer (host and player) |
| `mist-signaling-server-linux-x86_64.tar.gz` | Optional relay/signaling server for a Linux VPS |
| `SHA256SUMS.txt` | Checksums of the files above |

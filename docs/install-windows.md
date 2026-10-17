# Install Mist on Windows (no compiling)

You need two Windows 10/11 PCs (64-bit): the **host**, which runs the games,
and the **player**, which you sit at. Both get the same installer.

## 1. Download and install (both PCs)

1. Download **`Mist Remote Play_<version>_x64-setup.exe`**:
   * from the project's **Releases** page on GitHub, or
   * from the latest successful **Windows installer** run under the
     repository's **Actions** tab (artifact *MistRemotePlay-windows-installer*,
     a zip that contains the installer; you must be signed in to GitHub).
2. Run it. Windows may show *"Windows protected your PC"* because the
   installer is not code-signed yet: click **More info → Run anyway**.
3. Approve the administrator prompt. The installer adds Mist and two
   Windows Firewall rules so the PCs can reach each other.

Everything Mist needs is included (video/audio engine, codecs, runtime
libraries). Nothing else has to be installed, except optionally
**ViGEmBus** on the host if you want to play with game controllers (see
[Controllers](#controllers)).

## 2. On the host PC

1. Open **Mist Remote Play** → **Host** → **Start hosting**.
2. Click **Create pairing code**. Keep this window open.

The host now runs Mist's server itself. The **How others connect** card
shows the addresses other PCs use, and this PC's certificate fingerprint.

## 3. On the player PC

1. Open **Mist Remote Play** → **Connect**.
2. **Same network** (home Wi-Fi/LAN): click **Scan the network**, then
   **Pair** next to the host. **Over the internet**: type the host's address
   (see [Playing over the internet](#playing-over-the-internet)).
3. Type the pairing code from the host and click **Pair with this host**.
4. When connecting to the address for the first time, Mist shows the
   server's **fingerprint**. Compare it with the one on the host (Host → How
   others connect). If it matches, click **It matches — trust and pair**.
5. On the host, check the device name and fingerprint and click **Approve**.

From now on the host appears on the player's **Home** page: click
**Connect**, approve the session on the host, and play. Shortcuts in the
streaming window: hold `Ctrl` `Alt` `Shift` and press **Q** (disconnect),
**F** (fullscreen), **M** (capture/release the mouse), **S** (statistics),
**A** (mute).

## Playing over the internet

The player needs to reach the host's server, which listens on **TCP port
47900** (changeable under Settings → Host):

1. In the **host's router**, forward TCP port 47900 to the host PC's local
   address (shown under Host → How others connect, e.g. `192.168.1.40`).
2. On the player, enter the host's **public IP address** (search "what is my
   IP" on the host) or a dynamic-DNS name, e.g. `myhome.duckdns.org`.

Video and audio then travel directly between the two PCs (encrypted,
peer-to-peer; Mist uses public STUN servers to get through the routers).
No port forwarding is needed for them in most homes.

**If connecting works but the stream never starts**, the two networks do
not allow a direct path (common with mobile networks, some ISPs and
corporate networks). Then you need a relay (TURN) server on a machine with
a public address, e.g. a small VPS: see
[networking.md](networking.md#deployment-docker-compose) for the ready-made Docker
deployment (signaling server + TURN + automatic TLS). Point both PCs at it
under Settings → Network and turn off **Run Mist's server on this PC** on
the host (Settings → Host).

## Controllers

Controllers plugged into the player PC are forwarded automatically. On the
**host**, install **ViGEmBus** once
(<https://github.com/nefarius/ViGEmBus/releases>) so Mist can create virtual
Xbox controllers for games. Without it, keyboard and mouse still work.

## Troubleshooting

| Problem | What to do |
|---|---|
| "cannot use port 47900 for the built-in server" | Another program (or a second Mist host) uses the port: change it under Settings → Host, then forward the new port. |
| Scan finds nothing | Both PCs must be on the same network, and the network must allow multicast (some guest Wi-Fi networks don't). Type the address shown on the host instead. |
| "cannot reach signaling server" | Wrong address, the host isn't hosting, or (internet) the port forward is missing. From the player you can test with `Test-NetConnection <address> -Port 47900` in PowerShell. |
| The fingerprint doesn't match | Don't trust it: you may not be talking to your host. Check the address. If you reinstalled the host, its fingerprint changed; remove the old entry from `%APPDATA%\Mist\MistRemotePlay\config\known-servers.json` and pair again. |
| Stream never starts over the internet | A direct path isn't possible; see the relay server above. |
| Black screen / no video | Settings → Diagnostics shows which encoders and capture methods work on that PC. |

More: [troubleshooting.md](troubleshooting.md) · [windows-setup.md](windows-setup.md) (details and building from source).

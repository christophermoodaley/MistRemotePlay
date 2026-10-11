# Troubleshooting

Start with `mist diagnose` (or Settings → Diagnostics): it lists capture
backends, working encoders/decoders, audio devices and input permissions as
detected right now. Logs: run with `MIST_LOG=debug`, or see Settings →
Diagnostics → Recent logs. `--json` makes CLI output machine-readable.

## Connecting

| Symptom | Cause / fix |
|---|---|
| `unencrypted ws:// is only allowed to localhost` | Use `wss://` for any remote server (TLS is required). |
| `invalid peer certificate: UnknownIssuer` | Self-signed server: `mist config set-ca /path/ca.pem` (Settings → Network → Extra CA). |
| `invalid peer certificate: CaUsedAsEndEntity` | The server is using a CA certificate directly; issue a server certificate signed by the CA. |
| `cannot reach signaling server … timed out` | Wrong URL/port, server down, or firewall. Check `curl https://HOST/healthz`. |
| `unknown or expired pairing code` | Codes expire after 5 minutes and work once. Create a new one; make sure both devices use the same signaling server. |
| `too many pairing attempts` | Rate limit (5/min). Wait a minute. |
| `rejected by host: this device is not paired` | The host forgot this device (revoked or reinstalled). Pair again. |
| `the host did not approve the session in time` | Someone must click *Allow* on the host within 90 s, or enable "Approve sessions from paired devices" on the host. |
| `host is busy with another session` | One session at a time per host. |
| `could not establish a peer connection (ICE)` | No direct path and no working TURN. Run `mist ice-check`; configure TURN (docs/networking.md). |
| Stats show `TURN relay` and high latency | The direct path failed; check that UDP isn't blocked and that both sides are not behind symmetric NAT. |
| `connection interrupted; waiting for recovery` | Network loss. Mist recovers automatically: same connection if it returns quickly, otherwise a new one (resume) without a new prompt. |

## Video

| Symptom | Cause / fix |
|---|---|
| `no working H.264 encoder found` | Install `gstreamer1.0-plugins-ugly` (x264) or OpenH264. |
| Hardware encoder listed but not used | Its test encode failed (driver/GPU); see the error in Diagnostics. Mist falls back automatically. |
| Low frame rate at high resolution | Software encoding is CPU bound: lower the resolution/frame rate, use the *Low latency* preset (fastest encoder settings), or a hardware encoder. Watch *Encode time* and *Dropped before encode* in host stats. |
| Latency higher than expected | Turn on the stats overlay (Ctrl+Alt+Shift+S) or look at *Latency breakdown* on the Streaming page: host = grab + conversion + encode (use the *Low latency* preset or a hardware encoder), network (TURN relay or Wi-Fi), decode, convert (window size differs from the stream on the software renderer: match them or use the GPU renderer), present. |
| `GPU video renderer unavailable (…); using the software renderer` | The GPU sink could not draw into the window, or OpenGL measured slower than 4 ms per 1080p frame (software OpenGL such as llvmpipe). Install your GPU's OpenGL driver and `gstreamer1.0-gl`; *Renderer → GPU (always)* forces it. |
| Stats overlay makes the stream slower | It costs a few ms per frame while visible on the software renderer; hide it (Ctrl+Alt+Shift+S) when you don't need it. |
| `no screen capture backend available` | The host has no graphical session (`DISPLAY`/`WAYLAND_DISPLAY` unset). |
| Wayland: nothing happens when a session starts | The portal dialog may be on another screen; also check that an `xdg-desktop-portal` backend is installed. |
| Black/frozen image on Wayland when the screen is static | PipeWire only sends frames on damage; Mist sets `keepalive-time` on `pipewiresrc` to re-send the last frame (untested on real compositors). Report your compositor if it persists. |

## Audio

| Symptom | Cause / fix |
|---|---|
| `system audio unavailable` / `audio device error … retrying` | No PulseAudio/PipeWire-pulse server, or the selected output disappeared. Video continues; audio restarts automatically. |
| You hear your own microphone | Not possible by design (only output monitors are captured). Check whether a loopback module routes the mic to the output on the host. |
| No audio on the client | Check the client volume/mute (Streaming page, Ctrl+Alt+Shift+A) and `no usable audio output device` warnings. |

## Input

| Symptom | Cause / fix |
|---|---|
| `Remote input is disabled by the host` | The host turned input off (Host → Remote control). |
| `remote input unavailable: no permission to open /dev/uinput` | See docs/linux-setup.md (udev rule) or use XTEST on X11. |
| Windows: input doesn't reach some windows | UIPI: elevated windows refuse input from a non-elevated host. |
| Keys stuck after a crash | The host releases all input after 3 s without data and on session end; toggling *Allow input* off/on also releases everything. |
| Mouse is offset on multi-monitor Wayland | Use relative mode (Ctrl+Alt+Shift+M). |
| Controller not detected | `virtual controllers unavailable` on the host: install ViGEmBus (Windows) or grant uinput (Linux). On the client: Settings → Controllers → Detect. |
| No rumble | The client's controller must support force feedback (gilrs reports it per pad), and the game must use the virtual pad's rumble. The host's *Input* stat counts forwarded rumble commands; 0 means the game sent none. |

## Desktop app

* `cannot start the streaming client`: the app could not find the bundled
  `mist` CLI. In development set `MIST_CLI_PATH=/path/to/target/release/mist`.
* Blank window on Linux VMs: try `WEBKIT_DISABLE_COMPOSITING_MODE=1`.

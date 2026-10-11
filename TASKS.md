# Tasks

## Handoff checkpoint (resume here)

Everything is pushed to `claude/steam-remote-play-alt-zs3as5`. Latest round (2026-10-11): **lowest latency +
controller support**. Done and measured (details and numbers in docs/benchmarks.md):

1. ✅ Per-stage latency breakdown (host / network / decode / convert / present) in every client stats event, the
   overlay, the desktop app and `e2e.py`. The capture time now includes the X11 grab (~1.7 ms).
2. ✅ Stats overlay taken out of the video path while hidden (it cost ~3 ms/frame even invisible); off by default.
3. ✅ *Low latency* preset (x264 ultrafast, 20 Mbit/s cap) is the new default: 1080p60 15.2 ms vs 18.4 ms Balanced.
4. ✅ GPU window renderer on X11 (`glimagesink` embedded in the window, input passes through, overlay on the GPU);
   `auto` uses it only when a 1080p frame draws in < 4 ms, so software OpenGL falls back. Verified working with
   llvmpipe; **not benchmarked on a real GPU**.
5. ✅ NACK retransmission was silently off (0 NACKs at 5 % loss, also before this round): now enabled on the client's
   jitter buffers; 5 % loss 48 → 28.5 ms. FEC tried, no gain, not enabled.
6. ✅ Controllers: host → client **rumble** (uinput FF_RUMBLE, ViGEm notifications, gilrs on the client) and a
   cross-process controller test (`e2e.py --gamepad-check`, in CI): input applied, rumble echoed back, held pad
   neutralized on disconnect and on client SIGKILL. uinput/ViGEm/gilrs FF code compiled but not run (no hardware).
7. ✅ Bugs found by re-running e2e: XTEST release-all could be lost when the input worker closed its X connection
   (Shift stayed held ~1 in 3 runs) — round trip before close; pairing code shown before the server registered it
   (2/8 failures under loss) — shown on `PairingRegistered`.
8. ✅ `cargo test --workspace`: 93 passed, 0 failed; clippy clean; Windows cross-check clean; full e2e sweep in docs/testing.md.

Next candidates: benchmark the GPU renderer on real hardware; a Windows GPU renderer that keeps mouse input
(e.g. draw into our own D3D11 swap chain); hardware encoders with GPU-side colour conversion (host stage is
grab + ~2.5 ms CPU conversion + encode); avoid host-side CPU downscaling (720p from a 1080p desktop costs ~4 ms);
tail-loss mitigation (working FEC needs webrtcbin to map the FEC stream); run uinput force feedback on a real
`/dev/uinput`.

Test environment needs (recreate after a container reset): `Xvfb :99` (+ `xclock`/animated pattern for motion),
PulseAudio with a null sink set as default (`pulseaudio -D --exit-idle-time=-1 --load=module-null-sink ...`),
apt packages listed in docs/linux-setup.md plus coturn, iproute2, xdotool, ffmpeg, gcc-mingw-w64-x86-64.

Status as of 2026-10-11. "Verified" means run in this project's tests; see
docs/compatibility.md for the full matrix and docs/benchmarks.md for numbers.

## Milestone 0 — Toolchain ✅

- [x] Inspected environment: Ubuntu 24.04 VM, 4 vCPU, no GPU, no `/dev/uinput`, headless
- [x] Installed GStreamer 1.24.2 (+ webrtcbin/libnice, x264, openh264, libav, pulse), WebKitGTK, coturn, PulseAudio, mingw (cross-check)
- [x] Capture+encode prototype (`ximagesrc ! x264enc`, 1080p ≈ 58 fps)
- [x] Workspace, architecture doc, this file, documented build

## Milestone 1 — Live video ✅

- [x] Host captures the real X11 desktop, encodes H.264, streams via WebRTC
- [x] Client decodes and renders (headless sink and native window)
- [x] Two separate processes; separate network namespaces (no second physical machine available)
- [x] End-to-end test asserting changing frames

## Milestone 2 — Pairing, auth, WebRTC ✅

- [x] Signaling protocol (versioned, typed, validated), Ed25519 device identities
- [x] Pairing codes with HMAC proofs, host consent, single use, TTL, failure cap
- [x] Signed end-to-end envelopes (SDP/ICE bound to device keys), replay guard
- [x] TLS signaling (wss) with custom-CA support; plain ws only to localhost
- [x] STUN/TURN config from the server; accurate failure reporting

## Milestone 3 — Audio and input ✅ (Linux X11), Implemented elsewhere

- [x] System audio (PulseAudio monitor → Opus), device-failure restart, resampling (44.1 → 48 kHz verified)
- [x] Keyboard, mouse (absolute/relative/buttons/scroll) via XTest — verified across processes
- [x] Validation, permission gate, rate limits, release on disconnect/interruption — verified (incl. SIGKILL)
- [x] Controller subsystem: discovery (gilrs), normalization, mapping editor, slots, unreliable state channel
- [x] Rumble host → client (uinput FF, ViGEm notifications, gilrs); controller path verified across processes (CI)
- [ ] Run uinput keyboard/mouse, virtual gamepads and their rumble on a machine with `/dev/uinput` (blocked here)
- [ ] Test with physical controllers (none attached in the VM)

## Milestone 4 — Internet ✅ (simulated)

- [x] Signaling server with health endpoints, limits, structured logs, env config
- [x] coturn integration (TURN REST credentials), forced relay, UDP-blocked (TURN/TCP) — verified in netns
- [x] Liveness detection, reconnect with backoff, resume on a new peer connection — verified with 10/13/25 s outages
- [x] `mist ice-check`: distinguishes configured vs working TURN
- [ ] Test across real internet paths / physical NATs (blocked: single VM)
- [ ] Test TURN over TLS (`turns:`) with a real certificate
- [ ] Run the Docker Compose deployment (no Docker in the VM)

## Milestone 5 — Acceleration and adaptation ✅ software / ⏳ hardware

- [x] Encoder/decoder detection with functional probes, software fallback
- [x] Adaptive bitrate (AIMD on RTCP loss/RTT + client drops) — verified under induced loss
- [x] Bounded queues (leaky pre-encoder queue, 1-frame appsink, latest-frame renderer)
- [x] Per-frame timing (capture, encode, decode, capture→display latency via SEI + clock sync)
- [x] 1080p60 measured with software encoding
- [x] Latency breakdown per stage; overlay off the video path; Low latency preset; NACK retransmission fixed
- [x] GPU window renderer on X11 with measured fallback (verified with software OpenGL only)
- [ ] Benchmark the GPU renderer on a real GPU; GPU renderer on Windows/Wayland
- [ ] Verify hardware encoders/decoders (NVENC, VA, QSV, AMF, MF) — blocked: no GPU
- [ ] Measure A/V offset explicitly; consider rtpbin lip-sync for audio
- [ ] GPU-resident path on Linux (VA memory / dmabuf from PipeWire)

## Milestone 6 — Cross-platform hardening ⏳

- [x] Linux X11 host → Linux client
- [x] Windows code paths type-check (`x86_64-pc-windows-gnu`)
- [ ] Windows → Windows, Windows → Linux, Linux → Windows (blocked: no Windows machine)
- [ ] Wayland (GNOME/KDE/wlroots) host tests (blocked: no compositor)
- [ ] GPU encoder matrix

## Milestone 7 — Release quality ⏳

- [x] Desktop UI (Home, Host, Connect, Streaming, Settings) on real state; verified with two instances
- [x] Configuration persistence; secrets in owner-only/DPAPI storage
- [x] Unit, integration and e2e tests; CI workflow (`.github/workflows/ci.yml`) green on GitHub (Linux, Windows, cross-check, frontend)
- [x] Install, troubleshooting, security, networking, compatibility docs
- [x] Dependency license audit (docs/licensing.md)
- [x] Linux `.deb` package: built, installed and launched locally
- [ ] AppImage (needs network access to linuxdeploy)
- [ ] Windows installer (MSI/NSIS) and bundling the GStreamer runtime
- [ ] Code signing; auto-update (out of scope for now)

## Backlog / improvements

- [ ] PAKE (e.g. SPAKE2/CPace) pairing so a malicious server cannot brute-force the 40-bit code
- [ ] Multiple simultaneous viewers per host (tee + per-peer webrtcbin)
- [ ] HEVC / AV1 where both endpoints support them; HDR; 1440p120 / 4K60 testing
- [ ] Faster recovery after long outages (reset signaling backoff when the network returns)
- [ ] Microphone forwarding (opt-in, explicit consent) — intentionally not implemented yet
- [ ] `cargo deny` in CI
- [x] GPU rendering in the client window — X11 via embedded `glimagesink` (Windows/Wayland still CPU)
- [ ] Stream at the client's window/monitor size so neither side scales on the CPU

## Known issues

- Software renderer (Windows, Wayland, or slow OpenGL) scales and copies frames on the CPU: ~5 ms at 1080p→720p,
  ~10 ms at 1080p→1440p, plus ~1 ms present.
- Under packet loss, a lost last packet of a frame is only detected when the next frame arrives (tail loss).
- uinput absolute pointer maps across all monitors on Wayland (use relative mode on multi-monitor hosts).
- One streaming session per host.

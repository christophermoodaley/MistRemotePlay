# Tasks

Status as of 2026-10-09. "Verified" means run in this project's tests; see
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
- [ ] Run uinput keyboard/mouse and virtual gamepads on a machine with `/dev/uinput` (blocked here)
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
- [x] Unit, integration and e2e tests; CI workflow written (`.github/workflows/ci.yml`, not yet run on GitHub)
- [x] Install, troubleshooting, security, networking, compatibility docs
- [x] Dependency license audit (docs/licensing.md)
- [ ] Linux packages: `.deb` built locally (see below); AppImage needs network access to linuxdeploy
- [ ] Windows installer (MSI/NSIS) and bundling the GStreamer runtime
- [ ] Code signing; auto-update (out of scope for now)

## Backlog / improvements

- [ ] PAKE (e.g. SPAKE2/CPace) pairing so a malicious server cannot brute-force the 40-bit code
- [ ] Multiple simultaneous viewers per host (tee + per-peer webrtcbin)
- [ ] HEVC / AV1 where both endpoints support them; HDR; 1440p120 / 4K60 testing
- [ ] Faster recovery after long outages (reset signaling backoff when the network returns)
- [ ] Microphone forwarding (opt-in, explicit consent) — intentionally not implemented yet
- [ ] `cargo deny` in CI
- [ ] GPU rendering in the client window (wgpu) instead of CPU blit

## Known issues

- Client window copies frames on the CPU; adds roughly one frame of latency vs headless.
- uinput absolute pointer maps across all monitors on Wayland (use relative mode on multi-monitor hosts).
- One streaming session per host.

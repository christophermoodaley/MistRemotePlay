# Benchmarks

All numbers below were **measured**, not estimated. They come from the
end-to-end harness (`tests/e2e/e2e.py --json-out …`) with separate host,
client and signaling processes.

## Environment (2026-10-11 measurements)

| | |
|---|---|
| Machine | Cloud VM, 4 vCPU Intel Xeon @ 2.10 GHz, 16 GB RAM, **no GPU** |
| OS | Ubuntu 24.04, kernel 6.18.44 |
| GStreamer | 1.24.2 |
| Display | Xvfb 1920×1080, a 60 fps animated test pattern (SMPTE bars + running clock) drawn on screen and captured with `ximagesrc` |
| Encoder / decoder | x264 (software) / FFmpeg `avdec_h264` (software, slice threads) |
| Network | Loopback (no added latency) |
| Build | `cargo build --release` |

Host and client run on the **same machine**, so they share the CPU (encode
and decode compete with each other and with the X server drawing the test
pattern) and the network adds almost no delay. Real two-machine results
will differ; this table is a baseline, not a promise. Run-to-run variation
on this shared VM is ±1–2 ms, so each row is the mean of three 15 s runs.

## Definitions

* **Latency**: from the moment the capture source sampled the frame (its
  timestamp on the pipeline clock, so the X11 grab is included) until the
  decoded frame reaches the client's presentation step (headless sink:
  frame handed to the sink; software window: after the present call; GPU
  window: the draw call). Measured per frame via an embedded SEI timestamp
  and NTP-style clock-offset estimation (offset uncertainty ±0.5–1 ms).
  Not included: how long the content waited on screen before the next
  capture tick (up to one frame interval), and display scan-out.
* **Stages** (sum to the total): *host* = grab + colour conversion/scaling
  + encode; *network* = packetization, transport, jitter buffer,
  depayloading; *decode*; *convert* = client colour conversion/scaling (or
  GPU upload); *present*. Every client stats event carries this breakdown
  (`latency_breakdown`); `e2e.py` averages it over the run.
* **Grab / Encode**: mean per-frame time of the X11 grab and inside the
  encoder element.

## Results

| Scenario | Stream | Latency (mean of 3 runs) | host | network | decode | convert | present | Encode | Grab | Bitrate |
|---|---|---|---|---|---|---|---|---|---|---|
| Headless client, **Low latency** preset (x264 ultrafast) | 1920×1080 @ 60 | **15.2 ms** (14.5 / 14.9 / 16.1) | 9.7 | 1.8 | 2.4 | 1.1 | 0.2 | 5.3 ms | 1.7 ms | 16.2 Mbit/s |
| Headless client, Balanced preset (x264 superfast) | 1920×1080 @ 60 | 18.4 ms (18.1 / 18.5 / 18.8) | 12.6 | 1.6 | 2.7 | 1.3 | 0.2 | 6.8 ms | 1.9 ms | 12.2 Mbit/s |
| Headless client, Low latency, desktop downscaled on the host | 1280×720 @ 60 | 13.0 ms (12.3 / 12.6 / 14.2) | 9.0 | 1.7 | 1.6 | 0.6 | 0.1 | 2.7 ms | 2.0 ms | – |
| Window client, software renderer, 1080p stream in a 1280×720 window | 1920×1080 @ 60 | 22.5 ms (21.2 / 25.4 / 20.9) | 11.3 | 2.0 | 2.7 | 5.2 | 1.3 | – | – | – |

All rows rendered 55–60 fps with 0 frames dropped by the renderer.

### Impaired network (network namespaces, TLS signaling, 1280×720 @ 30, Low latency preset)

Random loss is applied with iptables to everything the host sends (media,
RTCP and signaling).

| Scenario | Latency | network stage | NACKs received by the host | Notes |
|---|---|---|---|---|
| No loss | 15–17 ms | ~3 ms | 0 | |
| 5 % random loss | **28.5 ms** (21.5 / 33.1 / 31.0) | 9.5–21.8 ms | 465–658 per 20 s | before the NACK fix: 46.7–51.8 ms with **0** NACKs |
| 20 % random loss | 54.4 ms | 42.8 ms | 637 | adaptive bitrate down to 8.5 Mbit/s by the end of the 20 s run |

Under loss the remaining delay is mostly *tail loss*: when the last packet
of a frame is lost, the gap is only noticed when the next frame arrives
(33 ms later at 30 fps). ULPFEC was tried (`fec-type=ulp-red`, 10 %); it
did not improve latency here (the receiver could not associate the FEC
stream) and added ~1 ms without loss, so it is not enabled.

What the breakdown shows:

* **Host**: the Low latency preset halves x264's share; what remains is the
  X11 grab (~1.7 ms), BGRx→I420 conversion (~2.5 ms at 1080p) and, at
  720p, CPU downscaling of the 1080p desktop (~4 ms). A hardware encoder
  with GPU-side conversion would remove most of this (not measurable here).
* **Client software renderer**: scaling on the CPU dominates when the
  window size differs from the stream (5.2 ms for 1080p→720p; ~10 ms
  measured in isolation for 1080p→1440p). The GPU renderer moves this to
  the graphics card, but on this VM OpenGL is software (llvmpipe,
  ~54 ms per 1080p frame), so its benefit could not be measured and
  `renderer = auto` correctly falls back to the software renderer.
* **Network** on loopback is ~1.7 ms, mostly packetizing and handing ~20
  packets per frame through webrtcbin. The jitter-buffer size had no
  effect without loss (40 / 10 / 0 ms gave 18.7 / 17.6 / 18.5 ms total).

### Changes measured on 2026-10-11

Before/after on this VM, 1080p60 headless (each change measured on its own):

| Change | Effect |
|---|---|
| Statistics overlay removed from the video path while hidden (it cost time even when invisible) and off by default | client convert stage 4.4 → 1.2 ms |
| Low latency preset (x264 ultrafast, 20 Mbit/s cap) | 18.4 → 15.2 ms total |
| Grab time now counted (was outside the measurement before) | +1.7–2.0 ms to every number, an honesty fix rather than a slowdown |
| NACK retransmission actually enabled on the client's jitter buffers (`rtx-delay` 0, NACK only real gaps) | 5 % loss: ~48 → 28.5 ms; no loss: unchanged, 0 NACKs |

Before these changes the same VM measured 17.6–19.8 ms at 1080p60
*excluding* the grab, so the comparable figure is ~19.5–21.5 ms, versus
14.5–16.1 ms now.

## Earlier measurements (2026-10-09, different VM, latency excluded the screen grab)

These used a 4 vCPU Xeon @ 2.30 GHz VM, the Balanced preset (x264
`superfast`) and the earlier definition that started at the frame entering
the pipeline. They are kept for reference and are not directly comparable
with the table above.


| Scenario | Resolution | Rendered fps | Latency (mean, last 2 s) | Encode | Decode | Video bitrate | Path |
|---|---|---|---|---|---|---|---|
| Direct, loopback | 1280×720 @ 60 | 60.0 | 8.7 ms | 2.5 ms | 1.2 ms | 12.4 Mbit/s | host/udp |
| Direct, loopback | 1920×1080 @ 60 | 60.0 | 11.8 ms | 4.9 ms | 1.8 ms | 12.2 Mbit/s | host/udp |
| TURN relay (direct path blocked), namespaces | 1920×1080 @ 60 | 60.0 | 11.2 ms | 4.4 ms | 1.6 ms | 12.6 Mbit/s | relay/udp |
| 5 % random UDP loss (host egress), namespaces | 1920×1080 @ 60 | 59 | 18.9 ms | 4.2 ms | 1.8 ms | 11.9 Mbit/s | host/udp (749 NACKs, 0 unrecovered) |
| 20 % random loss on all host egress, namespaces | 1920×1080 @ 60 | 59 | 30.5 ms | 3.7 ms | 1.4 ms | 1.5 Mbit/s (controller at its 2 Mbit/s floor) | host/udp |
| Window client (softbuffer on Xvfb), live desktop | 1280×720 @ 30 | 30.0 | 23 ms | 2.4 ms | 0.5 ms | 0.1 Mbit/s (static desktop) | host/udp |
| Desktop app ↔ desktop app (Xvfb), live UI | 1600×900 @ 60 | 60 | 8–9 ms | 2.5 ms | 0.7 ms | – | host/udp |

Other observations:

* 1080p60 software encoding used ~5 ms per frame with x264 `superfast` on
  this CPU, leaving headroom at 60 fps; the leaky queue dropped a small number
  of frames before the encoder when host and client competed for CPU.
* The window client adds roughly one frame of latency compared with the
  headless sink (CPU blit plus present on a virtual X server).
* TURN over TCP with all UDP blocked: ICE took ~2 s longer; RTT 3.6 ms vs
  1.5 ms direct on the same bridge.

## Not measured (no hardware available)

* Hardware encoders/decoders (NVENC, VA-API, QSV, AMF, Media Foundation).
* The GPU window renderer on a real GPU (only software OpenGL here; it was
  verified to work, including input pass-through, but not benchmarked).
* 2560×1440 @ 120 and 3840×2160 @ 60.
* Real two-machine LAN and internet paths (Wi-Fi, WAN latency).
* Windows hosts and clients.

## Reproducing

```bash
cargo build --release
Xvfb :99 -screen 0 1920x1080x24 &
DISPLAY=:99 gst-launch-1.0 videotestsrc pattern=smpte is-live=true ! \
  video/x-raw,width=1920,height=1080,framerate=60/1 ! timeoverlay ! videoconvert ! ximagesink &
export DISPLAY=:99 MIST_BIN_DIR=$PWD/target/release
python3 tests/e2e/e2e.py --duration 15 --width 1920 --height 1080 --fps 60 --json-out result.json
# relay: add --netns --tls --block-direct --turn ; loss: --netns --tls --impair-loss 0.2   (root required for netns)
# Balanced preset instead of Low latency:
#   --client-config 'client.stream.encoder_quality="balanced"' --client-config client.stream.max_bitrate_kbps=15000
# window client on a second display: Xvfb :98 -screen 0 1920x1080x24 &  then add --window-display :98
```

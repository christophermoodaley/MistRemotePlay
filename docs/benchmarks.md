# Benchmarks

All numbers below were **measured**, not estimated. They come from the
end-to-end harness (`tests/e2e/e2e.py --json-out …`) with separate host,
client and signaling processes.

## Environment

| | |
|---|---|
| Machine | Cloud VM, 4 vCPU Intel Xeon @ 2.30 GHz, 15 GB RAM, **no GPU** |
| OS | Ubuntu 24.04.5, kernel 6.18.44 |
| GStreamer | 1.24.2 |
| Display | Xvfb 1920×1080, a 60 fps animated test pattern (SMPTE bars + running clock) drawn on screen and captured with `ximagesrc` |
| Encoder / decoder | x264 (software, `superfast`, zerolatency) / FFmpeg `avdec_h264` (software) |
| Network | Loopback, or Linux network namespaces joined by a bridge (no added latency) |
| Build | `cargo build --release`, date 2026-10-09 |

Host and client run on the **same machine**, so they share the CPU (encode
and decode compete) and the network adds almost no delay. Real two-machine
results will differ; this table is a baseline, not a promise.

## Definitions

* **Latency**: from the moment a captured frame enters the host pipeline
  until the decoded frame reaches the client's presentation step (headless
  sink: frame handed to the sink; window: after the window present call).
  Measured per frame via an embedded SEI timestamp and NTP-style clock offset
  estimation; the clock-offset uncertainty was ±0.5–1 ms in these runs.
  Not included: the time before the X server hands the frame to
  `ximagesrc`, and display scan-out.
* **Encode / decode**: mean per-frame time inside the encoder/decoder element.
* **RTT**: data-channel ping round trip (client side).

## Results

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
```

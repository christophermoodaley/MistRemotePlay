# Architecture

## Overview

```
          Host PC                                                    Client PC
┌──────────────────────────────┐   WSS (signed envelopes)   ┌──────────────────────────────┐
│ Desktop UI / `mist host`     │◄──────────► Signaling ◄────►│ Desktop UI / `mist connect`  │
│  └ HostService (app-core)    │            server           │  └ client session runner     │
│     ├ CaptureBackend         │                             │     ├ ClientMedia (decode)   │
│     ├ HostMedia (encode)     │   WebRTC (DTLS-SRTP/SCTP)   │     ├ window (winit+softbuf) │
│     ├ InputDispatcher ◄──────┼─────────────────────────────┼──── ├ keyboard/mouse capture  │
│     └ ControllerBackend      │  direct (ICE/STUN) or TURN  │     └ ControllerSource       │
└──────────────────────────────┘                             └──────────────────────────────┘
```

* The **signaling server** authenticates devices and relays small signed
  messages (pairing, session requests, SDP, ICE candidates). It never sees
  media, input, pairing codes or secrets.
* **Media and input flow directly between peers** over one bundled WebRTC
  connection: H.264 video and Opus audio over SRTP, input/control over SCTP
  data channels. When no direct path exists, ICE uses a TURN relay.

## Crates and interfaces

Platform-specific code sits behind traits so it can be tested without
hardware and replaced per platform:

| Interface | Crate | Implementations |
|---|---|---|
| `CaptureBackend` | `capture-api` | X11 (RandR + `ximagesrc`), Wayland (xdg-desktop-portal ScreenCast → PipeWire), Windows (Win32 monitor enumeration + `d3d11screencapturesrc`), test pattern |
| `AudioCaptureBackend` | `audio-api` (impl in `media-engine`) | PulseAudio/PipeWire-pulse monitor, WASAPI loopback, test tone |
| `EncoderBackend` | `media-engine::codecs` | NVENC, NVENC-D3D11, QSV, AMF, VA (lp/normal), legacy VA-API, Media Foundation, x264, OpenH264 |
| `DecoderBackend` | `media-engine::codecs` | NVDEC, D3D11, D3D12, VA, legacy VA-API, FFmpeg (avdec), OpenH264 |
| `MediaTransport` | `media-engine::transport` | `HostMedia`, `ClientMedia` (GStreamer `webrtcbin`) |
| `InputBackend` | `input-api` | X11 XTEST, Linux uinput, Win32 SendInput, mock |
| `ControllerBackend` | `controller-api` | Linux uinput gamepads, ViGEmBus (Windows), mock, "unavailable" |
| `SessionManager` | `session-manager` (impl `app-core::host::HostHandle`) | approve/reject/end sessions, input permission, emergency stop |

`media-engine` is the only crate that uses GStreamer; its public API is plain
Rust types (`HostMedia`, `ClientMedia`, `TransportEvent`, `CodecInfo`, stats
structs). Replacing GStreamer would mean reimplementing that crate only.

The UI (React in Tauri's webview) has no privileged capabilities: it calls a
fixed set of Tauri commands (`apps/desktop/src-tauri/src/lib.rs`) and
receives events. The Tauri capability file grants only `core:default`.

## Video pipeline (host)

```
capture src ─► [probe: capture time meta] ─► queue(leaky, 1 buffer) ─► videoconvertscale
  ─► caps(NV12/I420, W×H) ─► H.264 encoder ─► h264parse ─► caps(byte-stream, AU)
  ─► [probe: encode time, insert SEI capture timestamp] ─► rtph264pay ─► webrtcbin
```

* **Backpressure**: the single-buffer leaky queue before the encoder drops
  the *oldest* frame when encoding falls behind, so latency does not build up
  (`frames_dropped_before_encode` is reported).
* **Scaling** keeps the source aspect ratio and never upscales.
* **Keyframes**: GOP of 2 s plus on-demand keyframes (RTCP PLI handled by
  webrtcbin; `RequestKeyframe` on the control channel).
* **Encoder selection**: each candidate is checked for presence *and* with a
  short functional encode; the first working one is used unless the user
  picked one. Settings that exist only in some GStreamer versions are applied
  only if the element has them.
* **GPU paths**: a D3D11 capture feeding a D3D11-capable encoder stays in GPU
  memory (`d3d11convert`); all other combinations convert on the CPU
  (`videoconvertscale`, multi-threaded). Zero-copy is not claimed.

## Audio pipeline (host)

```
[separate pipeline] pulsesrc(@DEFAULT_MONITOR@) | wasapi2src(loopback)
   ─► audioconvert ─► audioresample ─► S16LE 48 kHz stereo ─► appsink
                                       │ (pushed into)
[session pipeline]  appsrc(live) ─► queue(leaky 100 ms) ─► opusenc(10 ms, low-delay, FEC)
   ─► rtpopuspay ─► webrtcbin
```

Audio capture runs in its own pipeline so that a device error (unplugged
headset, default device change) restarts audio with backoff without
interrupting video. The default *output monitor* is used, never the
microphone.

## Client pipeline

```
webrtcbin ─► rtph264depay ─► caps(byte-stream, AU) ─► [probe: SEI → capture-time meta]
  ─► h264parse ─► decoder ─► videoconvertscale(to window size, letterboxed)
  ─► caps(BGRx) ─► textoverlay(stats) ─► appsink(1 buffer, drop old)
webrtcbin ─► rtpopusdepay ─► opusdec(PLC, FEC) ─► audioconvert ─► audioresample ─► volume ─► audio sink
```

The window (`app-core::client::window`, winit + softbuffer) keeps only the
newest decoded frame; frames replaced before presentation are counted as
dropped. Pixels are copied to the window surface on the CPU.

Audio is played through a ~40 ms device buffer; video is presented as soon as
it is decoded. Both pass the same RTP jitter buffer (default 40 ms, applied
only when packets are missing). A/V offset is not separately measured.

## Timing and statistics

All timestamps use the GStreamer monotonic system clock.

* Host: capture time and encoder-input time are attached to each buffer as
  `GstReferenceTimestampMeta` (copied by encoders; PTS is not used as a key
  because encoders offset it). Encode time is measured per frame.
* The capture timestamp is embedded in the H.264 stream as an SEI "user data
  unregistered" NAL with a Mist UUID (decoders ignore it).
* The client estimates the host clock offset NTP-style from ping/pong over
  the control channel (minimum-RTT sample) and computes **capture → display
  latency** per frame. The reported uncertainty is ±RTT/2 of the best sample.
* Transport statistics come from `webrtcbin` `get-stats` (RTP packets, loss,
  jitter, NACK/PLI counts, RTCP RTT, selected ICE candidate types).

## Sessions

* One streaming session per host at a time; other requests get "busy".
* Host state machine (`session-manager::session`):
  `Requested → AwaitingApproval → Negotiating → Connected ⇄ Reconnecting → Ended(reason)`.
  Media can only start after explicit approval (or the host's opt-in
  "trust paired devices" setting).
* Liveness: the client pings every second. Three seconds of silence (and no
  decoded video on the client side) marks the connection interrupted; the
  host immediately releases held input. After the grace period the client
  builds a new peer connection with `resume = true`; the host accepts a resume
  for the same session id from the same authenticated client without a new
  prompt (live session, or within 60 s of failure).
* Signaling reconnects with exponential backoff and jitter; a lost signaling
  connection does not interrupt an established media session.

## Adaptive bitrate

`session-manager::bitrate::BitrateController` (AIMD) runs on the host once
per second using RTCP fraction lost and RTT from `webrtcbin` plus the
client's dropped-frame count: multiplicative decrease (×0.85, or ×0.6 above
25 % loss) with a 1 s cooldown, additive increase after 4 s without loss.
The encoder bitrate is changed live.

## Input

```
client window ─► InputMessage (JSON) ─► data channel "input" (reliable)
gamepads      ─► full state + seq     ─► data channel "gamepad" (unordered, no retransmit, resent every 100 ms)
host: InputWorker thread ─► InputDispatcher (decode, validate, permission, rate limit, held-state) ─► backend
```

Keys travel as USB HID usages (layout independent) and are mapped to evdev
codes, X11 keycodes and Windows scan codes from one table
(`protocol::keys`). Release-all happens on client request, focus loss,
permission revocation, interruption and disconnect.

## Desktop application

* The **host** runs in-process in the Tauri app through `HostHandle`.
* The **streaming client** runs as a child process (`mist connect`, bundled
  as a Tauri sidecar). A native window gives low-latency rendering and raw
  pointer/keyboard capture that a webview cannot provide; WebKitGTK also has
  no dependable WebRTC support. The app forwards the child's JSON events to
  the UI and controls it over stdin (`disconnect`, `mute`, `volume`,
  `overlay`, `fullscreen`).

## Key decisions

| Decision | Reason |
|---|---|
| GStreamer `webrtcbin` | Mature encoders/decoders/capture on both OSes plus WebRTC and NAT traversal in one framework. |
| SDP and candidates signed with device keys | A compromised signaling server cannot inject itself into an established pairing (DTLS fingerprints are authenticated end to end). |
| New peer connection instead of ICE restart | `webrtcbin` ICE-restart support was not relied on; renegotiating a fresh connection under the same approved session is simple and was tested. |
| Separate audio capture pipeline | Device failures restart audio only. |
| Native window for streaming | Lowest latency, pointer lock, raw input. |

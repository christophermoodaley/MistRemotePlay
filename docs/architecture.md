# Architecture

## Overview

```
          Host PC                                                    Client PC
┌──────────────────────────────┐   WSS (signed envelopes)   ┌──────────────────────────────┐
│ Desktop UI / `mist host`     │◄──────────► Signaling ◄────►│ Desktop UI / `mist connect`  │
│  └ HostService (app-core)    │            server           │  └ client session runner     │
│     ├ CaptureBackend         │                             │     ├ ClientMedia (decode)   │
│     ├ HostStream (encode)    │   WebRTC (DTLS-SRTP/SCTP)   │     ├ window (winit+softbuf) │
│     │  └ HostViewer per peer │       (one per viewer)      │                              │
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
| `MediaTransport` | `media-engine::transport` | `HostViewer`, `ClientMedia` (GStreamer `webrtcbin`) |
| `InputBackend` | `input-api` | X11 XTEST, Linux uinput, Win32 SendInput, mock |
| `ControllerBackend` | `controller-api` | Linux uinput gamepads, ViGEmBus (Windows), mock, "unavailable" |
| `SessionManager` | `session-manager` (impl `app-core::host::HostHandle`) | approve (as player or watcher)/reject/end sessions, roles, keyboard/mouse per player, input permission, emergency stop |

`media-engine` is the only crate that uses GStreamer; its public API is plain
Rust types (`HostStream`, `HostViewer`, `ClientMedia`, `TransportEvent`, `CodecInfo`, stats
structs). Replacing GStreamer would mean reimplementing that crate only.

The UI (React in Tauri's webview) has no privileged capabilities: it calls a
fixed set of Tauri commands (`apps/desktop/src-tauri/src/lib.rs`) and
receives events. The Tauri capability file grants only `core:default`.

## Video pipeline (host)

```
capture src ─► [probe: capture time = source PTS on the pipeline clock] ─► queue(leaky, 1 buffer)
  ─► videoconvertscale ─► caps(NV12/I420, W×H) ─► H.264 encoder ─► h264parse ─► caps(byte-stream, AU)
  ─► [probe: encode time, insert SEI with capture + encode-complete times] ─► rtph264pay ─► webrtcbin
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
webrtcbin ─► rtph264depay ─► caps(byte-stream, AU) ─► [probe: SEI → capture/encode-time metas, arrival time]
  ─► h264parse ─► decoder ─► [probe: decode-out time] ─► one of:
       software renderer:  videoconvertscale(to window size, letterboxed) ─► caps(BGRx)
                           ─► [textoverlay(stats), only while shown] ─► appsink(1 buffer, drop old)
       GPU renderer (X11): queue(leaky, 1 buffer) ─► [textoverlay(stats), only while shown]
                           ─► glimagesink embedded in the window (VideoOverlay)
webrtcbin ─► rtpopusdepay ─► opusdec(PLC, FEC) ─► audioconvert ─► audioresample ─► volume ─► audio sink
```

**Software renderer**: the window (`app-core::client::window`, winit +
softbuffer) keeps only the newest decoded frame; frames replaced before
presentation are counted as dropped. Scaling and YUV→RGB run on the CPU in
the pipeline, then pixels are copied to the window surface.

**GPU renderer** (`client.renderer`: `auto` | `gpu` | `software`): on X11
the window's id is given to `glimagesink`, which draws into a child window
that selects no input events (`handle-events=false`), so keyboard and mouse
still reach our window. Upload, colour conversion, scaling and the stats
overlay (as an overlay-composition meta) run on the GPU. A one-buffer leaky
queue in front of it keeps a slow GPU from stalling the decoder. `auto`
probes the sink once (24 frames of 1080p into the window) and uses it only
below 4 ms per frame, so software OpenGL falls back to the CPU path. Not
offered on Windows (the D3D11 sink's child window would capture mouse
input) or Wayland yet.

The **stats overlay** is linked into the running pipeline from an IDLE pad
probe only while visible (activated before linking, linked without a
caps-check, direct link restored on failure): a hidden `textoverlay` still
cost ~3 ms per 1080p frame.

Audio is played through a ~40 ms device buffer; video is presented as soon as
it is decoded. Both pass the same RTP jitter buffer (default 40 ms, applied
only when packets are missing; measured to add nothing without loss). A/V
offset is not separately measured.

**Loss recovery**: the client enables retransmission requests on every
jitter buffer itself (rtpbin `new-jitterbuffer`): `do-retransmission`,
`rtx-delay` 0 (NACK as soon as a gap is seen) and `rtx-next-seqnum` off
(NACK only real gaps; the next-packet guess misfires between frames).
webrtcbin's own setup did not enable NACK for our bundled streams. The host
answers with RTX. A packet not recovered within the jitter-buffer latency
is declared lost; the decoder conceals and a keyframe can be requested.

## Timing and statistics

All timestamps use the GStreamer monotonic system clock.

* Host: capture time and encoder-input time are attached to each buffer as
  `GstReferenceTimestampMeta` (copied by encoders; PTS is not used as a key
  because encoders offset it). Encode time is measured per frame.
* The capture time is the instant the source sampled the frame (buffer PTS
  + base time on the system clock), so the screen grab is part of the
  measured latency; the grab time itself is reported as `capture_ms_avg`.
* The capture and encode-complete timestamps are embedded in the H.264
  stream as an SEI "user data unregistered" NAL with a Mist UUID (decoders
  ignore it; the client stops scanning at the first slice).
* Per frame the client derives a **latency breakdown**: host (grab +
  convert + encode, host clock only), network (encode-complete → depayloaded,
  via the clock offset), decode, convert (decoded → renderer), present.
* The client estimates the host clock offset NTP-style from ping/pong over
  the control channel (minimum-RTT sample) and computes **capture → display
  latency** per frame. The reported uncertainty is ±RTT/2 of the best sample.
* Transport statistics come from `webrtcbin` `get-stats` (RTP packets, loss,
  jitter, NACK/PLI counts, RTCP RTT, selected ICE candidate types).

## Sessions

* Up to `host.max_clients` sessions (default 8) at once; further requests
  get "the host is full". A device asking for a new session replaces its
  previous one.
* Everyone watches **one shared stream**: `HostStream` captures and encodes
  once (video and audio end in `tee`s), and each session gets a
  `HostViewer`, its own payloaders and `webrtcbin` attached to the running
  pipeline, with a keyframe on join and again when its peer connects.
  The stream starts with the first approved session and stops when the
  last one leaves. Its resolution and frame rate come from the first
  session; later ones receive the same. Each session runs its own
  congestion control, and the encoder uses the **lowest** bitrate any of
  them asks for.
* **Roles**, chosen by the host per person: *play* (gamepads; keyboard and
  mouse only if the host allows it, by default for the first player) or
  *watch* (video and audio). Watchers are told `InputPermission { allowed:
  false }`, an existing message, so older clients need no update; the host
  enforces roles in the `InputDispatcher`.
* One `InputDispatcher` serves everyone, with per-client permissions, held
  input, rate limits and release. Each client numbers its pads from 0; the
  dispatcher maps them onto the host's 4 virtual controller slots (XInput
  limits Windows games to 4) and routes rumble back to the owner's own pad
  number. A pad that finds every slot taken is refused and counted.
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

**Rumble** flows the other way: games' force feedback on the virtual pad
(uinput FF_RUMBLE uploads/plays answered by the input worker every 4 ms;
ViGEm X360 motor notifications on Windows) becomes a `Rumble` control
message, and the client plays it on the physical controller in that slot
with gilrs (strong/weak motors; zero stops it). Only connected pads are
forwarded.

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

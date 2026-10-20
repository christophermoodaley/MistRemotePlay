//! State shared between the async session runner and the renderer /
//! input threads.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::media::timing::{now_us, ClockSync, RateCounter};
use crate::media::{ClientMedia, DataChannelKind, FrameTimestamps, MediaTransport, WindowTarget};
use crate::protocol::encode;
use crate::protocol::input::InputMessage;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum ClientCmd {
    Disconnect,
    SetVolume(f64),
    ToggleMute,
    ToggleOverlay,
    /// A controller event from a source other than the physical-controller
    /// thread (scripted controller in tests); handled like a real one.
    Controller(crate::controller::source::ControllerEvent),
}

/// Where capture-to-display latency is spent (means over the last two
/// seconds, ms). Stages add up to the total; each is measured per frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LatencyBreakdown {
    /// Capture → encoded on the host: colour conversion, queueing, encoding (host clock only).
    pub host_ms: Option<f32>,
    /// Encoded on the host → depayloaded on the client: packetization,
    /// network, jitter buffer (uses the synchronized clock offset).
    pub network_ms: Option<f32>,
    /// Depayloaded → decoded.
    pub decode_ms: Option<f32>,
    /// Decoded → reaches the renderer. Software renderer: CPU scaling,
    /// colour conversion, overlay. GPU renderer: texture upload and
    /// hand-off to the GL thread, up to the draw call.
    pub convert_ms: Option<f32>,
    /// Reaches the renderer → presented. Software renderer: window
    /// wake-up, copy and present. GPU renderer: ~0 (measured at the draw
    /// call; the GPU work and buffer swap that follow are not included).
    pub present_ms: Option<f32>,
    /// Frames in the window.
    pub samples: u32,
}

const STAGES: usize = 6;

/// Rolling per-frame stage samples (`f32::NAN` = not measured).
#[derive(Default)]
struct LatencyWindow {
    samples: VecDeque<(Instant, [f32; STAGES])>,
}

impl LatencyWindow {
    fn push(&mut self, v: [f32; STAGES]) {
        self.samples.push_back((Instant::now(), v));
        while self.samples.len() > 600 {
            self.samples.pop_front();
        }
    }

    /// Mean of each stage over the last two seconds.
    fn means(&mut self) -> ([Option<f32>; STAGES], u32) {
        let cutoff = Instant::now() - Duration::from_secs(2);
        while self.samples.front().map(|(t, _)| *t < cutoff).unwrap_or(false) {
            self.samples.pop_front();
        }
        let mut out = [None; STAGES];
        for (i, slot) in out.iter_mut().enumerate() {
            let vals = self.samples.iter().map(|(_, v)| v[i]).filter(|v| v.is_finite());
            let (sum, n) = vals.fold((0.0f32, 0u32), |(s, n), v| (s + v, n + 1));
            if n > 0 {
                *slot = Some(sum / n as f32);
            }
        }
        (out, self.samples.len() as u32)
    }
}

pub struct ClientShared {
    pub media: RwLock<Option<Arc<ClientMedia>>>,
    pub clock: Mutex<ClockSync>,
    pub input_allowed: AtomicBool,
    pub gamepad_seq: AtomicU32,
    pub frames_presented: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub present_rate: Mutex<RateCounter>,
    latency: Mutex<LatencyWindow>,
    pub stream_size: Mutex<Option<(u32, u32)>>,
    /// Native window the GPU video sink can draw into (set by the window).
    pub video_target: Mutex<Option<WindowTarget>>,
    /// The current media session draws into `video_target` itself; the
    /// software renderer must not paint over it.
    pub gpu_presenting: AtomicBool,
    pub cmd: mpsc::UnboundedSender<ClientCmd>,
}

impl ClientShared {
    pub fn new(cmd: mpsc::UnboundedSender<ClientCmd>) -> Arc<Self> {
        Arc::new(ClientShared {
            media: RwLock::new(None),
            clock: Mutex::new(ClockSync::default()),
            input_allowed: AtomicBool::new(false),
            gamepad_seq: AtomicU32::new(0),
            frames_presented: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            present_rate: Mutex::new(RateCounter::new(Duration::from_secs(2))),
            latency: Mutex::new(LatencyWindow::default()),
            stream_size: Mutex::new(None),
            video_target: Mutex::new(None),
            gpu_presenting: AtomicBool::new(false),
            cmd,
        })
    }

    /// Forward an input event to the host (no-op while disconnected).
    pub fn send_input(&self, msg: InputMessage) {
        let channel = match msg {
            InputMessage::Gamepad { .. } => DataChannelKind::Gamepad,
            _ => DataChannelKind::Input,
        };
        if let Some(m) = self.media.read().as_ref() {
            if let Err(e) = m.send(channel, &encode(&msg)) {
                tracing::trace!("input not sent: {e}");
            }
        }
    }

    pub fn next_gamepad_seq(&self) -> u32 {
        self.gamepad_seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

    /// Called when a frame becomes visible. Records capture-to-display
    /// latency and its stages when the frame carries host timestamps and
    /// the clocks have been synchronized.
    pub fn on_presented(&self, frame: &FrameTimestamps) {
        self.frames_presented.fetch_add(1, Ordering::Relaxed);
        self.present_rate.lock().tick();
        let Some(cap) = frame.capture_host_us else { return };
        let (cap_local, enc_local) = {
            let clock = self.clock.lock();
            let Some(cap_local) = clock.host_to_client(cap) else { return };
            (cap_local, frame.encoded_host_us.and_then(|e| clock.host_to_client(e)))
        };
        let now = now_us();
        if now < cap_local {
            return;
        }
        let ms = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (Some(a), Some(b)) => (b as f64 - a as f64) as f32 / 1000.0,
            _ => f32::NAN,
        };
        let host = ms(Some(cap), frame.encoded_host_us);
        let network = ms(enc_local, frame.arrival_us);
        let decode = ms(frame.arrival_us, frame.decode_end_us);
        let convert = ms(frame.decode_end_us, Some(frame.sink_us));
        let present = ms(Some(frame.sink_us), Some(now));
        let total = (now - cap_local) as f32 / 1000.0;
        self.latency.lock().push([total, host, network, decode, convert, present]);
    }

    /// Mean latency over the last two seconds.
    pub fn recent_latency_ms(&self) -> Option<f32> {
        self.latency.lock().means().0[0]
    }

    pub fn latency_breakdown(&self) -> LatencyBreakdown {
        let ([_, host_ms, network_ms, decode_ms, convert_ms, present_ms], samples) = self.latency.lock().means();
        LatencyBreakdown { host_ms, network_ms, decode_ms, convert_ms, present_ms, samples }
    }

    /// Ask the GPU renderer to redraw (window resized).
    pub fn expose(&self) {
        if let Some(m) = self.media.read().as_ref() {
            m.expose();
        }
    }

    pub fn set_output_size(&self, size: Option<(u32, u32)>) {
        if let Some(m) = self.media.read().as_ref() {
            m.set_output_size(size);
        }
    }
}

//! State shared between the async session runner and the renderer /
//! input threads.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mist_media_engine::timing::{now_us, ClockSync, RateCounter};
use mist_media_engine::{ClientMedia, DataChannelKind, MediaTransport};
use mist_protocol::encode;
use mist_protocol::input::InputMessage;
use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum ClientCmd {
    Disconnect,
    SetVolume(f64),
    ToggleMute,
    ToggleOverlay,
}

pub struct ClientShared {
    pub media: RwLock<Option<Arc<ClientMedia>>>,
    pub clock: Mutex<ClockSync>,
    pub input_allowed: AtomicBool,
    pub gamepad_seq: AtomicU32,
    pub frames_presented: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub present_rate: Mutex<RateCounter>,
    latencies_ms: Mutex<VecDeque<(Instant, f32)>>,
    pub stream_size: Mutex<Option<(u32, u32)>>,
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
            latencies_ms: Mutex::new(VecDeque::new()),
            stream_size: Mutex::new(None),
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

    /// Called when a frame becomes visible. Computes capture-to-display
    /// latency when the frame carries a host capture timestamp and the
    /// clocks have been synchronized.
    pub fn on_presented(&self, capture_host_us: Option<u64>) {
        self.frames_presented.fetch_add(1, Ordering::Relaxed);
        self.present_rate.lock().tick();
        let Some(cap) = capture_host_us else { return };
        let Some(cap_local) = self.clock.lock().host_to_client(cap) else { return };
        let now = now_us();
        if now >= cap_local {
            let ms = (now - cap_local) as f32 / 1000.0;
            let mut l = self.latencies_ms.lock();
            l.push_back((Instant::now(), ms));
            while l.len() > 600 {
                l.pop_front();
            }
        }
    }

    /// Mean latency over the last two seconds.
    pub fn recent_latency_ms(&self) -> Option<f32> {
        let mut l = self.latencies_ms.lock();
        let cutoff = Instant::now() - Duration::from_secs(2);
        while l.front().map(|(t, _)| *t < cutoff).unwrap_or(false) {
            l.pop_front();
        }
        if l.is_empty() {
            None
        } else {
            Some(l.iter().map(|(_, v)| v).sum::<f32>() / l.len() as f32)
        }
    }

    pub fn set_output_size(&self, size: Option<(u32, u32)>) {
        if let Some(m) = self.media.read().as_ref() {
            m.set_output_size(size);
        }
    }
}

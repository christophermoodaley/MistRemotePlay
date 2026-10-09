//! Frame sinks: the window renderer's latest-frame slot, and a headless
//! sink that records what was received (tests, benchmarks).

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use mist_media_engine::{FrameSink, VideoFrame};
use parking_lot::Mutex;
use serde::Serialize;

use super::shared::ClientShared;

/// Keeps only the newest decoded frame; a frame replaced before it was
/// drawn counts as dropped (renderer slower than the stream).
pub struct LatestFrameSink {
    slot: Mutex<Option<VideoFrame>>,
    pending_wake: AtomicBool,
    wake: Box<dyn Fn() + Send + Sync>,
    shared: Arc<ClientShared>,
}

impl LatestFrameSink {
    pub fn new(shared: Arc<ClientShared>, wake: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(LatestFrameSink { slot: Mutex::new(None), pending_wake: AtomicBool::new(false), wake: Box::new(wake), shared })
    }

    pub fn take(&self) -> Option<VideoFrame> {
        self.pending_wake.store(false, Ordering::Release);
        self.slot.lock().take()
    }
}

impl FrameSink for LatestFrameSink {
    fn push(&self, frame: VideoFrame) {
        if self.slot.lock().replace(frame).is_some() {
            self.shared.frames_dropped.fetch_add(1, Ordering::Relaxed);
        }
        if !self.pending_wake.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct HeadlessReport {
    pub frames: u64,
    pub distinct_frames: usize,
    pub width: u32,
    pub height: u32,
    pub frames_with_capture_timestamp: u64,
}

/// Counts frames and distinct contents; "presents" immediately.
pub struct HeadlessSink {
    shared: Arc<ClientShared>,
    hashes: Mutex<HashSet<u64>>,
    report: Mutex<HeadlessReport>,
    frames: AtomicU64,
}

impl HeadlessSink {
    pub fn new(shared: Arc<ClientShared>) -> Arc<Self> {
        Arc::new(HeadlessSink { shared, hashes: Mutex::new(HashSet::new()), report: Mutex::new(Default::default()), frames: AtomicU64::new(0) })
    }

    pub fn report(&self) -> HeadlessReport {
        let mut r = self.report.lock().clone();
        r.frames = self.frames.load(Ordering::Relaxed);
        r.distinct_frames = self.hashes.lock().len();
        r
    }
}

impl FrameSink for HeadlessSink {
    fn push(&self, frame: VideoFrame) {
        self.frames.fetch_add(1, Ordering::Relaxed);
        // Hash a sparse sample of the frame (cheap, detects motion).
        let h = frame.with_data(|d| {
            let mut s = std::collections::hash_map::DefaultHasher::new();
            for chunk in d.chunks(997).step_by(7) {
                chunk.first().hash(&mut s);
                chunk.last().hash(&mut s);
            }
            d.len().hash(&mut s);
            s.finish()
        });
        if let Some(h) = h {
            let mut set = self.hashes.lock();
            if set.len() < 100_000 {
                set.insert(h);
            }
        }
        {
            let mut r = self.report.lock();
            r.width = frame.width;
            r.height = frame.height;
            if frame.capture_host_us.is_some() {
                r.frames_with_capture_timestamp += 1;
            }
        }
        self.shared.on_presented(frame.capture_host_us);
    }
}

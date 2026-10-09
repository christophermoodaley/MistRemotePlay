//! Timing helpers: monotonic clock, PTS-keyed lookups, rolling averages,
//! rate counters and peer clock-offset estimation. All pure logic.

use std::collections::VecDeque;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Microseconds on the GStreamer system clock (CLOCK_MONOTONIC on Linux,
/// QueryPerformanceCounter on Windows). The same clock timestamps buffers.
pub fn now_us() -> u64 {
    use gst::prelude::*;
    gst::SystemClock::obtain().time().useconds()
}

// Per-buffer timestamps carried as GstReferenceTimestampMeta (copied by
// encoders/decoders, unlike PTS which some elements offset).
pub static CAPTURE_REF: LazyLock<gst::Caps> = LazyLock::new(|| gst::Caps::new_empty_simple("timestamp/x-mist-capture"));
pub static DECODE_REF: LazyLock<gst::Caps> =
    LazyLock::new(|| gst::Caps::new_empty_simple("timestamp/x-mist-decode-in"));
pub static ARRIVAL_REF: LazyLock<gst::Caps> = LazyLock::new(|| gst::Caps::new_empty_simple("timestamp/x-mist-arrival"));
pub static ENCODE_REF: LazyLock<gst::Caps> =
    LazyLock::new(|| gst::Caps::new_empty_simple("timestamp/x-mist-encode-in"));

/// Attach `us` under `reference`, replacing any earlier value. Sources such
/// as ximagesrc recycle buffers without clearing metas, so stale entries
/// must be removed rather than appended to.
pub fn add_ref_ts(buf: &mut gst::BufferRef, reference: &gst::Caps, us: u64) {
    buf.foreach_meta_mut(|mut meta| {
        let stale = meta
            .downcast_ref::<gst::ReferenceTimestampMeta>()
            .map(|m| m.reference().can_intersect(reference))
            .unwrap_or(false);
        std::ops::ControlFlow::Continue(if stale {
            gst::buffer::BufferMetaForeachAction::Remove
        } else {
            gst::buffer::BufferMetaForeachAction::Keep
        })
    });
    gst::ReferenceTimestampMeta::add(buf, reference, gst::ClockTime::from_useconds(us), gst::ClockTime::NONE);
}

/// Newest value attached under `reference`.
pub fn ref_ts(buf: &gst::BufferRef, reference: &gst::Caps) -> Option<u64> {
    buf.iter_meta::<gst::ReferenceTimestampMeta>()
        .filter(|m| m.reference().can_intersect(reference))
        .last()
        .map(|m| m.timestamp().useconds())
}

/// Fixed-capacity PTS -> value map for correlating a frame across pipeline
/// stages (capture -> encoder -> payloader).
#[derive(Debug)]
pub struct PtsMap<V> {
    entries: VecDeque<(u64, V)>,
    capacity: usize,
}

impl<V: Copy> PtsMap<V> {
    pub fn new(capacity: usize) -> Self {
        PtsMap { entries: VecDeque::with_capacity(capacity), capacity }
    }

    pub fn insert(&mut self, pts: u64, v: V) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back((pts, v));
    }

    pub fn get(&self, pts: u64) -> Option<V> {
        self.entries.iter().rev().find(|(p, _)| *p == pts).map(|(_, v)| *v)
    }

    /// Remove and return the value for `pts`, dropping anything older.
    pub fn take(&mut self, pts: u64) -> Option<V> {
        let idx = self.entries.iter().position(|(p, _)| *p == pts)?;
        let v = self.entries[idx].1;
        self.entries.drain(..=idx);
        Some(v)
    }
}

/// Exponentially weighted moving average.
#[derive(Debug, Clone, Copy)]
pub struct Ewma {
    value: Option<f64>,
    alpha: f64,
}

impl Ewma {
    pub fn new(alpha: f64) -> Self {
        Ewma { value: None, alpha }
    }

    pub fn add(&mut self, sample: f64) {
        self.value = Some(match self.value {
            None => sample,
            Some(v) => v + self.alpha * (sample - v),
        });
    }

    pub fn get(&self) -> Option<f64> {
        self.value
    }
}

/// Events per second over a sliding window.
#[derive(Debug)]
pub struct RateCounter {
    window: Duration,
    events: VecDeque<Instant>,
    total: u64,
}

impl RateCounter {
    pub fn new(window: Duration) -> Self {
        RateCounter { window, events: VecDeque::new(), total: 0 }
    }

    pub fn tick(&mut self) {
        self.tick_at(Instant::now());
    }

    pub fn tick_at(&mut self, now: Instant) {
        self.total += 1;
        self.events.push_back(now);
        self.trim(now);
    }

    fn trim(&mut self, now: Instant) {
        while let Some(t) = self.events.front() {
            if now.duration_since(*t) > self.window {
                self.events.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn rate(&mut self) -> f64 {
        self.rate_at(Instant::now())
    }

    pub fn rate_at(&mut self, now: Instant) -> f64 {
        self.trim(now);
        self.events.len() as f64 / self.window.as_secs_f64()
    }

    pub fn total(&self) -> u64 {
        self.total
    }
}

/// NTP-style offset estimation from ping/pong exchanges.
///
/// `offset = host_clock - client_clock`. The sample with the smallest RTT in
/// the window is used, since its one-way delays are least asymmetric. The
/// true offset lies within +-RTT/2 of the estimate.
#[derive(Debug, Default)]
pub struct ClockSync {
    samples: VecDeque<(u64 /*rtt*/, i64 /*offset*/)>,
}

impl ClockSync {
    const WINDOW: usize = 16;

    pub fn add_sample(&mut self, client_send_us: u64, host_us: u64, client_recv_us: u64) -> Option<u64> {
        if client_recv_us < client_send_us {
            return None;
        }
        let rtt = client_recv_us - client_send_us;
        let midpoint = client_send_us as i64 + (rtt / 2) as i64;
        let offset = host_us as i64 - midpoint;
        if self.samples.len() == Self::WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back((rtt, offset));
        Some(rtt)
    }

    pub fn offset_us(&self) -> Option<i64> {
        self.samples.iter().min_by_key(|(rtt, _)| *rtt).map(|(_, o)| *o)
    }

    /// Error bound of the current offset estimate.
    pub fn uncertainty_us(&self) -> Option<u64> {
        self.samples.iter().map(|(rtt, _)| *rtt / 2).min()
    }

    pub fn latest_rtt_us(&self) -> Option<u64> {
        self.samples.back().map(|(rtt, _)| *rtt)
    }

    /// Convert a host timestamp to the client clock.
    pub fn host_to_client(&self, host_us: u64) -> Option<u64> {
        let o = self.offset_us()?;
        u64::try_from(host_us as i64 - o).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pts_map_take_drops_older() {
        let mut m = PtsMap::new(3);
        m.insert(1, 10);
        m.insert(2, 20);
        m.insert(3, 30);
        m.insert(4, 40); // evicts 1
        assert_eq!(m.get(1), None);
        assert_eq!(m.take(3), Some(30));
        assert_eq!(m.get(2), None);
        assert_eq!(m.get(4), Some(40));
    }

    #[test]
    fn rate_counter_window() {
        let mut r = RateCounter::new(Duration::from_secs(1));
        let t0 = Instant::now();
        for i in 0..60 {
            r.tick_at(t0 + Duration::from_millis(i * 16));
        }
        let rate = r.rate_at(t0 + Duration::from_millis(960));
        assert!((59.0..=61.0).contains(&rate), "{rate}");
        assert_eq!(r.rate_at(t0 + Duration::from_secs(5)), 0.0);
        assert_eq!(r.total(), 60);
    }

    #[test]
    fn clock_sync_prefers_min_rtt() {
        let mut c = ClockSync::default();
        // host clock is client + 1_000_000us; symmetric 10ms path
        c.add_sample(0, 1_005_000, 10_000);
        // asymmetric slow sample should not win
        c.add_sample(100_000, 1_150_000, 200_000);
        assert_eq!(c.offset_us(), Some(1_000_000));
        assert_eq!(c.uncertainty_us(), Some(5_000));
        assert_eq!(c.host_to_client(1_500_000), Some(500_000));
        assert_eq!(c.add_sample(10, 0, 5), None);
    }

    #[test]
    fn ewma_converges() {
        let mut e = Ewma::new(0.5);
        assert_eq!(e.get(), None);
        e.add(10.0);
        e.add(20.0);
        assert_eq!(e.get(), Some(15.0));
    }
}

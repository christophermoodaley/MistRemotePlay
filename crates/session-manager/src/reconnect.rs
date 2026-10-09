//! Exponential backoff with jitter for signaling and peer reconnects.

use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ReconnectPolicy {
    pub initial: Duration,
    pub max: Duration,
    /// `None` = retry forever.
    pub max_attempts: Option<u32>,
    attempt: u32,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconnectState {
    Connected,
    Waiting { attempt: u32, delay_ms: u64 },
    GaveUp { attempts: u32 },
}

impl ReconnectPolicy {
    pub fn new(initial: Duration, max: Duration, max_attempts: Option<u32>) -> Self {
        ReconnectPolicy { initial, max, max_attempts, attempt: 0 }
    }

    /// Peer-connection recovery: a handful of quick attempts.
    pub fn session() -> Self {
        Self::new(Duration::from_millis(500), Duration::from_secs(8), Some(6))
    }

    /// Signaling connection: keep trying with a capped delay.
    pub fn signaling() -> Self {
        Self::new(Duration::from_secs(1), Duration::from_secs(30), None)
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    pub fn attempts(&self) -> u32 {
        self.attempt
    }

    /// Next delay, or `None` once attempts are exhausted. Jitter (0-25 %)
    /// keeps many clients from reconnecting in lockstep after an outage.
    pub fn next_delay(&mut self) -> Option<Duration> {
        if let Some(max) = self.max_attempts {
            if self.attempt >= max {
                return None;
            }
        }
        let exp = self.initial.saturating_mul(1u32 << self.attempt.min(16));
        let base = exp.min(self.max);
        self.attempt += 1;
        let mut r = [0u8; 1];
        let _ = getrandom::getrandom(&mut r);
        let jitter = base.mul_f64(r[0] as f64 / 255.0 * 0.25);
        Some(base + jitter)
    }

    pub fn state(&self, connected: bool, pending: Option<Duration>) -> ReconnectState {
        match (connected, pending) {
            (true, _) => ReconnectState::Connected,
            (false, Some(d)) => ReconnectState::Waiting { attempt: self.attempt, delay_ms: d.as_millis() as u64 },
            (false, None) => ReconnectState::GaveUp { attempts: self.attempt },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_caps_and_gives_up() {
        let mut p = ReconnectPolicy::new(Duration::from_millis(100), Duration::from_millis(1000), Some(6));
        let delays: Vec<_> = std::iter::from_fn(|| p.next_delay()).collect();
        assert_eq!(delays.len(), 6);
        let bases = [100, 200, 400, 800, 1000, 1000];
        for (d, b) in delays.iter().zip(bases) {
            let ms = d.as_millis() as u64;
            assert!(ms >= b && ms <= b + b / 4 + 1, "{ms} vs {b}");
        }
        assert_eq!(p.next_delay(), None);
        assert_eq!(p.state(false, None), ReconnectState::GaveUp { attempts: 6 });
        p.reset();
        assert!(p.next_delay().is_some());
    }

    #[test]
    fn unlimited_never_gives_up() {
        let mut p = ReconnectPolicy::signaling();
        for _ in 0..100 {
            assert!(p.next_delay().unwrap() <= Duration::from_millis(37_500));
        }
    }
}

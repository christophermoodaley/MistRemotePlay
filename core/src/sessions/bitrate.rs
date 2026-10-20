//! Adaptive bitrate: loss/RTT-driven AIMD controller.
//!
//! Inputs are measurements the host actually has: RTCP receiver-report loss
//! fraction and RTT from webrtcbin, plus the client's reported decoder drops.
//! The controller backs off multiplicatively on congestion and probes upward
//! additively when the path is clean.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default)]
pub struct NetworkSample {
    /// 0.0..=1.0 from RTCP receiver reports.
    pub fraction_lost: Option<f64>,
    pub rtt_ms: Option<f64>,
    /// Frames the client dropped since the previous sample.
    pub client_dropped_frames: u64,
}

#[derive(Debug)]
pub struct BitrateController {
    min_kbps: u32,
    max_kbps: u32,
    current: u32,
    baseline_rtt: Option<f64>,
    last_decrease: Option<Instant>,
    clean_since: Option<Instant>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Decision {
    Hold,
    Decrease(u32),
    Increase(u32),
}

impl BitrateController {
    const LOSS_HIGH: f64 = 0.08;
    const LOSS_LOW: f64 = 0.02;
    const PROBE_AFTER: Duration = Duration::from_secs(4);
    const DECREASE_COOLDOWN: Duration = Duration::from_secs(1);

    pub fn new(min_kbps: u32, max_kbps: u32, start_kbps: u32) -> Self {
        BitrateController {
            min_kbps,
            max_kbps,
            current: start_kbps.clamp(min_kbps, max_kbps),
            baseline_rtt: None,
            last_decrease: None,
            clean_since: None,
        }
    }

    pub fn current_kbps(&self) -> u32 {
        self.current
    }

    pub fn update(&mut self, s: NetworkSample) -> Decision {
        self.update_at(s, Instant::now())
    }

    pub fn update_at(&mut self, s: NetworkSample, now: Instant) -> Decision {
        let loss = s.fraction_lost.unwrap_or(0.0);
        // Track the minimum RTT seen as the uncongested baseline.
        let rtt_inflated = match (s.rtt_ms, self.baseline_rtt) {
            (Some(rtt), Some(base)) => {
                self.baseline_rtt = Some(base.min(rtt));
                rtt > base * 2.0 + 30.0
            }
            (Some(rtt), None) => {
                self.baseline_rtt = Some(rtt);
                false
            }
            _ => false,
        };
        let congested = loss > Self::LOSS_HIGH || rtt_inflated || s.client_dropped_frames > 10;
        if congested {
            self.clean_since = None;
            if self.last_decrease.map(|t| now.duration_since(t) < Self::DECREASE_COOLDOWN).unwrap_or(false) {
                return Decision::Hold;
            }
            let factor = if loss > 0.25 { 0.6 } else { 0.85 };
            let next = ((self.current as f64 * factor) as u32).max(self.min_kbps);
            self.last_decrease = Some(now);
            return self.set(next);
        }
        if loss < Self::LOSS_LOW {
            let since = *self.clean_since.get_or_insert(now);
            if now.duration_since(since) >= Self::PROBE_AFTER {
                let step = (self.current / 20).max(250);
                let next = (self.current + step).min(self.max_kbps);
                return self.set(next);
            }
        } else {
            self.clean_since = None;
        }
        Decision::Hold
    }

    fn set(&mut self, next: u32) -> Decision {
        let prev = self.current;
        self.current = next;
        match next.cmp(&prev) {
            std::cmp::Ordering::Less => Decision::Decrease(next),
            std::cmp::Ordering::Greater => Decision::Increase(next),
            std::cmp::Ordering::Equal => Decision::Hold,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> NetworkSample {
        NetworkSample { fraction_lost: Some(0.0), rtt_ms: Some(20.0), client_dropped_frames: 0 }
    }

    #[test]
    fn backs_off_on_loss_and_respects_min() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(1000, 20_000, 10_000);
        let lossy = NetworkSample { fraction_lost: Some(0.3), ..clean() };
        assert_eq!(c.update_at(lossy, t0), Decision::Decrease(6000));
        // Cooldown prevents collapsing on one burst.
        assert_eq!(c.update_at(lossy, t0 + Duration::from_millis(200)), Decision::Hold);
        let mut t = t0;
        for _ in 0..20 {
            t += Duration::from_secs(2);
            c.update_at(lossy, t);
        }
        assert_eq!(c.current_kbps(), 1000);
    }

    #[test]
    fn probes_up_after_clean_period_and_respects_max() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(1000, 12_000, 10_000);
        assert_eq!(c.update_at(clean(), t0), Decision::Hold);
        assert_eq!(c.update_at(clean(), t0 + Duration::from_secs(2)), Decision::Hold);
        assert_eq!(c.update_at(clean(), t0 + Duration::from_secs(5)), Decision::Increase(10_500));
        let mut t = t0 + Duration::from_secs(5);
        for _ in 0..20 {
            t += Duration::from_secs(1);
            c.update_at(clean(), t);
        }
        assert_eq!(c.current_kbps(), 12_000);
    }

    #[test]
    fn rtt_inflation_counts_as_congestion() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(1000, 20_000, 10_000);
        c.update_at(clean(), t0);
        let bloated = NetworkSample { rtt_ms: Some(200.0), ..clean() };
        assert_eq!(c.update_at(bloated, t0 + Duration::from_secs(2)), Decision::Decrease(8500));
    }

    #[test]
    fn decoder_drops_count_as_congestion() {
        let mut c = BitrateController::new(1000, 20_000, 10_000);
        let s = NetworkSample { client_dropped_frames: 30, ..clean() };
        assert!(matches!(c.update(s), Decision::Decrease(_)));
    }
}

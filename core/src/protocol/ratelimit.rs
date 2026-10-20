//! Token-bucket rate limiter used for signaling and input validation.

use std::time::Instant;

#[derive(Debug, Clone)]
pub struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(rate_per_sec: f64, burst: f64) -> Self {
        TokenBucket { capacity: burst, tokens: burst, refill_per_sec: rate_per_sec, last: Instant::now() }
    }

    pub fn try_take(&mut self) -> bool {
        self.try_take_at(Instant::now())
    }

    pub fn try_take_at(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn burst_then_refill() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(10.0, 5.0);
        b.last = t0;
        for _ in 0..5 {
            assert!(b.try_take_at(t0));
        }
        assert!(!b.try_take_at(t0));
        assert!(b.try_take_at(t0 + Duration::from_millis(100)));
        assert!(!b.try_take_at(t0 + Duration::from_millis(100)));
        // Long idle never exceeds burst capacity.
        let later = t0 + Duration::from_secs(60);
        for _ in 0..5 {
            assert!(b.try_take_at(later));
        }
        assert!(!b.try_take_at(later));
    }
}

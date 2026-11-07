//! Exponential backoff with jitter, shared by push reconnects, polling
//! retries and token-refresh retries.

use std::time::Duration;

use rand::Rng;

/// How retry delays grow: `initial × multiplier^attempt`, capped at `max`,
/// then spread by ±`jitter` (a fraction of the delay).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BackoffPolicy {
    pub initial: Duration,
    pub max: Duration,
    pub multiplier: f64,
    pub jitter: f64,
}

impl BackoffPolicy {
    /// Push reconnects and polling errors: reconnect after 1500 ms;
    /// repeated failures
    /// double that up to a minute so an offline machine does not spin.
    pub const RECONNECT: Self = Self {
        initial: Duration::from_millis(1500),
        max: Duration::from_secs(60),
        multiplier: 2.0,
        jitter: 0.2,
    };

    /// Delay before retry number `attempt` (0 = first retry). `unit` in
    /// `[-1, 1]` picks the point in the jitter range.
    pub fn delay(&self, attempt: u32, unit: f64) -> Duration {
        let base =
            self.initial.as_secs_f64() * self.multiplier.max(1.0).powi(attempt.min(64) as i32);
        let capped = base.min(self.max.as_secs_f64());
        let jittered = capped * (1.0 + self.jitter.clamp(0.0, 1.0) * unit.clamp(-1.0, 1.0));
        Duration::from_secs_f64(jittered.max(0.0))
    }
}

/// Retry state for one [`BackoffPolicy`].
#[derive(Debug, Clone)]
pub(crate) struct Backoff {
    policy: BackoffPolicy,
    attempt: u32,
}

impl Backoff {
    pub fn new(policy: BackoffPolicy) -> Self {
        Self { policy, attempt: 0 }
    }

    /// Delay before the next retry; each call moves one step up.
    pub fn next_delay(&mut self) -> Duration {
        let unit = rand::thread_rng().gen_range(-1.0..=1.0);
        let d = self.policy.delay(self.attempt, unit);
        self.attempt = self.attempt.saturating_add(1);
        d
    }

    /// Start over from `initial` (after a success).
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_schedule_doubles_from_1500ms_to_a_minute() {
        let p = BackoffPolicy::RECONNECT;
        let secs: Vec<f64> = (0..8).map(|a| p.delay(a, 0.0).as_secs_f64()).collect();
        assert_eq!(secs, [1.5, 3.0, 6.0, 12.0, 24.0, 48.0, 60.0, 60.0]);
        assert_eq!(p.delay(1000, 0.0), Duration::from_secs(60));
    }

    #[test]
    fn jitter_stays_within_range() {
        let p = BackoffPolicy::RECONNECT;
        assert_eq!(p.delay(0, -1.0), Duration::from_millis(1200));
        assert_eq!(p.delay(0, 1.0), Duration::from_millis(1800));
        let mut b = Backoff::new(p);
        for attempt in 0..10 {
            let d = b.next_delay();
            let mid = p.delay(attempt, 0.0).as_secs_f64();
            assert!(
                (mid * 0.8 - 1e-9..=mid * 1.2 + 1e-9).contains(&d.as_secs_f64()),
                "{d:?} outside ±20% of {mid}"
            );
        }
        b.reset();
        assert!(b.next_delay() <= Duration::from_millis(1800));
    }
}

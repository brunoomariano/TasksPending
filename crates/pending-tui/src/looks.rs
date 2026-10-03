//! Telling the dashboard that the user is looking, without telling it on
//! every key: it only needs to know roughly when the last activity was.

use std::time::{Duration, Instant};

/// Activity is reported at most this often.
pub const LOOK_EVERY: Duration = Duration::from_secs(60);

/// Decides which key presses and mouse events are reported as a look.
#[derive(Debug, Default)]
pub struct LookThrottle {
    /// When the last look was reported.
    last: Option<Instant>,
}

impl LookThrottle {
    /// Whether activity at `now` should be reported: the first one, and then
    /// one every [`LOOK_EVERY`] at most. Answering yes counts as reported.
    pub fn due(&mut self, now: Instant) -> bool {
        let due = self
            .last
            .is_none_or(|last| now.saturating_duration_since(last) >= LOOK_EVERY);
        if due {
            self.last = Some(now);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first activity is reported; more of it within a minute is not;
    /// after a minute the next one is, and the minute counts again from
    /// there (not from the activity in between).
    #[test]
    fn activity_is_reported_at_most_once_a_minute() {
        let mut throttle = LookThrottle::default();
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);

        assert!(throttle.due(at(0)), "the first activity");
        assert!(!throttle.due(at(1)));
        assert!(!throttle.due(at(59)));
        assert!(throttle.due(at(60)), "a minute later");
        assert!(!throttle.due(at(119)), "counts from the last report");
        assert!(throttle.due(at(120)));
        assert!(throttle.due(at(3600)), "after a long pause");
        assert!(!throttle.due(at(3600)));
    }
}

//! The guard on prompts (D242 (5)): a prompt does not accept a cancel key in its first
//! second, nor, when its confirmation runs a side effect, its confirm key. Keys in that
//! second are dropped, not queued, so a key meant for the screen behind cannot answer it.

use std::time::{Duration, Instant};

/// How long a prompt waits before it accepts an answer that ends it.
pub const PROMPT_GUARD: Duration = Duration::from_secs(1);

/// What a key does to a prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Cancel,
    Confirm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guard {
    opened: Instant,
    wait: Duration,
    /// The confirmation runs a side effect, so it waits too.
    side_effect: bool,
}

impl Guard {
    pub fn new(opened: Instant, side_effect: bool) -> Self {
        Self::with_wait(opened, side_effect, PROMPT_GUARD)
    }

    pub fn with_wait(opened: Instant, side_effect: bool, wait: Duration) -> Self {
        Self {
            opened,
            wait,
            side_effect,
        }
    }

    /// Whether `answer` is taken at `now`. A key refused here is dropped by the caller.
    pub fn accepts(&self, answer: Answer, now: Instant) -> bool {
        let waited = now.saturating_duration_since(self.opened) >= self.wait;
        match answer {
            Answer::Cancel => waited,
            Answer::Confirm => waited || !self.side_effect,
        }
    }

    /// When the guard opens, for drawing the keys dimmed until then and for the next draw.
    pub fn opens_at(&self) -> Instant {
        self.opened + self.wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_waits_a_second_before_it_can_be_cancelled() {
        let opened = Instant::now();
        let guard = Guard::new(opened, false);
        assert!(!guard.accepts(Answer::Cancel, opened));
        assert!(!guard.accepts(Answer::Cancel, opened + Duration::from_millis(999)));
        assert!(guard.accepts(Answer::Cancel, opened + Duration::from_secs(1)));
        // A confirmation with no side effect, such as sending a search, is taken at once.
        assert!(guard.accepts(Answer::Confirm, opened));
    }

    #[test]
    fn a_side_effect_waits_before_it_is_confirmed() {
        let opened = Instant::now();
        let guard = Guard::new(opened, true);
        assert!(!guard.accepts(Answer::Confirm, opened + Duration::from_millis(500)));
        assert!(guard.accepts(Answer::Confirm, opened + Duration::from_millis(1000)));
        assert_eq!(guard.opens_at(), opened + PROMPT_GUARD);
    }
}

//! When a change becomes a refresh. A pure state machine: the caller passes
//! the time in, so the rules are tested with fixed instants and no sleeps.
//!
//! - Changes wait for a quiet period (`debounce`), but never longer than
//!   `max_wait` from the first one, so a steady stream still refreshes.
//! - One refresh is in flight at a time. A change during it schedules exactly
//!   one more, after it ends.
//! - A status that loads without being ours means the app just wrote the
//!   repository. The changes it showed are dropped, and the changes that
//!   follow within one debounce period are folded: they are the write's own
//!   events arriving late. The engine's reload already shows that write.

use std::time::{Duration, Instant};

use gm_schema::OperationId;

#[derive(Clone, Copy, Debug)]
struct Span {
    first: Instant,
    last: Instant,
}

#[derive(Debug)]
pub struct Coalescer {
    debounce: Duration,
    max_wait: Duration,
    pending: Option<Span>,
    /// Changes before this instant are the app's own writes; see the module docs.
    quiet_until: Option<Instant>,
    in_flight: Option<OperationId>,
    /// A change arrived while a refresh was in flight.
    dirty: bool,
}

impl Coalescer {
    pub fn new(debounce: Duration, max_wait: Duration) -> Self {
        Self {
            debounce,
            max_wait,
            pending: None,
            quiet_until: None,
            in_flight: None,
            dirty: false,
        }
    }

    /// A file or git metadata change that the status may show.
    pub fn change(&mut self, now: Instant) {
        if self.in_flight.is_some() {
            self.dirty = true;
        } else if self.quiet_until.is_none_or(|until| now >= until) {
            self.record(now);
        }
    }

    /// An explicit request, such as the window gaining focus. Not folded
    /// into the quiet period: the user asked for it.
    pub fn request(&mut self, now: Instant) {
        if self.in_flight.is_some() {
            self.dirty = true;
        } else {
            self.record(now);
        }
    }

    /// A status loaded under `id`. Ours is ignored here; any other comes from
    /// the app's own commands or from an open.
    pub fn status_loaded(&mut self, id: OperationId, now: Instant) {
        if self.in_flight == Some(id) {
            return;
        }
        self.pending = None;
        self.quiet_until = Some(now + self.debounce);
    }

    /// When the pending refresh is due. `None` when nothing waits or a
    /// refresh is in flight.
    pub fn due(&self) -> Option<Instant> {
        if self.in_flight.is_some() {
            return None;
        }
        self.pending
            .map(|span| (span.last + self.debounce).min(span.first + self.max_wait))
    }

    /// Whether a refresh should start now. While the engine is busy the
    /// caller polls again: a status started behind a write would repeat it.
    pub fn should_start(&self, now: Instant, engine_busy: bool) -> bool {
        !engine_busy && self.due().is_some_and(|due| now >= due)
    }

    /// Our refresh was dispatched with `id`.
    pub fn started(&mut self, id: OperationId) {
        self.pending = None;
        self.in_flight = Some(id);
        self.dirty = false;
    }

    /// Our refresh reached a final state. A change seen while it ran waits
    /// one debounce period, then a single refresh runs.
    pub fn finished(&mut self, id: OperationId, now: Instant) {
        if self.in_flight != Some(id) {
            return;
        }
        self.in_flight = None;
        if self.dirty {
            self.dirty = false;
            self.record(now);
        }
    }

    fn record(&mut self, now: Instant) {
        self.pending = Some(match self.pending {
            Some(span) => Span {
                first: span.first,
                last: now,
            },
            None => Span {
                first: now,
                last: now,
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEBOUNCE: Duration = Duration::from_millis(150);
    const MAX_WAIT: Duration = Duration::from_millis(1000);

    fn coalescer() -> Coalescer {
        Coalescer::new(DEBOUNCE, MAX_WAIT)
    }

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn nothing_pending_means_nothing_to_start() {
        let base = Instant::now();
        let c = coalescer();
        assert_eq!(c.due(), None);
        assert!(!c.should_start(at(base, 10_000), false));
    }

    #[test]
    fn one_change_waits_for_the_quiet_period() {
        let base = Instant::now();
        let mut c = coalescer();
        c.change(at(base, 0));
        assert!(!c.should_start(at(base, 149), false));
        assert!(c.should_start(at(base, 150), false));
    }

    #[test]
    fn a_burst_inside_the_window_is_one_refresh_at_its_end() {
        let base = Instant::now();
        let mut c = coalescer();
        for ms in [0, 40, 80, 120] {
            c.change(at(base, ms));
        }
        assert_eq!(c.due(), Some(at(base, 270)));
        assert!(!c.should_start(at(base, 269), false));
        assert!(c.should_start(at(base, 270), false));
    }

    #[test]
    fn a_steady_stream_refreshes_at_the_max_wait() {
        let base = Instant::now();
        let mut c = coalescer();
        for ms in (0..=1000).step_by(50) {
            c.change(at(base, ms));
        }
        assert_eq!(c.due(), Some(at(base, 1000)));
    }

    #[test]
    fn a_busy_engine_defers_the_start() {
        let base = Instant::now();
        let mut c = coalescer();
        c.change(at(base, 0));
        assert!(!c.should_start(at(base, 500), true));
        assert!(c.should_start(at(base, 500), false));
    }

    #[test]
    fn a_change_during_a_refresh_schedules_exactly_one_more() {
        let base = Instant::now();
        let mut c = coalescer();
        c.change(at(base, 0));
        c.started(OperationId(7));
        c.change(at(base, 200));
        c.change(at(base, 210));
        c.change(at(base, 220));
        assert_eq!(c.due(), None, "nothing starts while one is in flight");
        c.finished(OperationId(7), at(base, 300));
        assert_eq!(c.due(), Some(at(base, 450)));
        c.started(OperationId(8));
        c.finished(OperationId(8), at(base, 460));
        assert_eq!(
            c.due(),
            None,
            "no change after the second refresh, so no third"
        );
    }

    #[test]
    fn no_change_during_a_refresh_schedules_nothing() {
        let base = Instant::now();
        let mut c = coalescer();
        c.request(at(base, 0));
        c.started(OperationId(1));
        c.finished(OperationId(1), at(base, 100));
        assert_eq!(c.due(), None);
    }

    #[test]
    fn a_finish_for_another_id_is_ignored() {
        let base = Instant::now();
        let mut c = coalescer();
        c.started(OperationId(1));
        c.change(at(base, 0));
        c.finished(OperationId(2), at(base, 10));
        assert_eq!(c.due(), None);
        c.finished(OperationId(1), at(base, 20));
        assert_eq!(c.due(), Some(at(base, 170)));
    }

    #[test]
    fn a_foreign_status_drops_what_it_already_showed() {
        let base = Instant::now();
        let mut c = coalescer();
        c.change(at(base, 0));
        c.status_loaded(OperationId(5), at(base, 50));
        assert_eq!(c.due(), None);
    }

    #[test]
    fn changes_right_after_a_foreign_status_are_folded() {
        let base = Instant::now();
        let mut c = coalescer();
        c.status_loaded(OperationId(5), at(base, 0));
        c.change(at(base, 60));
        assert_eq!(c.due(), None, "late events of the app's own write");
        c.change(at(base, 150));
        assert_eq!(c.due(), Some(at(base, 300)), "after the quiet period again");
    }

    #[test]
    fn a_focus_request_is_not_folded_into_the_quiet_period() {
        let base = Instant::now();
        let mut c = coalescer();
        c.status_loaded(OperationId(5), at(base, 0));
        c.request(at(base, 10));
        assert_eq!(c.due(), Some(at(base, 160)));
    }

    #[test]
    fn our_own_status_does_not_open_a_quiet_period() {
        let base = Instant::now();
        let mut c = coalescer();
        c.started(OperationId(3));
        c.status_loaded(OperationId(3), at(base, 0));
        c.finished(OperationId(3), at(base, 1));
        c.change(at(base, 10));
        assert_eq!(c.due(), Some(at(base, 160)));
    }
}

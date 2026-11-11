//! The code health state of the window (docs/guide/code-health.md, D225): the report the engine
//! answered for HEAD, the progress of the index, and the lookups the Code page's tree needs to
//! put a heat dot on a file or a folder. Nothing here touches GPUI.
//!
//! The engine answers `LoadHealth` with progress, then a partial report, then the final one. This
//! state keeps only the answers to the read in flight (`pending`); an answer to any other read is
//! dropped, as everywhere else.

use std::collections::HashMap;
use std::sync::Arc;

use gm_schema::{FileHealth, HealthProgress, HealthReport, OperationId};

/// The band of a file's heat dot: the hottest 1%, 5% or 10% of the files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeatBand {
    Top1,
    Top5,
    Top10,
}

impl HeatBand {
    /// The band of a file at `percentile` (the share of files colder than it, 0 to 100), or
    /// `None` below the hottest tenth.
    #[must_use]
    pub(crate) fn of_percentile(percentile: f64) -> Option<Self> {
        if percentile >= 99.0 {
            Some(Self::Top1)
        } else if percentile >= 95.0 {
            Some(Self::Top5)
        } else if percentile >= 90.0 {
            Some(Self::Top10)
        } else {
            None
        }
    }
}

/// What the Code page shows of code health: the report, the index progress, the error.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct HealthState {
    report: Option<Arc<HealthReport>>,
    progress: Option<HealthProgress>,
    /// The `LoadHealth` read in flight; its answers are the only ones kept.
    pending: Option<OperationId>,
    error: Option<String>,
    /// The index of each file of the report by its path.
    paths: HashMap<String, usize>,
    /// For each folder, the band and rank of the hottest banded file inside it.
    folders: HashMap<String, (HeatBand, u32)>,
}

impl HealthState {
    /// A read of the health was asked for under `id`. A new request replaces an old error.
    pub(crate) fn requested(&mut self, id: OperationId) {
        self.pending = Some(id);
        self.progress = None;
        self.error = None;
    }

    /// The index of the read `id` filled up to `progress`.
    pub(crate) fn progress(&mut self, id: OperationId, progress: HealthProgress) {
        if self.pending == Some(id) {
            self.progress = Some(progress);
        }
    }

    /// The read `id` answered with `report`. A partial report is shown while the read goes on; a
    /// complete one ends the read.
    pub(crate) fn loaded(&mut self, id: OperationId, report: Box<HealthReport>) {
        if self.pending != Some(id) {
            return;
        }
        let complete = report.complete;
        let (paths, folders) = lookups(&report);
        self.paths = paths;
        self.folders = folders;
        self.report = Some(Arc::new(*report));
        if complete {
            self.pending = None;
            self.progress = None;
        }
    }

    /// The read `id` failed with `message`.
    /// Returns whether `id` was the health read, so the caller leaves the failure to this state.
    pub(crate) fn failed(&mut self, id: OperationId, message: String) -> bool {
        if self.pending == Some(id) {
            self.pending = None;
            self.progress = None;
            self.error = Some(message);
            return true;
        }
        false
    }

    /// The read `id` was cancelled: nothing is in flight any more, and nothing failed.
    pub(crate) fn cancelled(&mut self, id: OperationId) {
        if self.pending == Some(id) {
            self.pending = None;
            self.progress = None;
        }
    }

    /// Whether a read is in flight.
    #[must_use]
    pub(crate) fn pending(&self) -> bool {
        self.pending.is_some()
    }

    /// The band and rank of the file at `path`, when it is in the hottest tenth.
    #[must_use]
    pub(crate) fn file_band(&self, path: &str) -> Option<(HeatBand, u32)> {
        let report = self.report.as_ref()?;
        let file: &FileHealth = &report.files[*self.paths.get(path)?];
        HeatBand::of_percentile(file.percentile).map(|band| (band, file.rank))
    }

    /// The band and rank of the hottest file inside the folder `folder`, when there is one.
    #[must_use]
    pub(crate) fn folder_band(&self, folder: &str) -> Option<(HeatBand, u32)> {
        self.folders.get(folder).copied()
    }

    /// How many files the report holds.
    #[must_use]
    pub(crate) fn of(&self) -> usize {
        self.report.as_ref().map_or(0, |report| report.files.len())
    }

    /// The line the Code page shows about the read, or `None` when there is nothing to say.
    #[must_use]
    pub(crate) fn status_text(&self) -> Option<String> {
        if let Some(message) = &self.error {
            return Some(format!("Code health failed: {message}"));
        }
        if self.pending()
            && let Some(progress) = self.progress
        {
            return Some(format!(
                "Reading history: {} of {} commits",
                grouped(progress.indexed),
                grouped(progress.total)
            ));
        }
        if self.report.as_ref().is_some_and(|report| !report.complete) {
            return Some("Older history is still being read; answers are partial.".to_owned());
        }
        None
    }

    /// Forgets everything: another repository opened.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The index of each file by path, and for each folder the hottest banded file inside it.
fn lookups(report: &HealthReport) -> (HashMap<String, usize>, HashMap<String, (HeatBand, u32)>) {
    let mut paths = HashMap::new();
    let mut folders: HashMap<String, (HeatBand, u32)> = HashMap::new();
    for (index, file) in report.files.iter().enumerate() {
        paths.insert(file.path.clone(), index);
        let Some(band) = HeatBand::of_percentile(file.percentile) else {
            continue;
        };
        for folder in ancestors(&file.path) {
            let best = folders
                .entry(folder.to_owned())
                .or_insert((band, file.rank));
            if file.rank < best.1 {
                *best = (band, file.rank);
            }
        }
    }
    (paths, folders)
}

/// The folders above `path`, outermost first: `a` and `a/b` for `a/b/c.rs`.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(move |(at, _)| &path[..at])
}

/// A count with a comma between each group of three digits: `12,340`.
fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{Ownership, Trend};

    fn file(path: &str, rank: u32, percentile: f64) -> FileHealth {
        FileHealth {
            path: path.to_owned(),
            rank,
            percentile,
            heat: 1.0,
            trend: Trend::Steady,
            spark: [0; gm_schema::SPARK_BARS],
            dormancy_break: None,
            changes_together: Vec::new(),
            ownership: Ownership {
                authors: 1,
                top_author_share: 1.0,
                minor_contributors: 0,
            },
            constraints: Vec::new(),
        }
    }

    fn report(files: Vec<FileHealth>, complete: bool) -> HealthReport {
        HealthReport {
            rev: "a1b2c3d4".to_owned(),
            config_hash: "3fa9e1c2".to_owned(),
            complete,
            files,
        }
    }

    fn progress(indexed: u64, total: u64) -> HealthProgress {
        HealthProgress { indexed, total }
    }

    /// Two folders of banded files and one cold file, hottest first.
    fn sample() -> HealthState {
        let mut state = HealthState::default();
        state.requested(OperationId(1));
        state.loaded(
            OperationId(1),
            Box::new(report(
                vec![
                    file("a/b/d.rs", 1, 99.5),
                    file("a/x.rs", 2, 96.0),
                    file("a/b/c.rs", 3, 91.0),
                    file("ab/z.rs", 4, 90.5),
                    file("cold.rs", 5, 10.0),
                ],
                true,
            )),
        );
        state
    }

    #[test]
    fn bands_follow_the_thresholds() {
        assert_eq!(HeatBand::of_percentile(100.0), Some(HeatBand::Top1));
        assert_eq!(HeatBand::of_percentile(99.0), Some(HeatBand::Top1));
        assert_eq!(HeatBand::of_percentile(98.9), Some(HeatBand::Top5));
        assert_eq!(HeatBand::of_percentile(95.0), Some(HeatBand::Top5));
        assert_eq!(HeatBand::of_percentile(94.9), Some(HeatBand::Top10));
        assert_eq!(HeatBand::of_percentile(90.0), Some(HeatBand::Top10));
        assert_eq!(HeatBand::of_percentile(89.9), None);
        assert_eq!(HeatBand::of_percentile(0.0), None);
    }

    #[test]
    fn a_file_has_its_band_and_rank() {
        let state = sample();
        assert_eq!(state.file_band("a/x.rs"), Some((HeatBand::Top5, 2)));
        assert_eq!(state.file_band("a/b/c.rs"), Some((HeatBand::Top10, 3)));
        assert_eq!(state.file_band("cold.rs"), None);
        assert_eq!(state.file_band("missing.rs"), None);
        assert_eq!(state.of(), 5);
    }

    #[test]
    fn a_folder_takes_the_hottest_file_below_it() {
        let state = sample();
        // `a/b` holds rank 1 and rank 3; `a` holds all of them and rank 2.
        assert_eq!(state.folder_band("a/b"), Some((HeatBand::Top1, 1)));
        assert_eq!(state.folder_band("a"), Some((HeatBand::Top1, 1)));
    }

    #[test]
    fn a_folder_takes_the_best_rank_even_when_listed_later() {
        let mut state = HealthState::default();
        state.requested(OperationId(2));
        state.loaded(
            OperationId(2),
            Box::new(report(
                vec![file("src/a.rs", 4, 91.0), file("src/deep/b.rs", 2, 96.0)],
                true,
            )),
        );
        assert_eq!(state.folder_band("src"), Some((HeatBand::Top5, 2)));
        assert_eq!(state.folder_band("src/deep"), Some((HeatBand::Top5, 2)));
    }

    #[test]
    fn a_folder_of_cold_files_has_no_band_and_a_prefix_is_not_a_folder() {
        let state = sample();
        assert_eq!(state.folder_band("cold.rs"), None);
        // `ab/z.rs` is banded, but `a` is not its folder, and `ab` is not a prefix of `a/`.
        assert_eq!(state.folder_band("ab"), Some((HeatBand::Top10, 4)));
        assert_eq!(state.folder_band("a/b/c.rs"), None);
    }

    #[test]
    fn a_stale_answer_is_ignored() {
        let mut state = HealthState::default();
        state.requested(OperationId(1));
        state.progress(OperationId(9), progress(1, 2));
        state.loaded(
            OperationId(9),
            Box::new(report(vec![file("x.rs", 1, 99.0)], true)),
        );
        assert!(state.pending());
        assert_eq!(state.of(), 0);
        assert_eq!(state.status_text(), None);
        state.failed(OperationId(9), "late".to_owned());
        assert!(state.pending());
        assert_eq!(state.status_text(), None);
    }

    #[test]
    fn a_partial_report_is_shown_and_the_read_goes_on() {
        let mut state = HealthState::default();
        state.requested(OperationId(3));
        state.loaded(
            OperationId(3),
            Box::new(report(vec![file("x.rs", 1, 99.0)], false)),
        );
        assert!(state.pending());
        assert_eq!(state.file_band("x.rs"), Some((HeatBand::Top1, 1)));
        assert_eq!(
            state.status_text().as_deref(),
            Some("Older history is still being read; answers are partial.")
        );
    }

    #[test]
    fn a_complete_report_ends_the_read_and_the_text() {
        let mut state = HealthState::default();
        state.requested(OperationId(3));
        state.progress(OperationId(3), progress(5, 10));
        state.loaded(
            OperationId(3),
            Box::new(report(vec![file("x.rs", 1, 99.0)], true)),
        );
        assert!(!state.pending());
        assert_eq!(state.status_text(), None);
        assert_eq!(state.of(), 1);
    }

    #[test]
    fn progress_says_how_many_commits_are_read() {
        let mut state = HealthState::default();
        state.requested(OperationId(4));
        state.progress(OperationId(4), progress(12_340, 20_000));
        assert_eq!(
            state.status_text().as_deref(),
            Some("Reading history: 12,340 of 20,000 commits")
        );
    }

    #[test]
    fn a_failure_is_its_message_and_ends_the_read() {
        let mut state = HealthState::default();
        state.requested(OperationId(5));
        state.failed(OperationId(5), "fatal: bad object".to_owned());
        assert!(!state.pending());
        assert_eq!(
            state.status_text().as_deref(),
            Some("Code health failed: fatal: bad object")
        );
    }

    #[test]
    fn a_new_request_replaces_the_error() {
        let mut state = HealthState::default();
        state.requested(OperationId(5));
        state.failed(OperationId(5), "boom".to_owned());
        state.requested(OperationId(6));
        assert!(state.pending());
        assert_eq!(state.status_text(), None);
    }

    #[test]
    fn a_cancelled_read_ends_without_an_error() {
        let mut state = HealthState::default();
        state.requested(OperationId(7));
        state.cancelled(OperationId(8));
        assert!(state.pending());
        state.cancelled(OperationId(7));
        assert!(!state.pending());
        assert_eq!(state.status_text(), None);
    }

    #[test]
    fn clear_forgets_the_report_and_the_read() {
        let mut state = sample();
        state.requested(OperationId(10));
        state.clear();
        assert!(!state.pending());
        assert_eq!(state.of(), 0);
        assert_eq!(state.file_band("a/x.rs"), None);
        assert_eq!(state.folder_band("a"), None);
        assert_eq!(state, HealthState::default());
    }

    #[test]
    fn counts_have_thousands_separators() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(12_340), "12,340");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}

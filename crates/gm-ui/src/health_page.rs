//! The code health state of the window (docs/guide/code-health.md, D225): the report the engine
//! answered for HEAD, the progress of the index, and the lookups the Code page's tree needs to
//! put a heat dot on a file or a folder. Nothing here touches GPUI.
//!
//! The engine answers `LoadHealth` with progress, then a partial report, then the final one. This
//! state keeps only the answers to the read in flight (`pending`); an answer to any other read is
//! dropped, as everywhere else.

use std::collections::HashMap;
use std::sync::Arc;

use gm_schema::{
    DormancyBreak, FileHealth, HealthProgress, HealthReport, OperationId, Ownership, Partner,
    SPARK_BARS, Trend,
};

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

/// The dormancy breaks the Health tab lists before it says how many more there are.
pub(crate) const DORMANT_SHOWN: usize = 5;

/// What the Code page shows of code health: the report, the index progress, the error, and the
/// Health tab's filter and selection.
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
    /// The folder the Health tab's list is limited to, as typed.
    filter: String,
    /// The path of the file the Health tab's details show.
    selected: Option<String>,
    /// The indices of the files under the filter, in rank order. Kept by `refresh`.
    rows: Vec<usize>,
    /// The indices of the filtered files with a dormancy break, in rank order. Kept by `refresh`.
    dormant: Vec<usize>,
    /// The heat of the hottest file of the report. Kept by `refresh`.
    max_heat: f64,
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
        self.refresh();
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

    /// Whether a complete or partial report is held.
    #[must_use]
    pub(crate) fn has_report(&self) -> bool {
        self.report.is_some()
    }

    /// The file at `index` of the report, in rank order.
    #[must_use]
    pub(crate) fn file(&self, index: usize) -> Option<&FileHealth> {
        self.report.as_ref()?.files.get(index)
    }

    /// The folder the list is limited to, as typed.
    #[must_use]
    pub(crate) fn filter(&self) -> &str {
        &self.filter
    }

    /// Limits the list to the folder `text` names. Returns whether the filter changed; a changed
    /// filter recomputes the rows, an unchanged one costs nothing.
    pub(crate) fn set_filter(&mut self, text: &str) -> bool {
        if self.filter == text {
            return false;
        }
        text.clone_into(&mut self.filter);
        self.refresh();
        true
    }

    /// The indices of the files under the filter, hottest first. Empty filter: every file.
    #[must_use]
    pub(crate) fn rows(&self) -> &[usize] {
        &self.rows
    }

    /// The indices of the files under the filter that have a dormancy break, hottest first.
    #[must_use]
    pub(crate) fn dormant(&self) -> &[usize] {
        &self.dormant
    }

    /// The heat of the hottest file of the report: the full scale of the heat bars.
    #[must_use]
    pub(crate) fn max_heat(&self) -> f64 {
        self.max_heat
    }

    /// The path of the file the details show.
    #[must_use]
    pub(crate) fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Shows the details of the file at `path`.
    pub(crate) fn select(&mut self, path: &str) {
        self.selected = Some(path.to_owned());
    }

    /// The file the details show, when it is in the report.
    #[must_use]
    pub(crate) fn selected_file(&self) -> Option<&FileHealth> {
        let index = *self.paths.get(self.selected.as_deref()?)?;
        self.file(index)
    }

    /// Recomputes what the list keeps from the report and the filter, and the heat scale.
    fn refresh(&mut self) {
        self.rows.clear();
        self.dormant.clear();
        self.max_heat = 0.0;
        let Some(report) = self.report.as_deref() else {
            return;
        };
        let scope = scope_of(&self.filter);
        self.max_heat = report
            .files
            .iter()
            .map(|file| file.heat)
            .fold(0.0, f64::max);
        self.rows = report
            .files
            .iter()
            .enumerate()
            .filter(|(_, file)| is_under(&file.path, scope))
            .map(|(index, _)| index)
            .collect();
        self.dormant = self
            .rows
            .iter()
            .copied()
            .filter(|&index| report.files[index].dormancy_break.is_some())
            .collect();
        self.dormant.sort_by_key(|&index| report.files[index].rank);
    }

    /// Forgets everything: another repository opened.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The folder a filter names: surrounding spaces and slashes left out. Empty for no folder.
#[must_use]
pub(crate) fn scope_of(filter: &str) -> &str {
    filter.trim().trim_matches('/')
}

/// Whether `path` is `scope` or lies under it, on a segment boundary: `src/billing` holds
/// `src/billing/a.rs` but not `src/billing2/a.rs`. An empty scope holds every path.
#[must_use]
pub(crate) fn is_under(path: &str, scope: &str) -> bool {
    scope.is_empty()
        || path == scope
        || path
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// A file's heat as a share of the hottest file's, 0 to 1, for the width of its heat bar.
#[must_use]
pub(crate) fn heat_share(heat: f64, max_heat: f64) -> f64 {
    if max_heat > 0.0 {
        (heat / max_heat).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The heights in pixels of a file's activity bars: each share of the file's own busiest bar,
/// `max_px` for that bar. A bar with commits is at least one pixel high.
#[must_use]
pub(crate) fn spark_heights(spark: &[u32; SPARK_BARS], max_px: u32) -> [u32; SPARK_BARS] {
    let mut heights = [0; SPARK_BARS];
    let top = spark.iter().copied().max().unwrap_or(0);
    if top == 0 {
        return heights;
    }
    for (height, &commits) in heights.iter_mut().zip(spark) {
        if commits > 0 {
            let scaled = u64::from(commits) * u64::from(max_px) / u64::from(top);
            *height = u32::try_from(scaled).unwrap_or(max_px).max(1);
        }
    }
    heights
}

/// The word for a trend.
#[must_use]
pub(crate) fn trend_word(trend: Trend) -> &'static str {
    match trend {
        Trend::Rising => "rising",
        Trend::Steady => "steady",
        Trend::Falling => "falling",
    }
}

/// A share as a whole percentage, rounded to the nearest.
#[must_use]
pub(crate) fn percent_round(share: f64) -> String {
    format!("{}", (share * 100.0).round())
}

/// The rank line of the details: the rank, the report's size and the percentile.
#[must_use]
pub(crate) fn rank_line(file: &FileHealth, of: usize) -> String {
    // From the rank, not the rounded percentile, so the hottest file never claims 100%.
    let colder = of.saturating_sub(usize::try_from(file.rank).unwrap_or(of));
    let share = (colder * 100).checked_div(of).unwrap_or(0);
    format!("Rank {} of {of} · hotter than {share}% of files", file.rank)
}

/// What a dormancy break says, in the list and in the details.
#[must_use]
pub(crate) fn dormancy_text(dormancy: &DormancyBreak) -> String {
    format!(
        "Quiet {} active days, now changing on {} of the last 30",
        dormancy.quiet_active_days, dormancy.recent_active_days
    )
}

/// A file that changes with the one shown: its degree and how many commits it shares with it.
#[must_use]
pub(crate) fn partner_text(partner: &Partner) -> String {
    format!(
        "degree {:.2} · {} shared commits",
        partner.degree, partner.shared_commits
    )
}

/// The three lines of a file's ownership: its authors, its top author's share, its occasional
/// authors. Counts and shares only.
#[must_use]
pub(crate) fn ownership_lines(ownership: &Ownership) -> [String; 3] {
    [
        format!("{} authors", ownership.authors),
        format!(
            "Top author: {}% of commits",
            percent_round(ownership.top_author_share)
        ),
        format!("{} occasional authors", ownership.minor_contributors),
    ]
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

    fn paths(state: &HealthState, indices: &[usize]) -> Vec<String> {
        indices
            .iter()
            .map(|&index| state.file(index).unwrap().path.clone())
            .collect()
    }

    fn dormant(path: &str, rank: u32) -> FileHealth {
        let mut file = file(path, rank, 99.0);
        file.dormancy_break = Some(gm_schema::DormancyBreak {
            quiet_active_days: 150,
            recent_active_days: 4,
        });
        file
    }

    #[test]
    fn an_empty_filter_keeps_every_file_in_rank_order() {
        let state = sample();
        assert_eq!(state.filter(), "");
        assert_eq!(state.rows(), &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_folder_filter_matches_on_a_segment_boundary() {
        let mut state = sample();
        assert!(state.set_filter("a"));
        assert_eq!(
            paths(&state, state.rows()),
            ["a/b/d.rs", "a/x.rs", "a/b/c.rs"]
        );
        assert!(state.set_filter("a/b"));
        assert_eq!(paths(&state, state.rows()), ["a/b/d.rs", "a/b/c.rs"]);
        assert!(state.set_filter("ab"));
        assert_eq!(paths(&state, state.rows()), ["ab/z.rs"]);
    }

    #[test]
    fn a_filter_that_names_a_file_keeps_that_file_and_nothing_else() {
        let mut state = sample();
        state.set_filter("cold.rs");
        assert_eq!(paths(&state, state.rows()), ["cold.rs"]);
        state.set_filter("missing");
        assert!(state.rows().is_empty());
    }

    #[test]
    fn the_filter_ignores_surrounding_slashes_and_spaces() {
        assert_eq!(scope_of("/src/billing/"), "src/billing");
        assert_eq!(scope_of("  src/billing  "), "src/billing");
        assert_eq!(scope_of("/"), "");
        assert_eq!(scope_of(""), "");
        let mut state = sample();
        state.set_filter("/a/b/");
        assert_eq!(paths(&state, state.rows()), ["a/b/d.rs", "a/b/c.rs"]);
    }

    #[test]
    fn is_under_needs_the_folder_and_a_slash() {
        assert!(is_under("src/billing/a.rs", "src/billing"));
        assert!(is_under("src/billing", "src/billing"));
        assert!(!is_under("src/billing2/a.rs", "src/billing"));
        assert!(!is_under("src/bill", "src/billing"));
        assert!(is_under("anything", ""));
    }

    #[test]
    fn a_changed_filter_says_so_and_an_unchanged_one_does_not() {
        let mut state = sample();
        assert!(state.set_filter("a"));
        assert!(!state.set_filter("a"));
        assert_eq!(state.filter(), "a");
    }

    #[test]
    fn dormant_files_are_listed_in_rank_order_under_the_filter() {
        let mut state = HealthState::default();
        state.requested(OperationId(20));
        state.loaded(
            OperationId(20),
            Box::new(report(
                vec![
                    file("src/a.rs", 1, 99.0),
                    dormant("src/b.rs", 2),
                    dormant("docs/c.md", 3),
                    dormant("src/d.rs", 4),
                ],
                true,
            )),
        );
        assert_eq!(
            paths(&state, state.dormant()),
            ["src/b.rs", "docs/c.md", "src/d.rs"]
        );
        state.set_filter("src");
        assert_eq!(paths(&state, state.dormant()), ["src/b.rs", "src/d.rs"]);
        state.set_filter("nothing");
        assert!(state.dormant().is_empty());
    }

    #[test]
    fn the_heat_scale_is_the_hottest_file_of_the_whole_report() {
        let mut state = HealthState::default();
        let mut hot = file("hot.rs", 1, 99.0);
        hot.heat = 8.0;
        let mut warm = file("warm.rs", 2, 98.0);
        warm.heat = 2.0;
        state.requested(OperationId(21));
        state.loaded(OperationId(21), Box::new(report(vec![hot, warm], true)));
        assert!((state.max_heat() - 8.0).abs() < f64::EPSILON);
        state.set_filter("warm");
        assert!((state.max_heat() - 8.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_selection_shows_its_file_and_survives_a_filter() {
        let mut state = sample();
        assert!(state.selected_file().is_none());
        state.select("a/x.rs");
        assert_eq!(state.selected(), Some("a/x.rs"));
        assert_eq!(state.selected_file().unwrap().rank, 2);
        state.set_filter("cold");
        assert_eq!(state.selected_file().unwrap().path, "a/x.rs");
    }

    #[test]
    fn a_selection_of_a_file_not_in_the_report_shows_nothing() {
        let mut state = sample();
        state.select("gone.rs");
        assert_eq!(state.selected(), Some("gone.rs"));
        assert!(state.selected_file().is_none());
    }

    #[test]
    fn each_answer_refreshes_the_rows() {
        let mut state = HealthState::default();
        state.set_filter("src");
        assert!(state.rows().is_empty());
        state.requested(OperationId(22));
        state.loaded(
            OperationId(22),
            Box::new(report(vec![file("src/a.rs", 1, 99.0)], false)),
        );
        assert_eq!(state.rows(), &[0]);
        state.loaded(
            OperationId(22),
            Box::new(report(
                vec![file("src/a.rs", 1, 99.0), file("src/b.rs", 2, 98.0)],
                true,
            )),
        );
        assert_eq!(state.rows(), &[0, 1]);
    }

    #[test]
    fn clear_forgets_the_filter_and_the_selection() {
        let mut state = sample();
        state.set_filter("a");
        state.select("a/x.rs");
        state.clear();
        assert_eq!(state.filter(), "");
        assert_eq!(state.selected(), None);
        assert!(state.rows().is_empty());
        assert!(!state.has_report());
    }

    #[test]
    fn heat_bars_scale_to_the_hottest_file() {
        assert!((heat_share(2.0, 8.0) - 0.25).abs() < f64::EPSILON);
        assert!((heat_share(9.0, 8.0) - 1.0).abs() < f64::EPSILON);
        assert!(heat_share(1.0, 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn spark_bars_are_proportional_to_the_busiest_bar() {
        let mut spark = [0; SPARK_BARS];
        spark[0] = 10;
        spark[1] = 5;
        spark[2] = 1;
        let heights = spark_heights(&spark, 16);
        assert_eq!(heights[0], 16);
        assert_eq!(heights[1], 8);
        assert_eq!(heights[2], 1, "a bar with a commit is never empty");
        assert_eq!(heights[3], 0);
        assert_eq!(spark_heights(&[0; SPARK_BARS], 16), [0; SPARK_BARS]);
    }

    #[test]
    fn trends_are_words() {
        assert_eq!(trend_word(Trend::Rising), "rising");
        assert_eq!(trend_word(Trend::Steady), "steady");
        assert_eq!(trend_word(Trend::Falling), "falling");
    }

    #[test]
    fn shares_round_to_the_nearest_and_the_hottest_file_never_claims_all() {
        let first = file("a.rs", 1, 99.97);
        assert_eq!(
            rank_line(&first, 3604),
            "Rank 1 of 3604 · hotter than 99% of files"
        );
        assert_eq!(percent_round(0.18), "18");
        assert_eq!(percent_round(0.995), "100");
    }

    #[test]
    fn a_partner_says_its_degree_and_its_shared_commits() {
        let partner = Partner {
            path: "src/billing/Ledger.java".to_owned(),
            degree: 0.724,
            shared_commits: 23,
        };
        assert_eq!(partner_text(&partner), "degree 0.72 · 23 shared commits");
    }

    #[test]
    fn the_detail_sentences_read_as_the_brief_words_them() {
        let mut hot = file("src/billing/Invoice.java", 3, 99.8);
        hot.dormancy_break = None;
        assert_eq!(
            rank_line(&hot, 1240),
            "Rank 3 of 1240 · hotter than 99% of files"
        );
        assert_eq!(
            dormancy_text(&gm_schema::DormancyBreak {
                quiet_active_days: 140,
                recent_active_days: 5,
            }),
            "Quiet 140 active days, now changing on 5 of the last 30"
        );
        assert_eq!(
            ownership_lines(&Ownership {
                authors: 14,
                top_author_share: 0.18,
                minor_contributors: 9,
            }),
            [
                "14 authors",
                "Top author: 18% of commits",
                "9 occasional authors"
            ]
        );
    }
}

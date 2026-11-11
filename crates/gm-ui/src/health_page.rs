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
    CHURN_DAYS, ChurnSummary, Complexity, DormancyBreak, FileHealth, HealthProgress, HealthReport,
    OperationId, Ownership, Partner, SPARK_BARS, SURVIVAL_AGES, Trend,
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
    /// The churn of the files under the filter that have churn; `None` when none has. Kept by
    /// `refresh`.
    churn: Option<ChurnSummary>,
    /// A request came while a read was in flight, so the read is asked again when it ends (D229).
    wanted_again: bool,
}

impl HealthState {
    /// A read of the health was asked for under `id`. A new request replaces an old error and
    /// answers any earlier request that came while the previous read was in flight.
    pub(crate) fn requested(&mut self, id: OperationId) {
        self.pending = Some(id);
        self.progress = None;
        self.error = None;
        self.wanted_again = false;
    }

    /// A request came while a read is in flight: the read answers an older HEAD, so one more
    /// read follows it (D229).
    pub(crate) fn want_again(&mut self) {
        self.wanted_again = true;
    }

    /// Whether a request was skipped and no read is in flight any more. Taking it clears it, so
    /// the caller asks once, after a complete answer, a failure or a cancel.
    pub(crate) fn take_ask_again(&mut self) -> bool {
        if self.pending.is_none() && self.wanted_again {
            self.wanted_again = false;
            return true;
        }
        false
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

    /// The churn of the folder the list is limited to: the sum over its files that have churn.
    /// `None` when no file under the filter has churn.
    #[must_use]
    pub(crate) fn churn(&self) -> Option<ChurnSummary> {
        self.churn.clone()
    }

    /// Whether the answers are still being read: a read is in flight, or the report is partial.
    #[must_use]
    pub(crate) fn reading(&self) -> bool {
        self.pending() || self.report.as_ref().is_some_and(|report| !report.complete)
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

    /// The position of the selected file among the rows, or `None` when it is not listed.
    #[must_use]
    pub(crate) fn selected_row(&self) -> Option<usize> {
        let selected = self.selected.as_deref()?;
        self.rows.iter().position(|&index| {
            self.file(index)
                .is_some_and(|file| file.path.as_str() == selected)
        })
    }

    /// The path of the file on the row at `row`, when there is one.
    #[must_use]
    pub(crate) fn path_at_row(&self, row: usize) -> Option<&str> {
        let index = *self.rows.get(row)?;
        self.file(index).map(|file| file.path.as_str())
    }

    /// Recomputes what the list keeps from the report and the filter, and the heat scale.
    fn refresh(&mut self) {
        self.rows.clear();
        self.dormant.clear();
        self.max_heat = 0.0;
        self.churn = None;
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
        // The churn of a removed file counts toward the folder it was in (D229).
        self.churn = ChurnSummary::of(
            self.rows
                .iter()
                .filter_map(|&index| report.files[index].churn.as_ref())
                .chain(
                    report
                        .removed_churn
                        .iter()
                        .filter(|removed| is_under(&removed.path, scope))
                        .map(|removed| &removed.churn),
                ),
        );
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

/// The index in `SURVIVAL_AGES` of the age the details name: 30 active days.
const DETAIL_SURVIVAL_INDEX: usize = 2;

/// The line of a folder's churn: the share of its added lines rewritten within `CHURN_DAYS`,
/// and how many lines were added.
#[must_use]
pub(crate) fn churn_line(churn: &ChurnSummary) -> String {
    format!(
        "Rewritten within {CHURN_DAYS} active days: {}% of {} lines",
        percent_round(churn.rewritten_share),
        grouped(u64::from(churn.lines_added))
    )
}

/// Each age of `SURVIVAL_AGES` with the share of lines still alive at it, as a whole percentage.
#[must_use]
pub(crate) fn survival_points(churn: &ChurnSummary) -> Vec<(u32, String)> {
    SURVIVAL_AGES
        .iter()
        .copied()
        .zip(churn.survival.iter().copied())
        .map(|(age, share)| (age, format!("{}%", percent_round(share))))
        .collect()
}

/// The line of a file's churn that names the share still alive after 30 active days.
#[must_use]
pub(crate) fn alive_line(churn: &ChurnSummary) -> String {
    format!(
        "Alive after 30 active days: {}%",
        percent_round(churn.survival[DETAIL_SURVIVAL_INDEX])
    )
}

/// The share at the bottom of a survival chart: the lowest share less five points, rounded down
/// to a tenth and kept between 0 and 0.9, so the bars use the track's height for the range shown.
#[must_use]
pub(crate) fn survival_floor(survival: &[f64]) -> f64 {
    let lowest = survival.iter().copied().fold(1.0, f64::min);
    let base = (lowest - 0.05).clamp(0.0, 0.9);
    // The epsilon keeps a share that is exactly a tenth from rounding down one tenth too far.
    ((base * 10.0) + 1e-9).floor() / 10.0
}

/// Each share of a survival chart as the fraction of its track to fill, 0 to 1, measured from
/// `survival_floor`: the floor is empty and 100% is full.
#[must_use]
pub(crate) fn survival_fills(survival: &[f64]) -> Vec<f64> {
    let floor = survival_floor(survival);
    survival
        .iter()
        .map(|&share| ((share - floor) / (1.0 - floor)).clamp(0.0, 1.0))
        .collect()
}

/// The two lines of a file's complexity: its nesting, and how it compares with the other
/// measured files (the percentile rounded down).
#[must_use]
pub(crate) fn complexity_lines(complexity: &Complexity) -> [String; 2] {
    [
        format!(
            "Deepest nesting {} · mean {:.1} · {} lines",
            complexity.indent_max, complexity.indent_mean, complexity.lines
        ),
        format!(
            "More complex than {}% of the measured files",
            complexity.percentile.floor()
        ),
    ]
}

/// A move in the hotspot list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Up,
    Down,
    First,
    Last,
    PageUp,
    PageDown,
}

/// The step a key of the hotspot list makes, or `None` for any other key.
#[must_use]
pub(crate) fn step_of_key(key: &str) -> Option<Step> {
    match key {
        "up" => Some(Step::Up),
        "down" => Some(Step::Down),
        "home" => Some(Step::First),
        "end" => Some(Step::Last),
        "pageup" => Some(Step::PageUp),
        "pagedown" => Some(Step::PageDown),
        _ => None,
    }
}

/// The rows a page of the list moves by when the viewport holds `visible_px` high rows of
/// `row_px`. 15 when the viewport is not laid out yet.
#[must_use]
pub(crate) fn page_rows(visible_px: f32, row_px: f32) -> usize {
    const UNKNOWN: usize = 15;
    if visible_px <= 0.0 || row_px <= 0.0 {
        return UNKNOWN;
    }
    let mut rows = 0;
    let mut used = 0.0;
    while used + row_px <= visible_px {
        rows += 1;
        used += row_px;
    }
    rows.max(1)
}

/// The row a key moves the selection to, among `len` rows. `current` is the selected row. With
/// no selection, Down, Home and `PageDown` select the first row and Up, End and `PageUp` the last.
/// `page` is the number of rows a page moves by, at least one. `None` for an empty list.
#[must_use]
pub(crate) fn move_selection(
    current: Option<usize>,
    len: usize,
    step: Step,
    page: usize,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    let page = page.max(1);
    let Some(current) = current.map(|row| row.min(last)) else {
        return Some(match step {
            Step::Up | Step::Last | Step::PageUp => last,
            Step::Down | Step::First | Step::PageDown => 0,
        });
    };
    Some(match step {
        Step::Up => current.saturating_sub(1),
        Step::Down => current.saturating_add(1).min(last),
        Step::First => 0,
        Step::Last => last,
        Step::PageUp => current.saturating_sub(page),
        Step::PageDown => current.saturating_add(page).min(last),
    })
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
    use gm_schema::{AGE_BUCKETS, LineChurn, Ownership, Trend};

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
            churn: None,
            bytes: 0,
            complexity: None,
        }
    }

    fn report(files: Vec<FileHealth>, complete: bool) -> HealthReport {
        HealthReport {
            rev: "a1b2c3d4".to_owned(),
            config_hash: "3fa9e1c2".to_owned(),
            complete,
            files,
            removed_churn: Vec::new(),
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

    /// A file whose lines added in the window were `added`, `rewritten` within 14 days, with all
    /// of them still alive at the oldest age.
    fn file_with_churn(path: &str, rank: u32, added: u32, rewritten: u32) -> FileHealth {
        let mut file = file(path, rank, 99.0);
        let mut churn = LineChurn {
            added,
            rewritten,
            ..LineChurn::default()
        };
        churn.alive[AGE_BUCKETS - 1] = added;
        file.churn = Some(churn);
        file
    }

    #[test]
    fn the_churn_of_the_folder_sums_its_files_that_have_churn() {
        let mut state = HealthState::default();
        state.requested(OperationId(30));
        state.loaded(
            OperationId(30),
            Box::new(report(
                vec![
                    file_with_churn("src/a.rs", 1, 4, 1),
                    file("src/b.rs", 2, 99.0),
                    file_with_churn("docs/c.md", 3, 6, 2),
                ],
                true,
            )),
        );
        let whole = state.churn().expect("the churn of the whole repository");
        assert_eq!((whole.lines_added, whole.rewritten_14), (10, 3));
        state.set_filter("src");
        let folder = state.churn().expect("the churn of src");
        assert_eq!((folder.lines_added, folder.rewritten_14), (4, 1));
        state.set_filter("src/b.rs");
        assert_eq!(state.churn(), None, "a file without churn has none");
    }

    #[test]
    fn no_churn_in_the_report_is_no_churn_and_clear_forgets_it() {
        let mut state = sample();
        assert_eq!(state.churn(), None);
        state.requested(OperationId(31));
        state.loaded(
            OperationId(31),
            Box::new(report(vec![file_with_churn("x.rs", 1, 2, 0)], true)),
        );
        assert!(state.churn().is_some());
        state.clear();
        assert_eq!(state.churn(), None);
    }

    #[test]
    fn reading_is_a_read_in_flight_or_a_partial_report() {
        let mut state = HealthState::default();
        assert!(!state.reading());
        state.requested(OperationId(32));
        assert!(state.reading());
        state.loaded(
            OperationId(32),
            Box::new(report(vec![file("x.rs", 1, 99.0)], false)),
        );
        assert!(state.reading());
        state.loaded(
            OperationId(32),
            Box::new(report(vec![file("x.rs", 1, 99.0)], true)),
        );
        assert!(!state.reading());
    }

    #[test]
    fn the_churn_line_rounds_the_share_and_groups_the_lines() {
        let churn = ChurnSummary {
            lines_added: 12_340,
            rewritten_14: 4_000,
            rewritten_share: 0.3241,
            survival: [1.0; SURVIVAL_AGES.len()],
        };
        assert_eq!(
            churn_line(&churn),
            "Rewritten within 14 active days: 32% of 12,340 lines"
        );
    }

    #[test]
    fn the_survival_points_name_each_age_with_its_share() {
        let churn = ChurnSummary {
            lines_added: 10,
            rewritten_14: 5,
            rewritten_share: 0.5,
            survival: [0.9, 0.82, 0.5, 0.41, 0.2, 0.0],
        };
        let points = survival_points(&churn);
        let ages: Vec<u32> = points.iter().map(|(age, _)| *age).collect();
        assert_eq!(ages, [7, 14, 30, 60, 90, 180]);
        let shares: Vec<&str> = points.iter().map(|(_, share)| share.as_str()).collect();
        assert_eq!(shares, ["90%", "82%", "50%", "41%", "20%", "0%"]);
        assert_eq!(SURVIVAL_AGES[DETAIL_SURVIVAL_INDEX], 30);
        assert_eq!(alive_line(&churn), "Alive after 30 active days: 50%");
    }

    fn close(actual: f64, expected: f64) -> bool {
        (actual - expected).abs() < 1e-9
    }

    #[test]
    fn the_survival_floor_is_the_lowest_share_less_five_points_to_a_tenth_at_most_half() {
        assert!(close(survival_floor(&[0.98, 0.66, 1.0]), 0.6));
        assert!(close(survival_floor(&[0.99, 0.86]), 0.8));
        assert!(close(survival_floor(&[0.3, 0.9]), 0.2));
        assert!(close(survival_floor(&[0.35, 1.0]), 0.3));
        assert!(close(survival_floor(&[0.2]), 0.1));
        assert!(close(survival_floor(&[0.02]), 0.0));
        assert!(close(survival_floor(&[0.0, 0.0]), 0.0));
        assert!(close(survival_floor(&[1.0, 1.0]), 0.9));
    }

    #[test]
    fn the_survival_fills_measure_from_the_floor_to_the_full_track() {
        let fills = survival_fills(&[0.98, 0.66, 1.0, 0.7]);
        // The floor is 0.6, so 66% to 100% fill the track.
        assert!(close(fills[0], 0.95));
        assert!(close(fills[1], 0.15));
        assert!(close(fills[2], 1.0));
        assert!(close(fills[3], 0.25));
        let fills = survival_fills(&[0.9, 0.4]);
        assert!(close(fills[0], 0.6 / 0.7));
        assert!(close(fills[1], 0.1 / 0.7));
        assert!(survival_fills(&[0.0; 3]).iter().all(|&fill| fill == 0.0));
    }

    #[test]
    fn a_complexity_block_names_the_nesting_and_the_rounded_down_percentile() {
        let complexity = Complexity {
            lines: 1840,
            indent_max: 7,
            indent_mean: 2.1,
            indent_total: 3864,
            percentile: 97.5,
        };
        assert_eq!(
            complexity_lines(&complexity),
            [
                "Deepest nesting 7 · mean 2.1 · 1840 lines".to_owned(),
                "More complex than 97% of the measured files".to_owned(),
            ]
        );
    }

    #[test]
    fn keys_map_to_steps_and_others_map_to_none() {
        assert_eq!(step_of_key("up"), Some(Step::Up));
        assert_eq!(step_of_key("down"), Some(Step::Down));
        assert_eq!(step_of_key("home"), Some(Step::First));
        assert_eq!(step_of_key("end"), Some(Step::Last));
        assert_eq!(step_of_key("pageup"), Some(Step::PageUp));
        assert_eq!(step_of_key("pagedown"), Some(Step::PageDown));
        assert_eq!(step_of_key("enter"), None);
        assert_eq!(step_of_key("a"), None);
    }

    #[test]
    fn arrows_move_one_row_and_stop_at_the_ends() {
        assert_eq!(move_selection(Some(1), 5, Step::Down, 3), Some(2));
        assert_eq!(move_selection(Some(1), 5, Step::Up, 3), Some(0));
        assert_eq!(move_selection(Some(0), 5, Step::Up, 3), Some(0));
        assert_eq!(move_selection(Some(4), 5, Step::Down, 3), Some(4));
    }

    #[test]
    fn home_and_end_go_to_the_first_and_last_row() {
        assert_eq!(move_selection(Some(3), 5, Step::First, 3), Some(0));
        assert_eq!(move_selection(Some(1), 5, Step::Last, 3), Some(4));
        assert_eq!(move_selection(Some(0), 1, Step::Last, 3), Some(0));
    }

    #[test]
    fn pages_move_by_the_page_and_stop_at_the_ends() {
        assert_eq!(move_selection(Some(2), 20, Step::PageDown, 15), Some(17));
        assert_eq!(move_selection(Some(10), 20, Step::PageDown, 15), Some(19));
        assert_eq!(move_selection(Some(10), 20, Step::PageUp, 15), Some(0));
        assert_eq!(move_selection(Some(10), 20, Step::PageUp, 0), Some(9));
    }

    #[test]
    fn with_no_selection_down_selects_the_first_row() {
        assert_eq!(move_selection(None, 5, Step::Down, 3), Some(0));
        assert_eq!(move_selection(None, 5, Step::First, 3), Some(0));
        assert_eq!(move_selection(None, 5, Step::PageDown, 3), Some(0));
        assert_eq!(move_selection(None, 5, Step::Up, 3), Some(4));
        assert_eq!(move_selection(None, 5, Step::Last, 3), Some(4));
        assert_eq!(move_selection(None, 5, Step::PageUp, 3), Some(4));
    }

    #[test]
    fn an_empty_list_has_no_selection_and_a_stale_row_is_brought_back() {
        assert_eq!(move_selection(None, 0, Step::Down, 3), None);
        assert_eq!(move_selection(Some(0), 0, Step::Up, 3), None);
        assert_eq!(move_selection(Some(10), 3, Step::Up, 3), Some(1));
    }

    #[test]
    fn a_page_is_the_rows_that_fit_or_fifteen_when_unknown() {
        assert_eq!(page_rows(0.0, 24.0), 15);
        assert_eq!(page_rows(400.0, 0.0), 15);
        assert_eq!(page_rows(100.0, 24.0), 4);
        assert_eq!(page_rows(10.0, 24.0), 1);
    }

    #[test]
    fn the_selection_is_a_row_of_the_list_under_the_filter() {
        let mut state = sample();
        state.select("a/x.rs");
        assert_eq!(state.selected_row(), Some(1));
        assert_eq!(state.path_at_row(2), Some("a/b/c.rs"));
        assert_eq!(state.path_at_row(5), None);
        state.set_filter("a/b");
        assert_eq!(state.selected_row(), None);
        assert_eq!(state.path_at_row(1), Some("a/b/c.rs"));
        state.select("cold.rs");
        assert_eq!(state.selected_row(), None);
    }

    #[test]
    fn a_request_during_a_read_is_asked_again_once_the_read_ends() {
        let mut state = HealthState::default();
        assert!(!state.take_ask_again());
        state.requested(OperationId(30));
        state.want_again();
        state.loaded(
            OperationId(30),
            Box::new(report(vec![file("x.rs", 1, 99.0)], false)),
        );
        assert!(!state.take_ask_again(), "a partial answer keeps the read");
        state.loaded(
            OperationId(30),
            Box::new(report(vec![file("x.rs", 1, 99.0)], true)),
        );
        assert!(state.take_ask_again());
        assert!(!state.take_ask_again(), "asked once");
    }

    #[test]
    fn a_failed_or_cancelled_read_also_asks_again() {
        let mut state = HealthState::default();
        state.requested(OperationId(31));
        state.want_again();
        state.failed(OperationId(31), "boom".to_owned());
        assert!(state.take_ask_again());

        let mut state = HealthState::default();
        state.requested(OperationId(32));
        state.want_again();
        state.cancelled(OperationId(32));
        assert!(state.take_ask_again());
    }

    #[test]
    fn a_new_read_answers_the_request_it_started_from() {
        let mut state = HealthState::default();
        state.requested(OperationId(33));
        state.want_again();
        state.cancelled(OperationId(33));
        state.requested(OperationId(34));
        state.loaded(
            OperationId(34),
            Box::new(report(vec![file("x.rs", 1, 99.0)], true)),
        );
        assert!(!state.take_ask_again());
    }

    #[test]
    fn removed_files_count_toward_their_folder() {
        let mut churned = file("src/a.rs", 1, 99.0);
        churned.churn = Some(LineChurn {
            added: 4,
            rewritten: 1,
            ..LineChurn::default()
        });
        let removed = |path: &str, added, rewritten| gm_schema::RemovedChurn {
            path: path.to_owned(),
            churn: LineChurn {
                added,
                rewritten,
                ..LineChurn::default()
            },
        };
        let mut full = report(vec![churned], true);
        full.removed_churn = vec![
            removed("src/old.rs", 10, 5),
            removed("srcx/old.rs", 2, 0),
            removed("other/gone.rs", 3, 0),
        ];
        let mut state = HealthState::default();
        state.requested(OperationId(40));
        state.loaded(OperationId(40), Box::new(full));

        let summary = state.churn().expect("churn");
        assert_eq!((summary.lines_added, summary.rewritten_14), (19, 6));
        assert!(state.set_filter("src"));
        let summary = state.churn().expect("churn");
        assert_eq!((summary.lines_added, summary.rewritten_14), (14, 6));
        assert!(state.set_filter("srcx"));
        assert_eq!(state.churn().expect("churn").lines_added, 2);
        assert!(state.set_filter("other"));
        assert_eq!(state.churn().expect("churn").lines_added, 3);
        assert!(state.set_filter("nothing"));
        assert!(state.churn().is_none());
    }
}

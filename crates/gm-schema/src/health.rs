//! Code health (docs/guide/code-health.md, D225): what the engine answers about
//! how the files of a repository changed over time. Shared by the UI and MCP.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The number of bars in a file's activity chart.
pub const SPARK_BARS: usize = 12;

/// Churn counts a line rewritten when it dies within this many active days of being added.
pub const CHURN_DAYS: u32 = 14;

/// The ages, in active days of the repository, at which line survival is given.
pub const SURVIVAL_AGES: [u32; 6] = [7, 14, 30, 60, 90, 180];

/// Age buckets of `LineChurn`: one per `SURVIVAL_AGES` boundary, then older than the last.
pub const AGE_BUCKETS: usize = SURVIVAL_AGES.len() + 1;

/// The bucket of a line `age` active days old: the first `SURVIVAL_AGES` value it does not
/// exceed, or the last bucket when it is older than all of them.
#[must_use]
pub fn age_bucket(age: u32) -> usize {
    SURVIVAL_AGES
        .iter()
        .position(|&limit| age <= limit)
        .unwrap_or(SURVIVAL_AGES.len())
}

/// The direction of a file's recent change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Trend {
    Rising,
    Steady,
    Falling,
}

/// A quiet file that has started to change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DormancyBreak {
    /// Active days of the repository without a change before the recent ones.
    pub quiet_active_days: u32,
    /// How many of the last 30 active days of the repository changed the file.
    pub recent_active_days: u32,
}

/// A file that changes in the same commits as another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Partner {
    pub path: String,
    /// Shared commits divided by the mean of each file's commits.
    pub degree: f64,
    pub shared_commits: u32,
}

/// How widely a file is owned. Counts and shares only, never a name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Ownership {
    pub authors: u32,
    /// The share of the file's commits made by its most frequent author.
    pub top_author_share: f64,
    /// Authors who made fewer than 5% of the file's commits.
    pub minor_contributors: u32,
}

/// The indicators of one file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FileHealth {
    pub path: String,
    /// 1 for the hottest file.
    pub rank: u32,
    /// The share of files colder than this one, 0 to 100.
    pub percentile: f64,
    /// The decayed heat.
    pub heat: f64,
    pub trend: Trend,
    /// Commits per bar over the last 180 active days of the repository, oldest first.
    pub spark: [u32; SPARK_BARS],
    pub dormancy_break: Option<DormancyBreak>,
    /// At most 5, strongest first.
    pub changes_together: Vec<Partner>,
    pub ownership: Ownership,
    pub constraints: Vec<String>,
    /// How the lines added to the file in the window lived (docs/guide/code-health.md#churn).
    /// `None` until the index holds the changes of the window, or for a file whose lines are
    /// not followed (binary).
    pub churn: Option<LineChurn>,
    /// The size of the file at the commit, in bytes (`git ls-tree -l`); 0 when unknown.
    pub bytes: u64,
    /// How deeply the file's code is nested. `None` outside the measured hot set, for a
    /// file over 1 MiB or one that is not text.
    pub complexity: Option<Complexity>,
}

/// The nesting of a file's code, read from its indentation (docs/guide/code-health.md, D229).
/// One level is a tab or four spaces; blank lines do not count.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Complexity {
    /// Lines that are not blank.
    pub lines: u32,
    /// The deepest nesting, in whole levels.
    pub indent_max: u32,
    /// The mean nesting of the lines that are not blank, in levels.
    pub indent_mean: f64,
    /// The sum of every line's nesting, in levels, rounded: grows with size and depth.
    pub indent_total: u32,
    /// The share of measured files with a lower `indent_total`, 0 to 100.
    pub percentile: f64,
}

/// The churn of a file followed in the window that does not exist at the commit: it still
/// counts toward the folder of `path`, its path at its last change (D229).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemovedChurn {
    pub path: String,
    pub churn: LineChurn,
}

/// The lines added to one file in the window, followed along the first-parent history (D228).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LineChurn {
    /// Lines added at least `CHURN_DAYS` active days before the commit looked at.
    pub added: u32,
    /// Of `added`, the lines deleted or changed within `CHURN_DAYS` active days.
    pub rewritten: u32,
    /// Every line added in the window that has died, by the `age_bucket` of its age at death.
    pub deaths: [u32; AGE_BUCKETS],
    /// Every line added in the window still alive, by the `age_bucket` of its current age.
    pub alive: [u32; AGE_BUCKETS],
}

/// The churn of a folder or a file: the sums of its `LineChurn`s.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChurnSummary {
    pub lines_added: u32,
    pub rewritten_14: u32,
    /// `rewritten_14 / lines_added`, 0 when nothing was added.
    pub rewritten_share: f64,
    /// The share of lines still alive at each of `SURVIVAL_AGES` (Kaplan–Meier, actuarial: a
    /// line still alive counts as at risk for half of its last bucket).
    pub survival: [f64; SURVIVAL_AGES.len()],
}

impl ChurnSummary {
    /// Sums `items`. `None` when there is nothing to sum.
    pub fn of<'a>(items: impl IntoIterator<Item = &'a LineChurn>) -> Option<Self> {
        let mut total = LineChurn::default();
        let mut any = false;
        for item in items {
            any = true;
            total.added += item.added;
            total.rewritten += item.rewritten;
            for b in 0..AGE_BUCKETS {
                total.deaths[b] += item.deaths[b];
                total.alive[b] += item.alive[b];
            }
        }
        if !any {
            return None;
        }
        let mut at_risk: f64 = total
            .deaths
            .iter()
            .chain(total.alive.iter())
            .map(|&n| f64::from(n))
            .sum();
        let mut alive_share = 1.0;
        let mut survival = [1.0; SURVIVAL_AGES.len()];
        for (b, point) in survival.iter_mut().enumerate() {
            let died = f64::from(total.deaths[b]);
            let left = f64::from(total.alive[b]);
            let exposed = at_risk - left / 2.0;
            if exposed > 0.0 {
                alive_share *= 1.0 - died / exposed;
            }
            *point = alive_share;
            at_risk -= died + left;
        }
        let rewritten_share = if total.added == 0 {
            0.0
        } else {
            f64::from(total.rewritten) / f64::from(total.added)
        };
        Some(Self {
            lines_added: total.added,
            rewritten_14: total.rewritten,
            rewritten_share,
            survival,
        })
    }
}

/// The indicators of a repository at one commit and one configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HealthReport {
    /// The full id of the commit the answer was computed at.
    pub rev: String,
    /// A short hash of the configuration that applied.
    pub config_hash: String,
    /// False while the index does not hold every commit of the window.
    pub complete: bool,
    /// Every file changed in the window, hottest first (`rank` order).
    pub files: Vec<FileHealth>,
    /// The churn of files followed in the window that no longer exist, by path order.
    pub removed_churn: Vec<RemovedChurn>,
}

/// How far the index has been built for one commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthProgress {
    /// Commits of the window already in the index.
    pub indexed: u64,
    /// Commits in the window.
    pub total: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_fall_in_the_first_bucket_they_do_not_exceed() {
        assert_eq!(age_bucket(0), 0);
        assert_eq!(age_bucket(7), 0);
        assert_eq!(age_bucket(8), 1);
        assert_eq!(age_bucket(180), 5);
        assert_eq!(age_bucket(181), 6);
    }

    #[test]
    fn nothing_to_sum_is_no_summary() {
        assert_eq!(ChurnSummary::of([]), None);
    }

    #[test]
    fn half_the_lines_die_in_the_first_week_and_the_rest_live() {
        let mut churn = LineChurn {
            added: 10,
            rewritten: 5,
            ..LineChurn::default()
        };
        churn.deaths[0] = 5;
        churn.alive[6] = 5;
        let summary = ChurnSummary::of([&churn]).expect("a summary");
        assert_eq!(summary.lines_added, 10);
        assert!((summary.rewritten_share - 0.5).abs() < 1e-9);
        assert!((summary.survival[0] - 0.5).abs() < 1e-9);
        assert!((summary.survival[5] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn files_add_up() {
        let a = LineChurn {
            added: 4,
            rewritten: 1,
            ..LineChurn::default()
        };
        let b = LineChurn {
            added: 6,
            rewritten: 2,
            ..LineChurn::default()
        };
        let summary = ChurnSummary::of([&a, &b]).expect("a summary");
        assert_eq!((summary.lines_added, summary.rewritten_14), (10, 3));
    }
}

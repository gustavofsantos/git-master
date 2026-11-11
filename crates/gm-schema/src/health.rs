//! Code health (docs/guide/code-health.md, D225): what the engine answers about
//! how the files of a repository changed over time. Shared by the UI and MCP.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The number of bars in a file's activity chart.
pub const SPARK_BARS: usize = 12;

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
}

/// How far the index has been built for one commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthProgress {
    /// Commits of the window already in the index.
    pub indexed: u64,
    /// Commits in the window.
    pub total: u64,
}

//! Code health (docs/guide/code-health.md, D225, roadmap slice 6h).
//!
//! The index (`store`, `ingest`) records what each commit changed, read once from
//! `git log` and kept in `.git/gitmaster/health.db`; it holds no configuration, so
//! it never needs rebuilding when settings change. The indicators (`metrics`) are
//! a pure function of the commits of the window and the effective configuration
//! (`config`), so the same commit and configuration always give the same answer.

pub mod churn;
pub mod config;
pub mod ingest;
pub mod metrics;
pub mod store;

/// Seconds in a day; a commit's day is its committer time divided by this, in UTC.
pub const DAY_SECS: i64 = 86_400;

/// One file a commit changed, as `git log --numstat -M` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// The path after the commit.
    pub path: String,
    /// The path before the commit, for a rename or copy.
    pub old_path: Option<String>,
    /// Lines added; `None` for a binary file.
    pub added: Option<u32>,
    /// Lines deleted; `None` for a binary file.
    pub deleted: Option<u32>,
}

/// One hunk header of a diff, as `git log -p -U0 -M` reports it (D228). A rename with no
/// content change is one hunk with all four numbers 0 and `old_path` set. A hunk with all four
/// numbers 0 and `old_path` None is a marker: the file was binary on one side of this commit;
/// its known lines are forgotten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The path after the commit.
    pub path: String,
    /// The path before the commit, when it differs from `path`.
    pub old_path: Option<String>,
    /// 1-based first line of the removed range; with `old_count` 0, the line after which
    /// the new lines go (0 for the top of the file).
    pub old_start: u32,
    pub old_count: u32,
    /// 1-based first line of the added range.
    pub new_start: u32,
    pub new_count: u32,
}

/// The hunks of one first-parent commit against its first parent, in diff order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstParentCommit {
    pub sha: String,
    pub hunks: Vec<Hunk>,
}

/// One commit of the index. A merge commit has no `changes` (D225 (3)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRecord {
    /// The full commit id.
    pub sha: String,
    /// The committer time, in seconds since the epoch.
    pub time: i64,
    /// The first 16 hex digits of the SHA-256 of the mailmapped author address,
    /// lowercased (`%aE`). Only ever counted, never shown (D225 (5)).
    pub author: String,
    /// The number of parents; more than 1 for a merge.
    pub parents: u32,
    pub changes: Vec<FileChange>,
}

impl CommitRecord {
    /// The commit's UTC day number.
    pub fn day(&self) -> i64 {
        self.time.div_euclid(DAY_SECS)
    }
}

/// Settings of one scope: a folder with values of its own (`gitmaster-health.<path>.*`).
#[derive(Clone, Debug, PartialEq)]
pub struct ScopeConfig {
    /// A folder relative to the root, without a leading or trailing `/`.
    pub path: String,
    pub half_life: Option<f64>,
    pub dormancy_gap: Option<u32>,
}

/// The effective configuration of code health (docs/guide/code-health.md#configuration).
#[derive(Clone, Debug, PartialEq)]
pub struct HealthConfig {
    /// In active days.
    pub half_life: f64,
    pub min_half_life_days: f64,
    pub max_half_life_days: f64,
    /// In active days of the repository.
    pub dormancy_gap: u32,
    /// In calendar days before the queried commit.
    pub window_days: u32,
    /// Folders (ending in `/`), exact paths or patterns (`*`, `?`, `[`, `**`).
    pub exclude: Vec<String>,
    /// Full commit ids to leave out.
    pub ignore_revs: Vec<String>,
    pub scopes: Vec<ScopeConfig>,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            half_life: 30.0,
            min_half_life_days: 14.0,
            max_half_life_days: 365.0,
            dormancy_gap: 120,
            window_days: 1095,
            exclude: Vec::new(),
            ignore_revs: Vec::new(),
            scopes: Vec::new(),
        }
    }
}

//! Which versions of a script the person has reviewed (design/workflow-scripts.md, D101). A
//! version is its text's sha256. Reviewing a version is what lets it run, and it is where the
//! person can grant automatic approval of the applying actions of event runs, tied to that hash.
//! Any edit makes a new hash, so it is unreviewed again and the grant stops applying. Kept per
//! user, in `gitmaster.db` in the app's data directory.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

/// The reviews kept; the oldest go first.
pub const RETAINED_REVIEWS: usize = 2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustError(pub String);

impl std::fmt::Display for TrustError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TrustError {}

impl From<rusqlite::Error> for TrustError {
    fn from(e: rusqlite::Error) -> Self {
        Self(e.to_string())
    }
}

/// What is known of one reviewed version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub hash: String,
    /// The workflow's name when it was reviewed, to show in a list.
    pub workflow: String,
    pub reviewed_at: i64,
    /// Event runs of this version apply without asking (D64(b)). Never for agent runs.
    pub auto_approve: bool,
}

#[derive(Clone)]
pub struct TrustStore {
    #[allow(dead_code)]
    conn: Arc<Mutex<Connection>>,
}

/// `<data dir>/gitmaster/gitmaster.db`: `$XDG_DATA_HOME` or `~/.local/share` on Linux,
/// `~/Library/Application Support` on macOS. `None` without a home.
#[must_use]
pub fn default_file() -> Option<PathBuf> {
    todo!()
}

impl TrustStore {
    /// Opens (creating it and its parent directory when missing) the database at `path` and
    /// migrates the review table. Other tables of the file are left alone.
    pub fn open(_path: &Path) -> Result<Self, TrustError> {
        todo!()
    }

    pub fn open_in_memory() -> Result<Self, TrustError> {
        todo!()
    }

    /// Marks this version as reviewed, with the grant the person chose. Reviewing a version again
    /// replaces its grant.
    pub fn review(
        &self,
        _hash: &str,
        _workflow: &str,
        _auto_approve: bool,
        _now: i64,
    ) -> Result<(), TrustError> {
        todo!()
    }

    /// The review of this version, if the person reviewed it.
    pub fn get(&self, _hash: &str) -> Result<Option<Review>, TrustError> {
        todo!()
    }

    /// Whether this version was reviewed.
    pub fn is_reviewed(&self, _hash: &str) -> Result<bool, TrustError> {
        todo!()
    }

    /// Whether event runs of this version apply without asking. `false` for a version that was
    /// not reviewed.
    pub fn auto_approve(&self, _hash: &str) -> Result<bool, TrustError> {
        todo!()
    }

    /// Changes the grant of a reviewed version. Refuses a version that was not reviewed: a grant
    /// is only given at a review.
    pub fn set_auto_approve(&self, _hash: &str, _on: bool) -> Result<(), TrustError> {
        todo!()
    }

    /// Takes the review back. Returns whether there was one.
    pub fn revoke(&self, _hash: &str) -> Result<bool, TrustError> {
        todo!()
    }
}

//! Which versions of a script the person has reviewed (design/workflow-scripts.md, D101). A
//! version is its text's sha256. Reviewing a version is what lets it run, and it is where the
//! person can grant automatic approval of the applying actions of event runs, tied to that hash.
//! Any edit makes a new hash, so it is unreviewed again and the grant stops applying. Kept per
//! user, in `gitmaster.db` in the app's data directory.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

/// The reviews kept; the oldest go first.
pub const RETAINED_REVIEWS: usize = 2000;

/// Schema version in `script_meta`. Its own key, so the journal's version and this one move apart.
const SCHEMA_VERSION: &str = "1";
const SCHEMA_KEY: &str = "trust_schema_version";
/// How long a writer waits for another connection's lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const FILE_NAME: &str = "gitmaster.db";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS script_meta (
    key TEXT PRIMARY KEY,
    value TEXT
);
CREATE TABLE IF NOT EXISTS script_review (
    hash TEXT PRIMARY KEY,
    workflow TEXT NOT NULL,
    reviewed_at INTEGER NOT NULL,
    auto_approve INTEGER NOT NULL
);
";

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
    conn: Arc<Mutex<Connection>>,
}

/// `<data dir>/gitmaster/gitmaster.db`: `$XDG_DATA_HOME` or `~/.local/share` on Linux,
/// `~/Library/Application Support` on macOS. `None` without a home.
#[must_use]
pub fn default_file() -> Option<PathBuf> {
    data_file(
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
        cfg!(target_os = "macos"),
    )
}

/// The database under the given environment values. Same rules as the approvals file: a relative
/// `XDG_DATA_HOME` is ignored, and without a home directory there is no file.
#[must_use]
pub fn data_file(
    xdg_data_home: Option<OsString>,
    home: Option<OsString>,
    macos: bool,
) -> Option<PathBuf> {
    let absolute = |value: Option<OsString>| {
        value
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty() && path.is_absolute())
    };
    let base = if macos {
        absolute(home)?.join("Library/Application Support")
    } else {
        match absolute(xdg_data_home) {
            Some(base) => base,
            None => absolute(home)?.join(".local/share"),
        }
    };
    Some(base.join("gitmaster").join(FILE_NAME))
}

impl TrustStore {
    /// Opens (creating it and its parent directory when missing) the database at `path` and
    /// migrates the review table. Other tables of the file are left alone.
    pub fn open(path: &Path) -> Result<Self, TrustError> {
        let conn = open_file(path)
            .map_err(|e| TrustError(format!("cannot open {}: {}", path.display(), e.0)))?;
        Ok(Self::wrap(conn))
    }

    pub fn open_in_memory() -> Result<Self, TrustError> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrate(&mut conn)?;
        Ok(Self::wrap(conn))
    }

    fn wrap(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// A poisoned lock is an error for the caller, not a second panic in the run thread.
    fn lock(&self) -> Result<MutexGuard<'_, Connection>, TrustError> {
        self.conn.lock().map_err(|_| {
            TrustError(
                "the script review store is unavailable after a panic in another thread".into(),
            )
        })
    }

    /// Marks this version as reviewed, with the grant the person chose. Reviewing a version again
    /// replaces its grant.
    pub fn review(
        &self,
        hash: &str,
        workflow: &str,
        auto_approve: bool,
        now: i64,
    ) -> Result<(), TrustError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO script_review (hash, workflow, reviewed_at, auto_approve)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (hash) DO UPDATE SET
                 workflow = excluded.workflow,
                 reviewed_at = excluded.reviewed_at,
                 auto_approve = excluded.auto_approve",
            params![hash, workflow, now, auto_approve],
        )?;
        // The newest by review time stay; a tie keeps the one written last (higher rowid).
        tx.execute(
            "DELETE FROM script_review WHERE hash IN (
                 SELECT hash FROM script_review
                 ORDER BY reviewed_at DESC, rowid DESC
                 LIMIT -1 OFFSET ?1
             )",
            params![i64::try_from(RETAINED_REVIEWS).unwrap_or(i64::MAX)],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The review of this version, if the person reviewed it.
    pub fn get(&self, hash: &str) -> Result<Option<Review>, TrustError> {
        let conn = self.lock()?;
        let review = conn
            .query_row(
                "SELECT hash, workflow, reviewed_at, auto_approve FROM script_review
                 WHERE hash = ?1",
                params![hash],
                |row| {
                    Ok(Review {
                        hash: row.get(0)?,
                        workflow: row.get(1)?,
                        reviewed_at: row.get(2)?,
                        auto_approve: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(review)
    }

    /// Whether this version was reviewed.
    pub fn is_reviewed(&self, hash: &str) -> Result<bool, TrustError> {
        let conn = self.lock()?;
        let found: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM script_review WHERE hash = ?1)",
            params![hash],
            |row| row.get(0),
        )?;
        Ok(found)
    }

    /// Whether event runs of this version apply without asking. `false` for a version that was
    /// not reviewed.
    pub fn auto_approve(&self, hash: &str) -> Result<bool, TrustError> {
        Ok(self.get(hash)?.is_some_and(|review| review.auto_approve))
    }

    /// Changes the grant of a reviewed version. Refuses a version that was not reviewed: a grant
    /// is only given at a review.
    pub fn set_auto_approve(&self, hash: &str, on: bool) -> Result<(), TrustError> {
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE script_review SET auto_approve = ?2 WHERE hash = ?1",
            params![hash, on],
        )?;
        if changed == 0 {
            return Err(TrustError(format!(
                "this version of the script has not been reviewed: a grant is only given at a review ({hash})"
            )));
        }
        Ok(())
    }

    /// Takes the review back. Returns whether there was one.
    pub fn revoke(&self, hash: &str) -> Result<bool, TrustError> {
        let conn = self.lock()?;
        let removed = conn.execute("DELETE FROM script_review WHERE hash = ?1", params![hash])?;
        Ok(removed > 0)
    }
}

fn open_file(path: &Path) -> Result<Connection, TrustError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TrustError(format!("cannot create {}: {e}", parent.display())))?;
    }
    let mut conn = Connection::open(path)?;
    configure(&conn)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<(), TrustError> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}

/// Creates the review table of an empty database and refuses a store written by another version
/// of the app. The check and the creation run in one transaction.
fn migrate(conn: &mut Connection) -> Result<(), TrustError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(SCHEMA)?;
    let stored: Option<String> = tx
        .query_row(
            "SELECT value FROM script_meta WHERE key = ?1",
            params![SCHEMA_KEY],
            |row| row.get(0),
        )
        .optional()?;
    match stored {
        None => {
            tx.execute(
                "INSERT INTO script_meta (key, value) VALUES (?1, ?2)",
                params![SCHEMA_KEY, SCHEMA_VERSION],
            )?;
        }
        Some(version) if version == SCHEMA_VERSION => {}
        Some(version) => {
            return Err(TrustError(format!(
                "the script review store is version {version}; this app reads version {SCHEMA_VERSION}"
            )));
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::journal::Journal;

    const HASH: &str = "sha256:aaaa";

    fn fresh() -> TrustStore {
        TrustStore::open_in_memory().expect("open in memory")
    }

    #[test]
    fn data_file_follows_xdg_then_home() {
        assert_eq!(
            data_file(Some("/x".into()), Some("/h".into()), false),
            Some(PathBuf::from("/x/gitmaster/gitmaster.db"))
        );
        assert_eq!(
            data_file(Some("rel".into()), Some("/h".into()), false),
            Some(PathBuf::from("/h/.local/share/gitmaster/gitmaster.db"))
        );
        assert_eq!(
            data_file(None, Some("/h".into()), true),
            Some(PathBuf::from(
                "/h/Library/Application Support/gitmaster/gitmaster.db"
            ))
        );
        assert_eq!(data_file(None, None, false), None);
    }

    #[test]
    fn review_round_trips() {
        let s = fresh();
        assert_eq!(s.get(HASH).unwrap(), None);
        assert!(!s.is_reviewed(HASH).unwrap());
        s.review(HASH, "flow", false, 42).unwrap();
        assert!(s.is_reviewed(HASH).unwrap());
        assert_eq!(
            s.get(HASH).unwrap(),
            Some(Review {
                hash: HASH.into(),
                workflow: "flow".into(),
                reviewed_at: 42,
                auto_approve: false,
            })
        );
        assert!(!s.auto_approve(HASH).unwrap());
    }

    #[test]
    fn auto_approve_and_revoke_round_trip() {
        let s = fresh();
        s.review(HASH, "flow", false, 1).unwrap();
        s.set_auto_approve(HASH, true).unwrap();
        assert!(s.auto_approve(HASH).unwrap());
        s.set_auto_approve(HASH, false).unwrap();
        assert!(!s.auto_approve(HASH).unwrap());
        assert!(s.revoke(HASH).unwrap());
        assert!(!s.is_reviewed(HASH).unwrap());
        assert!(!s.revoke(HASH).unwrap());
    }

    #[test]
    fn re_review_replaces_the_grant() {
        let s = fresh();
        s.review(HASH, "flow", true, 1).unwrap();
        assert!(s.auto_approve(HASH).unwrap());
        s.review(HASH, "renamed", false, 9).unwrap();
        let review = s.get(HASH).unwrap().expect("reviewed");
        assert_eq!(review.workflow, "renamed");
        assert_eq!(review.reviewed_at, 9);
        assert!(!review.auto_approve);
    }

    #[test]
    fn an_unreviewed_hash_is_not_auto_approved() {
        let s = fresh();
        assert!(!s.auto_approve("sha256:never").unwrap());
        let error = s.set_auto_approve("sha256:never", true).unwrap_err();
        assert!(error.0.contains("only given at a review"), "{error}");
        assert!(!s.is_reviewed("sha256:never").unwrap());
    }

    #[test]
    fn a_grant_is_tied_to_its_hash() {
        let s = fresh();
        s.review(HASH, "flow", true, 1).unwrap();
        assert!(!s.auto_approve("sha256:edited").unwrap());
    }

    #[test]
    fn a_file_store_survives_reopening() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("gitmaster.db");
        {
            let s = TrustStore::open(&path).expect("open creates the parent");
            s.review(HASH, "persist", true, 5).unwrap();
        }
        let s = TrustStore::open(&path).expect("reopen");
        assert!(s.is_reviewed(HASH).unwrap());
        assert!(s.auto_approve(HASH).unwrap());
    }

    #[test]
    fn trust_coexists_with_the_journal_in_either_order() {
        for trust_first in [true, false] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("gitmaster.db");
            let (trust, journal) = if trust_first {
                let t = TrustStore::open(&path).expect("trust");
                let j = Journal::open(&path).expect("journal");
                (t, j)
            } else {
                let j = Journal::open(&path).expect("journal");
                let t = TrustStore::open(&path).expect("trust");
                (t, j)
            };
            trust.review(HASH, "both", true, 1).unwrap();
            drop(journal);
            let reopened = TrustStore::open(&path).expect("reopen");
            assert!(reopened.auto_approve(HASH).unwrap());
        }
    }

    #[test]
    fn pruning_keeps_the_newest_reviews() {
        let s = fresh();
        let total = RETAINED_REVIEWS + 3;
        for i in 0..total {
            s.review(
                &format!("sha256:{i}"),
                "prune",
                false,
                i64::try_from(i).expect("small"),
            )
            .unwrap();
        }
        for i in 0..3 {
            assert!(!s.is_reviewed(&format!("sha256:{i}")).unwrap(), "{i} kept");
        }
        assert!(s.is_reviewed(&format!("sha256:{}", 3)).unwrap());
        assert!(s.is_reviewed(&format!("sha256:{}", total - 1)).unwrap());
        let conn = s.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM script_review", [], |row| row.get(0))
            .unwrap();
        assert_eq!(usize::try_from(count).expect("count"), RETAINED_REVIEWS);
    }

    #[test]
    fn pruning_breaks_ties_by_insertion_order() {
        let s = fresh();
        for i in 0..=RETAINED_REVIEWS {
            s.review(&format!("sha256:{i}"), "tie", false, 7).unwrap();
        }
        assert!(!s.is_reviewed("sha256:0").unwrap());
        assert!(s.is_reviewed("sha256:1").unwrap());
    }

    #[test]
    fn concurrent_writers_lose_nothing() {
        let s = fresh();
        let handles: Vec<_> = (0..8_i64)
            .map(|t| {
                let s = s.clone();
                std::thread::spawn(move || {
                    for n in 0..50_i64 {
                        let hash = format!("sha256:{t}-{n}");
                        s.review(&hash, "parallel", n % 2 == 0, n).expect("review");
                        assert!(s.is_reviewed(&hash).expect("read"));
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread");
        }
        for t in 0..8_i64 {
            for n in 0..50_i64 {
                let review = s.get(&format!("sha256:{t}-{n}")).unwrap().expect("kept");
                assert_eq!(review.auto_approve, n % 2 == 0);
            }
        }
    }

    #[test]
    fn a_poisoned_lock_is_an_error() {
        let s = fresh();
        let shared = s.conn.clone();
        let handle = std::thread::spawn(move || {
            let _guard = shared.lock().expect("lock");
            panic!("poison the review store");
        });
        assert!(handle.join().is_err());
        assert!(s.get(HASH).is_err());
        assert!(s.review(HASH, "x", false, 1).is_err());
        assert!(s.revoke(HASH).is_err());
    }

    #[test]
    fn a_store_written_by_another_version_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gitmaster.db");
        drop(TrustStore::open(&path).expect("create"));
        {
            let raw = Connection::open(&path).expect("raw");
            raw.execute(
                "UPDATE script_meta SET value = '9' WHERE key = ?1",
                params![SCHEMA_KEY],
            )
            .expect("bump");
        }
        let error = TrustStore::open(&path).err().expect("refused");
        assert!(error.0.contains("version 9"), "{error}");
    }
}

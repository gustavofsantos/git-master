//! Code health: the index in `.git/gitmaster/health.db` (see mod.rs; D225 (5)).
//!
//! One row per commit and one per changed path, keyed by the commit id. Paths are
//! interned, so a path that many commits change is stored once. The index holds
//! facts only: no configuration and no score, so it never needs rebuilding. Two
//! processes may write the same file (the app and `gitmaster mcp`): each commit is
//! inserted in an immediate transaction, and its changes only when its row was new.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, Transaction, TransactionBehavior, params, params_from_iter};

use super::{CommitRecord, FileChange};

/// Schema version written to `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 1;
/// How long a writer waits for another connection's lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Keys per query. Well under SQLite's bound-parameter limit.
const CHUNK: usize = 500;

const SCHEMA: &str = "
CREATE TABLE commits (
    sha TEXT PRIMARY KEY,
    time INTEGER NOT NULL,
    author TEXT NOT NULL,
    parents INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE paths (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE);
CREATE TABLE changes (
    sha TEXT NOT NULL,
    path_id INTEGER NOT NULL,
    old_path_id INTEGER,
    added INTEGER,
    deleted INTEGER
);
CREATE INDEX changes_by_sha ON changes(sha);
";

/// The index file of a repository: `<common_dir>/gitmaster/health.db`.
#[must_use]
pub fn db_path(common_dir: &Path) -> PathBuf {
    common_dir.join("gitmaster").join("health.db")
}

/// The open index. Errors are messages, ready to show.
pub struct HealthStore {
    conn: Connection,
}

/// `prefix (?,?,...)` with `count` placeholders.
fn in_list(prefix: &str, count: usize) -> String {
    let marks = vec!["?"; count].join(",");
    format!("{prefix} ({marks})")
}

impl HealthStore {
    /// Opens the index at `path`, creating its folder and its schema when missing.
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let mut conn =
            Connection::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        prepare(&mut conn).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        Ok(Self { conn })
    }

    /// An index in memory, for tests.
    pub fn open_in_memory() -> Result<Self, String> {
        let mut conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        prepare(&mut conn)?;
        Ok(Self { conn })
    }

    /// Inserts the commits that are not yet in the index, with their changes, in one
    /// transaction. Returns how many commits were new.
    pub fn insert(&mut self, commits: &[CommitRecord]) -> Result<usize, String> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut paths = HashMap::new();
        let mut new = 0;
        for commit in commits {
            let inserted = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO commits (sha, time, author, parents)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| e.to_string())?
                .execute(params![
                    commit.sha,
                    commit.time,
                    commit.author,
                    i64::from(commit.parents)
                ])
                .map_err(|e| e.to_string())?;
            if inserted == 0 {
                continue;
            }
            new += 1;
            for change in &commit.changes {
                let path_id = intern(&tx, &mut paths, &change.path)?;
                let old_path_id = change
                    .old_path
                    .as_deref()
                    .map(|old| intern(&tx, &mut paths, old))
                    .transpose()?;
                tx.prepare_cached(
                    "INSERT INTO changes (sha, path_id, old_path_id, added, deleted)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(|e| e.to_string())?
                .execute(params![
                    commit.sha,
                    path_id,
                    old_path_id,
                    change.added.map(i64::from),
                    change.deleted.map(i64::from),
                ])
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(new)
    }

    /// The shas of `shas` that the index does not hold, in the input order.
    pub fn missing(&self, shas: &[String]) -> Result<Vec<String>, String> {
        let known = self.known(shas)?;
        Ok(shas
            .iter()
            .filter(|sha| !known.contains(sha.as_str()))
            .cloned()
            .collect())
    }

    /// The commits of `shas` that the index holds, in the input order, each with its
    /// changes in insertion order. Unknown shas are skipped.
    pub fn load(&self, shas: &[String]) -> Result<Vec<CommitRecord>, String> {
        let mut unique = shas.to_vec();
        unique.sort_unstable();
        unique.dedup();
        let mut commits = HashMap::new();
        let mut changes: HashMap<String, Vec<FileChange>> = HashMap::new();
        for chunk in unique.chunks(CHUNK) {
            let mut stmt = self
                .conn
                .prepare_cached(&in_list(
                    "SELECT sha, time, author, parents FROM commits WHERE sha IN",
                    chunk.len(),
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params_from_iter(chunk.iter()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (sha, time, author, parents) = row.map_err(|e| e.to_string())?;
                let parents = u32::try_from(parents)
                    .map_err(|_| format!("commit {sha} has {parents} parents"))?;
                commits.insert(sha, (time, author, parents));
            }

            let mut stmt = self
                .conn
                .prepare_cached(&format!(
                    "{} ORDER BY c.rowid",
                    in_list(
                        "SELECT c.sha, p.path, o.path, c.added, c.deleted FROM changes c
                         JOIN paths p ON p.id = c.path_id
                         LEFT JOIN paths o ON o.id = c.old_path_id
                         WHERE c.sha IN",
                        chunk.len(),
                    )
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params_from_iter(chunk.iter()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        FileChange {
                            path: row.get(1)?,
                            old_path: row.get(2)?,
                            added: count(row.get(3)?),
                            deleted: count(row.get(4)?),
                        },
                    ))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (sha, change) = row.map_err(|e| e.to_string())?;
                changes.entry(sha).or_default().push(change);
            }
        }
        Ok(shas
            .iter()
            .filter_map(|sha| {
                let (time, author, parents) = commits.get(sha)?;
                Some(CommitRecord {
                    sha: sha.clone(),
                    time: *time,
                    author: author.clone(),
                    parents: *parents,
                    changes: changes.get(sha).cloned().unwrap_or_default(),
                })
            })
            .collect())
    }

    /// The shas of `shas` that the index holds, as a set.
    fn known(&self, shas: &[String]) -> Result<HashSet<String>, String> {
        let mut known = HashSet::new();
        for chunk in shas.chunks(CHUNK) {
            let mut stmt = self
                .conn
                .prepare_cached(&in_list(
                    "SELECT sha FROM commits WHERE sha IN",
                    chunk.len(),
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params_from_iter(chunk.iter()), |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                known.insert(row.map_err(|e| e.to_string())?);
            }
        }
        Ok(known)
    }
}

/// A stored line count: `NULL` for a binary file.
fn count(value: Option<i64>) -> Option<u32> {
    value.and_then(|v| u32::try_from(v).ok())
}

/// The id of `path` in `paths`, added when new. `cache` spares the lookups of one insert.
fn intern(
    tx: &Transaction<'_>,
    cache: &mut HashMap<String, i64>,
    path: &str,
) -> Result<i64, String> {
    if let Some(id) = cache.get(path) {
        return Ok(*id);
    }
    tx.prepare_cached("INSERT OR IGNORE INTO paths (path) VALUES (?1)")
        .map_err(|e| e.to_string())?
        .execute([path])
        .map_err(|e| e.to_string())?;
    let id = tx
        .prepare_cached("SELECT id FROM paths WHERE path = ?1")
        .map_err(|e| e.to_string())?
        .query_row([path], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    cache.insert(path.to_owned(), id);
    Ok(id)
}

fn prepare(conn: &mut Connection) -> Result<(), String> {
    conn.busy_timeout(BUSY_TIMEOUT).map_err(|e| e.to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    migrate(conn)
}

/// Creates the schema of a new file. Runs in an immediate transaction, so two processes
/// opening a new file create it once.
fn migrate(conn: &mut Connection) -> Result<(), String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let version: i64 = tx
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    match version {
        0 => {
            tx.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(|e| e.to_string())?;
        }
        SCHEMA_VERSION => {}
        other => {
            return Err(format!(
                "the health index has schema version {other}; this build reads {SCHEMA_VERSION}"
            ));
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn sha(i: usize) -> String {
        format!("{i:040x}")
    }

    fn commit(i: usize, changes: Vec<FileChange>) -> CommitRecord {
        CommitRecord {
            sha: sha(i),
            time: 1_700_000_000 + i64::try_from(i).unwrap() * 60,
            author: "0123456789abcdef".to_owned(),
            parents: 1,
            changes,
        }
    }

    fn change(path: &str) -> FileChange {
        FileChange {
            path: path.to_owned(),
            old_path: None,
            added: Some(1),
            deleted: Some(0),
        }
    }

    fn row_count(store: &HealthStore, table: &str) -> i64 {
        store
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn db_path_is_under_the_gitmaster_folder() {
        assert_eq!(
            db_path(Path::new("/r/.git")),
            Path::new("/r/.git/gitmaster/health.db")
        );
    }

    #[test]
    fn a_commit_round_trips_with_its_changes_in_order() {
        let mut store = HealthStore::open_in_memory().unwrap();
        let mut rename = change("new name.txt");
        rename.old_path = Some("old name.txt".to_owned());
        let mut binary = change("ünï/bin.dat");
        binary.added = None;
        binary.deleted = None;
        let first = commit(1, vec![change("a b.txt"), rename, binary]);
        let merge = CommitRecord {
            parents: 2,
            ..commit(2, Vec::new())
        };
        assert_eq!(store.insert(&[first.clone(), merge.clone()]).unwrap(), 2);

        let loaded = store.load(&[sha(2), sha(9), sha(1)]).unwrap();
        assert_eq!(loaded, vec![merge, first]);
    }

    #[test]
    fn insert_counts_only_new_commits_and_never_duplicates_changes() {
        let mut store = HealthStore::open_in_memory().unwrap();
        let record = commit(1, vec![change("a.txt"), change("b.txt")]);
        assert_eq!(store.insert(std::slice::from_ref(&record)).unwrap(), 1);
        assert_eq!(store.insert(&[record.clone(), record]).unwrap(), 0);
        assert_eq!(row_count(&store, "changes"), 2);
        assert_eq!(row_count(&store, "paths"), 2);
    }

    #[test]
    fn missing_keeps_the_input_order() {
        let mut store = HealthStore::open_in_memory().unwrap();
        store.insert(&[commit(2, Vec::new())]).unwrap();
        let input = [sha(3), sha(2), sha(1)];
        assert_eq!(store.missing(&input).unwrap(), vec![sha(3), sha(1)]);
        assert!(store.missing(&[]).unwrap().is_empty());
    }

    #[test]
    fn a_path_is_stored_once_however_many_commits_change_it() {
        let mut store = HealthStore::open_in_memory().unwrap();
        let commits: Vec<_> = (0..50)
            .map(|i| commit(i, vec![change("same.txt")]))
            .collect();
        store.insert(&commits).unwrap();
        assert_eq!(row_count(&store, "paths"), 1);
        assert_eq!(row_count(&store, "changes"), 50);
    }

    #[test]
    fn a_new_file_gets_the_schema_and_reopens_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = db_path(dir.path());
        {
            let mut store = HealthStore::open(&path).unwrap();
            store.insert(&[commit(1, vec![change("a.txt")])]).unwrap();
        }
        let store = HealthStore::open(&path).unwrap();
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(store.load(&[sha(1)]).unwrap().len(), 1);
    }

    #[test]
    fn a_file_from_a_newer_build_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("health.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        let error = HealthStore::open(&path).err().unwrap();
        assert!(error.contains("schema version 99"), "{error}");
    }

    #[test]
    fn missing_is_fast_for_300k_shas() {
        let mut store = HealthStore::open_in_memory().unwrap();
        let known: Vec<_> = (0..300_000)
            .step_by(2)
            .map(|i| commit(i, Vec::new()))
            .collect();
        store.insert(&known).unwrap();
        let shas: Vec<String> = (0..300_000).map(sha).collect();
        let started = Instant::now();
        let missing = store.missing(&shas).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(missing.len(), 150_000);
        assert!(elapsed < Duration::from_secs(5), "missing took {elapsed:?}");
    }

    #[test]
    fn load_is_fast_for_200k_commits_with_five_changes() {
        let mut store = HealthStore::open_in_memory().unwrap();
        let commits: Vec<_> = (0..200_000)
            .map(|i| {
                let changes = (0..5)
                    .map(|f| change(&format!("dir{}/file{f}.rs", i % 97)))
                    .collect();
                commit(i, changes)
            })
            .collect();
        store.insert(&commits).unwrap();
        let shas: Vec<String> = (0..200_000).map(sha).collect();
        let started = Instant::now();
        let loaded = store.load(&shas).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(loaded.len(), 200_000);
        assert!(loaded.iter().all(|c| c.changes.len() == 5));
        assert!(elapsed < Duration::from_secs(20), "load took {elapsed:?}");
    }
}

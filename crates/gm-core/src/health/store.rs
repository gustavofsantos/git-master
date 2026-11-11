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

use super::{CommitRecord, FileChange, FirstParentCommit, Hunk};

/// Schema version written to `PRAGMA user_version`. Version 1 holds the commits and their
/// changes; version 2 adds the hunk headers of the first-parent commits (D228); version 3 adds
/// the binary marker to those hunks, so version 2's hunks are emptied and read again.
const SCHEMA_VERSION: i64 = 3;
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

/// Added by version 2: the first-parent commits whose hunks are stored (a row counts even
/// when the commit has none), and their hunk headers in order (`seq`). `parent` is written
/// as NULL: the replay needs no parent, and the column is kept for a later need.
const SCHEMA_V2: &str = "
CREATE TABLE fp_commits (sha TEXT PRIMARY KEY, parent TEXT) WITHOUT ROWID;
CREATE TABLE hunks (
    sha TEXT NOT NULL,
    seq INTEGER NOT NULL,
    path_id INTEGER NOT NULL,
    old_path_id INTEGER,
    old_start INTEGER NOT NULL,
    old_count INTEGER NOT NULL,
    new_start INTEGER NOT NULL,
    new_count INTEGER NOT NULL
);
CREATE INDEX hunks_by_sha ON hunks(sha);
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
        let known = self.known("commits", shas)?;
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

    /// Inserts the first-parent commits that the index does not hold yet, with their hunks,
    /// in one transaction. A commit already held keeps the hunks it has. Returns how many
    /// commits were new.
    pub fn insert_hunks(&mut self, commits: &[FirstParentCommit]) -> Result<usize, String> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut paths = HashMap::new();
        let mut new = 0;
        for commit in commits {
            let inserted = tx
                .prepare_cached("INSERT OR IGNORE INTO fp_commits (sha, parent) VALUES (?1, NULL)")
                .map_err(|e| e.to_string())?
                .execute([&commit.sha])
                .map_err(|e| e.to_string())?;
            if inserted == 0 {
                continue;
            }
            new += 1;
            for (seq, hunk) in commit.hunks.iter().enumerate() {
                let seq = i64::try_from(seq).map_err(|e| e.to_string())?;
                let path_id = intern(&tx, &mut paths, &hunk.path)?;
                let old_path_id = hunk
                    .old_path
                    .as_deref()
                    .map(|old| intern(&tx, &mut paths, old))
                    .transpose()?;
                tx.prepare_cached(
                    "INSERT INTO hunks (sha, seq, path_id, old_path_id, old_start, old_count,
                                        new_start, new_count)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )
                .map_err(|e| e.to_string())?
                .execute(params![
                    commit.sha,
                    seq,
                    path_id,
                    old_path_id,
                    i64::from(hunk.old_start),
                    i64::from(hunk.old_count),
                    i64::from(hunk.new_start),
                    i64::from(hunk.new_count),
                ])
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(new)
    }

    /// The shas of `shas` whose hunks the index does not hold, in the input order.
    pub fn missing_hunks(&self, shas: &[String]) -> Result<Vec<String>, String> {
        let known = self.known("fp_commits", shas)?;
        Ok(shas
            .iter()
            .filter(|sha| !known.contains(sha.as_str()))
            .cloned()
            .collect())
    }

    /// The first-parent commits of `shas` that the index holds, in the input order, each with
    /// its hunks in order. Unknown shas are skipped.
    pub fn load_hunks(&self, shas: &[String]) -> Result<Vec<FirstParentCommit>, String> {
        let known = self.known("fp_commits", shas)?;
        let mut present: Vec<String> = shas
            .iter()
            .filter(|sha| known.contains(sha.as_str()))
            .cloned()
            .collect();
        present.sort_unstable();
        present.dedup();
        let mut hunks: HashMap<String, Vec<Hunk>> = HashMap::new();
        for chunk in present.chunks(CHUNK) {
            let mut stmt = self
                .conn
                .prepare_cached(&format!(
                    "{} ORDER BY h.seq",
                    in_list(
                        "SELECT h.sha, p.path, o.path, h.old_start, h.old_count,
                                h.new_start, h.new_count
                         FROM hunks h
                         JOIN paths p ON p.id = h.path_id
                         LEFT JOIN paths o ON o.id = h.old_path_id
                         WHERE h.sha IN",
                        chunk.len(),
                    )
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params_from_iter(chunk.iter()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        [
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                        ],
                    ))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (sha, path, old_path, [old_start, old_count, new_start, new_count]) =
                    row.map_err(|e| e.to_string())?;
                hunks.entry(sha).or_default().push(Hunk {
                    path,
                    old_path,
                    old_start: hunk_number(old_start)?,
                    old_count: hunk_number(old_count)?,
                    new_start: hunk_number(new_start)?,
                    new_count: hunk_number(new_count)?,
                });
            }
        }
        Ok(shas
            .iter()
            .filter(|sha| known.contains(sha.as_str()))
            .map(|sha| FirstParentCommit {
                sha: sha.clone(),
                hunks: hunks.get(sha).cloned().unwrap_or_default(),
            })
            .collect())
    }

    /// The shas of `shas` that `table` (`commits` or `fp_commits`) holds, as a set.
    fn known(&self, table: &str, shas: &[String]) -> Result<HashSet<String>, String> {
        let mut known = HashSet::new();
        for chunk in shas.chunks(CHUNK) {
            let mut stmt = self
                .conn
                .prepare_cached(&in_list(
                    &format!("SELECT sha FROM {table} WHERE sha IN"),
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

/// A stored hunk number, which is never negative.
fn hunk_number(value: i64) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("the index holds a hunk number of {value}"))
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

/// Creates the schema of a new file, or brings an older one up to `SCHEMA_VERSION` and keeps
/// its rows. Runs in an immediate transaction, so two processes opening the same file migrate
/// it once.
fn migrate(conn: &mut Connection) -> Result<(), String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let version: i64 = tx
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if !(0..=SCHEMA_VERSION).contains(&version) {
        return Err(format!(
            "the health index has schema version {version}; this build reads {SCHEMA_VERSION}"
        ));
    }
    if version < 1 {
        tx.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    }
    if version < 2 {
        tx.execute_batch(SCHEMA_V2).map_err(|e| e.to_string())?;
    }
    if version == 2 {
        // Hunks read before the binary marker existed lack it: they are read again. The
        // numstat rows are kept.
        tx.execute_batch("DELETE FROM hunks; DELETE FROM fp_commits;")
            .map_err(|e| e.to_string())?;
    }
    if version != SCHEMA_VERSION {
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|e| e.to_string())?;
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

    fn hunk(path: &str, old_start: u32, old_count: u32, new_count: u32) -> Hunk {
        Hunk {
            path: path.to_owned(),
            old_path: None,
            old_start,
            old_count,
            new_start: old_start,
            new_count,
        }
    }

    fn first_parent(i: usize, hunks: Vec<Hunk>) -> FirstParentCommit {
        FirstParentCommit { sha: sha(i), hunks }
    }

    #[test]
    fn first_parent_hunks_round_trip_in_order_with_renames_and_empty_commits() {
        let mut store = HealthStore::open_in_memory().unwrap();
        let rename_only = Hunk {
            old_path: Some("old name.txt".to_owned()),
            ..hunk("new name.txt", 0, 0, 0)
        };
        let renamed = Hunk {
            old_path: Some("a.txt".to_owned()),
            ..hunk("b.txt", 2, 1, 3)
        };
        let first = first_parent(1, vec![hunk("ünï/x.rs", 0, 0, 4), rename_only, renamed]);
        let empty = first_parent(2, Vec::new());
        assert_eq!(
            store.insert_hunks(&[first.clone(), empty.clone()]).unwrap(),
            2
        );

        assert!(store.missing_hunks(&[sha(1), sha(2)]).unwrap().is_empty());
        assert_eq!(
            store.missing_hunks(&[sha(3), sha(2), sha(1)]).unwrap(),
            vec![sha(3)]
        );
        let loaded = store.load_hunks(&[sha(2), sha(9), sha(1)]).unwrap();
        assert_eq!(loaded, vec![empty, first]);
    }

    #[test]
    fn inserting_hunks_again_keeps_the_first_ones() {
        let mut store = HealthStore::open_in_memory().unwrap();
        let first = first_parent(1, vec![hunk("a.txt", 1, 2, 3)]);
        assert_eq!(store.insert_hunks(std::slice::from_ref(&first)).unwrap(), 1);
        let again = first_parent(1, vec![hunk("a.txt", 5, 5, 5), hunk("b.txt", 1, 1, 1)]);
        assert_eq!(store.insert_hunks(&[again, first.clone()]).unwrap(), 0);
        assert_eq!(store.load_hunks(&[sha(1)]).unwrap(), vec![first]);
        assert_eq!(row_count(&store, "hunks"), 1);
    }

    #[test]
    fn a_version_one_file_migrates_to_version_two_and_keeps_its_commits() {
        let dir = tempfile::tempdir().unwrap();
        let path = db_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let record = commit(1, vec![change("a.txt"), change("b.txt")]);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            let mut old = HealthStore { conn };
            assert_eq!(old.insert(std::slice::from_ref(&record)).unwrap(), 1);
        }

        let store = HealthStore::open(&path).unwrap();
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(store.load(&[sha(1)]).unwrap(), vec![record]);
        assert_eq!(store.missing(&[sha(1)]).unwrap(), Vec::<String>::new());
        assert_eq!(store.missing_hunks(&[sha(1)]).unwrap(), vec![sha(1)]);
        assert!(store.load_hunks(&[sha(1)]).unwrap().is_empty());
    }

    #[test]
    fn a_version_two_file_keeps_its_commits_and_empties_its_hunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = db_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let record = commit(1, vec![change("a.txt")]);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(SCHEMA_V2).unwrap();
            conn.pragma_update(None, "user_version", 2).unwrap();
            let mut old = HealthStore { conn };
            old.insert(std::slice::from_ref(&record)).unwrap();
            old.insert_hunks(&[first_parent(1, vec![hunk("a.txt", 0, 0, 1)])])
                .unwrap();
            assert_eq!(row_count(&old, "hunks"), 1);
        }

        let store = HealthStore::open(&path).unwrap();
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
        assert_eq!(store.load(&[sha(1)]).unwrap(), vec![record]);
        assert_eq!(row_count(&store, "hunks"), 0);
        assert_eq!(row_count(&store, "fp_commits"), 0);
        assert_eq!(store.missing_hunks(&[sha(1)]).unwrap(), vec![sha(1)]);
    }
}

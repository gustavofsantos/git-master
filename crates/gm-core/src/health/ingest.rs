//! Code health: reading the history of a repository into the index (see mod.rs;
//! docs/guide/code-health.md, "Building the index"). Every call is a read of git:
//! nothing is written to the repository, and no commit-graph is written (D32).

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use gm_schema::HealthProgress;
use sha2::{Digest, Sha256};

use super::store::HealthStore;
use super::{CommitRecord, FileChange};

/// Commits read per `git log` process and per transaction.
const BATCH: usize = 1000;
/// Starts each commit of the `git log` output.
const COMMIT_START: u8 = 0x01;
/// Bytes read from git's output per step; the cancel flag is checked between steps.
const READ_CHUNK: usize = 64 * 1024;

fn git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(["--no-optional-locks", "-c", "core.quotepath=off"])
        .env("LC_ALL", "C");
    command
}

/// The full id and committer time (seconds) of `rev`.
pub fn resolve(root: &Path, rev: &str) -> Result<(String, i64), String> {
    let output = git(root)
        .args(["log", "-1", "--format=%H%x00%ct", rev, "--"])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot resolve {rev}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut fields = text.trim_end().split('\0');
    let sha = fields.next().unwrap_or_default();
    let time = fields
        .next()
        .and_then(|time| time.parse::<i64>().ok())
        .ok_or_else(|| format!("git printed no commit time for {rev}"))?;
    if sha.is_empty() {
        return Err(format!("git printed no commit for {rev}"));
    }
    Ok((sha.to_owned(), time))
}

/// The commits reachable from `sha` with a committer time at or after `since`
/// (seconds), newest first.
pub fn window(root: &Path, sha: &str, since: i64) -> Result<Vec<String>, String> {
    // `--max-age` takes the epoch seconds as they are. `--since` parses a date, and
    // git reads a small epoch (below about 1973) as a date it cannot parse, which
    // falls back to now and hides every commit.
    let output = git(root)
        .args(["rev-list", &format!("--max-age={since}"), sha, "--"])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot list the history of {sha}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Reads the commits `shas`, in that order, with their changes, in one `git log`
/// process. An empty list reads nothing: git would otherwise read HEAD from an empty input.
pub fn read_commits(
    root: &Path,
    shas: &[String],
    cancel: &AtomicBool,
) -> Result<Vec<CommitRecord>, String> {
    if shas.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = git(root)
        .args([
            "log",
            "--no-walk=unsorted",
            "--stdin",
            "-z",
            "--numstat",
            "-M",
            "--no-ext-diff",
            "--format=%x01%H%x00%P%x00%ct%x00%aE",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;

    // The shas go in on a thread of their own: a long list must not fill git's
    // stdout pipe while we are still writing its stdin.
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "git log has no stdin".to_owned())?;
    let input: Vec<u8> = shas
        .iter()
        .flat_map(|sha| sha.bytes().chain(std::iter::once(b'\n')))
        .collect();
    let writer = std::thread::spawn(move || {
        // A write error means git exited early; its stderr says why.
        let _ = stdin.write_all(&input);
    });
    let Some(mut stderr) = child.stderr.take() else {
        let _ = child.kill();
        return Err("git log has no stderr".to_owned());
    };
    let errors = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text);
        text
    });

    let bytes = read_stdout(&mut child, cancel);
    if !matches!(bytes, Ok(Some(_))) {
        let _ = child.kill();
    }
    let status = child.wait();
    let _ = writer.join();
    let stderr = errors.join().unwrap_or_default();
    let bytes = match bytes {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Err("cancelled".to_owned()),
        Err(error) => return Err(error),
    };
    let status = status.map_err(|e| format!("cannot wait for git: {e}"))?;
    if !status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    parse_log(&bytes)
}

/// Reads git's stdout to its end. `Ok(None)` when `cancel` was set first.
fn read_stdout(child: &mut Child, cancel: &AtomicBool) -> Result<Option<Vec<u8>>, String> {
    let stdout = child
        .stdout
        .as_mut()
        .ok_or_else(|| "git log has no stdout".to_owned())?;
    let mut bytes = Vec::new();
    let mut chunk = vec![0; READ_CHUNK];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match stdout.read(&mut chunk) {
            Ok(0) => return Ok(Some(bytes)),
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(error) => return Err(format!("cannot read git's output: {error}")),
        }
    }
}

/// Parses the output of `git log --no-walk --stdin -z --numstat -M --format=%x01%H%x00%P%x00%ct%x00%aE`.
/// Each commit is `\x01 sha NUL parents NUL time NUL email NUL`, an optional newline,
/// then its numstat entries, back to back: `added TAB deleted TAB path NUL`, or for a
/// rename or copy `added TAB deleted TAB NUL old NUL new NUL`. `-` counts a binary file.
pub fn parse_log(bytes: &[u8]) -> Result<Vec<CommitRecord>, String> {
    let mut at = 0;
    let mut commits = Vec::new();
    while at < bytes.len() {
        if bytes[at] != COMMIT_START {
            return Err(format!("git log output: expected a commit at byte {at}"));
        }
        at += 1;
        let sha = text(take(bytes, &mut at, 0)?);
        let parents = text(take(bytes, &mut at, 0)?);
        let time = text(take(bytes, &mut at, 0)?);
        let email = text(take(bytes, &mut at, 0)?);
        let time = time
            .parse::<i64>()
            .map_err(|_| format!("git log output: bad commit time {time:?} for {sha}"))?;
        let parents = u32::try_from(parents.split_whitespace().count())
            .map_err(|_| format!("git log output: too many parents for {sha}"))?;

        if bytes.get(at) == Some(&b'\n') {
            at += 1;
        }
        let mut changes = Vec::new();
        while at < bytes.len() && bytes[at] != COMMIT_START {
            let added = count(take(bytes, &mut at, b'\t')?)?;
            let deleted = count(take(bytes, &mut at, b'\t')?)?;
            let change = if bytes.get(at) == Some(&0) {
                at += 1;
                let old = text(take(bytes, &mut at, 0)?);
                let path = text(take(bytes, &mut at, 0)?);
                FileChange {
                    path,
                    old_path: Some(old),
                    added,
                    deleted,
                }
            } else {
                FileChange {
                    path: text(take(bytes, &mut at, 0)?),
                    old_path: None,
                    added,
                    deleted,
                }
            };
            changes.push(change);
        }
        commits.push(CommitRecord {
            sha,
            time,
            author: author_key(&email),
            parents,
            changes,
        });
    }
    Ok(commits)
}

/// The field up to `stop` (consumed, not returned); an error when git's output ends first.
fn take<'a>(bytes: &'a [u8], at: &mut usize, stop: u8) -> Result<&'a [u8], String> {
    let rest = &bytes[*at..];
    let end = rest
        .iter()
        .position(|&byte| byte == stop)
        .ok_or_else(|| "git log output ended in the middle of a record".to_owned())?;
    *at += end + 1;
    Ok(&rest[..end])
}

fn text(field: &[u8]) -> String {
    String::from_utf8_lossy(field).into_owned()
}

/// A numstat count: `None` for `-`, a binary file.
fn count(field: &[u8]) -> Result<Option<u32>, String> {
    if field == b"-" {
        return Ok(None);
    }
    std::str::from_utf8(field)
        .ok()
        .and_then(|digits| digits.parse::<u32>().ok())
        .map(Some)
        .ok_or_else(|| format!("git log output: bad line count {:?}", text(field)))
}

/// The first 16 hex digits of the SHA-256 of the lowercased address (D225 (5)).
fn author_key(email: &str) -> String {
    let digest = Sha256::digest(email.to_lowercase().as_bytes());
    hex(&digest[..8])
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Reads the commits of `root`'s window that the index lacks, newest first, in batches,
/// and inserts them. `progress` is called after each batch. Cancelling returns
/// `Err("cancelled")`; what was inserted stays, and a later call resumes.
pub fn ingest(
    root: &Path,
    store: &mut HealthStore,
    sha: &str,
    since: i64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(HealthProgress),
) -> Result<HealthProgress, String> {
    let shas = window(root, sha, since)?;
    let missing = store.missing(&shas)?;
    let total = shas.len();
    let already = total - missing.len();
    let mut done = 0;
    progress(report(total, already + done));
    for batch in missing.chunks(BATCH) {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_owned());
        }
        let records = read_commits(root, batch, cancel)?;
        store.insert(&records)?;
        done += batch.len();
        progress(report(total, already + done));
    }
    Ok(report(total, already + done))
}

fn report(total: usize, indexed: usize) -> HealthProgress {
    HealthProgress {
        indexed: indexed as u64,
        total: total as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_AUTHOR: &str = "e3b0c44298fc1c14";

    fn header(sha: &str, parents: &str, time: i64, email: &str) -> Vec<u8> {
        format!("\x01{sha}\0{parents}\0{time}\0{email}\0").into_bytes()
    }

    #[test]
    fn author_key_is_the_first_16_hex_of_the_sha256_of_the_lowercased_address() {
        // sha256("") starts with e3b0c44298fc1c14.
        assert_eq!(author_key(""), EMPTY_AUTHOR);
        assert_eq!(author_key("A@X.io"), author_key("a@x.io"));
        assert_eq!(author_key("a@x.io").len(), 16);
        assert_ne!(author_key("a@x.io"), author_key("b@x.io"));
    }

    #[test]
    fn a_root_commit_has_no_parents_and_no_changes() {
        let bytes = header("aa", "", 100, "a@x.io");
        let commits = parse_log(&bytes).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].parents, 0);
        assert_eq!(commits[0].time, 100);
        assert!(commits[0].changes.is_empty());
    }

    #[test]
    fn a_merge_counts_its_parents_and_has_no_numstat() {
        let mut bytes = header("mm", "p1 p2", 200, "a@x.io");
        bytes.extend(header("c1", "p0", 100, "a@x.io"));
        bytes.extend(b"\n1\t2\tf.txt\0");
        let commits = parse_log(&bytes).unwrap();
        assert_eq!(commits[0].parents, 2);
        assert!(commits[0].changes.is_empty());
        assert_eq!(commits[1].changes.len(), 1);
    }

    #[test]
    fn paths_with_spaces_tabs_and_unicode_are_kept_whole() {
        let mut bytes = header("aa", "p", 1, "a@x.io");
        bytes.extend(b"\n3\t1\tmy file.txt\0");
        bytes.extend(b"2\t0\tthe\ttab.rs\0");
        bytes.extend("0\t4\tsrc/ünï/ß.txt\0".as_bytes());
        let commits = parse_log(&bytes).unwrap();
        let paths: Vec<_> = commits[0].changes.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, ["my file.txt", "the\ttab.rs", "src/ünï/ß.txt"]);
        assert_eq!(commits[0].changes[2].deleted, Some(4));
    }

    #[test]
    fn a_rename_keeps_the_old_and_new_paths() {
        let mut bytes = header("aa", "p", 1, "a@x.io");
        bytes.extend(b"\n0\t0\t\0old name.txt\0new name.txt\0");
        bytes.extend(b"1\t1\tother.txt\0");
        let commits = parse_log(&bytes).unwrap();
        let changes = &commits[0].changes;
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, "new name.txt");
        assert_eq!(changes[0].old_path.as_deref(), Some("old name.txt"));
        assert_eq!(changes[0].added, Some(0));
        assert_eq!(changes[1].old_path, None);
    }

    #[test]
    fn a_binary_file_has_no_counts() {
        let mut bytes = header("aa", "p", 1, "a@x.io");
        bytes.extend(b"\n-\t-\tlogo.png\0");
        let commits = parse_log(&bytes).unwrap();
        let change = &commits[0].changes[0];
        assert_eq!((change.added, change.deleted), (None, None));
        assert_eq!(change.path, "logo.png");
    }

    #[test]
    fn several_commits_split_at_their_markers() {
        let mut bytes = header("c2", "c1", 20, "A@x.io");
        bytes.extend(b"\n1\t0\ta.txt\0");
        bytes.extend(header("c1", "", 10, "b@x.io"));
        bytes.extend(b"\n2\t0\tb.txt\0");
        let commits = parse_log(&bytes).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "c2");
        assert_eq!(commits[0].author, author_key("a@x.io"));
        assert_eq!(commits[1].sha, "c1");
        assert_eq!(commits[1].changes[0].path, "b.txt");
    }

    #[test]
    fn empty_output_is_no_commits() {
        assert!(parse_log(b"").unwrap().is_empty());
    }

    #[test]
    fn truncated_output_is_an_error() {
        assert!(parse_log(b"\x01aa\0p\0").is_err());
        assert!(parse_log(b"\x01aa\0p\0 1\0a@x\0\n1\t0\tpath-without-nul").is_err());
        assert!(parse_log(b"\x01aa\0p\0nope\0a@x\0").is_err());
        assert!(parse_log(b"junk").is_err());
    }

    #[test]
    fn an_empty_list_reads_nothing_and_never_starts_git() {
        // The root does not need to be a repository: nothing runs.
        let cancel = AtomicBool::new(false);
        let commits = read_commits(Path::new("/nonexistent"), &[], &cancel).unwrap();
        assert!(commits.is_empty());
    }
}

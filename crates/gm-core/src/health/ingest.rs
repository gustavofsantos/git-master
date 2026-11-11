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
use super::{CommitRecord, FileChange, FirstParentCommit, Hunk};

/// Commits read per `git log` process and per transaction.
const BATCH: usize = 1000;
/// Commits whose hunks are read per `git log -p` process and per transaction (D228).
const HUNK_BATCH: usize = 500;
/// Starts each commit of the `git log` output.
const COMMIT_START: u8 = 0x01;
/// Bytes read from git's output per step; the cancel flag is checked between steps.
const READ_CHUNK: usize = 64 * 1024;
/// The flags of the hunk read (D228). The prefixes are forced so that a `diff.noprefix`
/// setting cannot change the paths `parse_hunks` reads; `--root` shows a root commit's
/// diff whatever `log.showRoot` says.
const HUNK_FLAGS: [&str; 11] = [
    "--diff-merges=first-parent",
    "-p",
    "-U0",
    "-M",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--root",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--format=%x01%H",
];

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
    list(root, false, sha, since)
}

/// The commits of the first-parent history of `sha` with a committer time at or after
/// `since` (seconds), newest first (D228): the commits whose hunks are read.
pub fn first_parent_window(root: &Path, sha: &str, since: i64) -> Result<Vec<String>, String> {
    list(root, true, sha, since)
}

fn list(root: &Path, first_parent: bool, sha: &str, since: i64) -> Result<Vec<String>, String> {
    // `--max-age` takes the epoch seconds as they are. `--since` parses a date, and
    // git reads a small epoch (below about 1973) as a date it cannot parse, which
    // falls back to now and hides every commit.
    let max_age = format!("--max-age={since}");
    let mut args = vec!["rev-list"];
    if first_parent {
        args.push("--first-parent");
    }
    args.extend([max_age.as_str(), sha, "--"]);
    let output = git(root)
        .args(&args)
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
    let bytes = log_output(
        root,
        &[
            "-z",
            "--numstat",
            "-M",
            "--no-ext-diff",
            "--format=%x01%H%x00%P%x00%ct%x00%aE",
        ],
        shas,
        cancel,
    )?;
    parse_log(&bytes)
}

/// Reads the hunk headers of the first-parent commits `shas`, in that order, in one `git log`
/// process (D228). An empty list reads nothing and starts no git process.
pub fn read_hunks(
    root: &Path,
    shas: &[String],
    cancel: &AtomicBool,
) -> Result<Vec<FirstParentCommit>, String> {
    let bytes = log_output(root, &HUNK_FLAGS, shas, cancel)?;
    parse_hunks(&bytes)
}

/// Runs `git log --no-walk=unsorted --stdin` with `flags` over `shas` and returns its stdout.
/// An empty list runs nothing: git would otherwise read HEAD from an empty input.
/// `Err("cancelled")` when `cancel` is set while git runs; git is then killed.
fn log_output(
    root: &Path,
    flags: &[&str],
    shas: &[String],
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    if shas.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = git(root)
        .args(["log", "--no-walk=unsorted", "--stdin"])
        .args(flags)
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
    Ok(bytes)
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

/// Reads the commits of `root`'s first-parent window whose hunks the index lacks, newest first,
/// in batches, and inserts them (D228). `progress` is called after each batch. Cancelling
/// returns `Err("cancelled")`; what was inserted stays, and a later call resumes.
pub fn ingest_hunks(
    root: &Path,
    store: &mut HealthStore,
    sha: &str,
    since: i64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(HealthProgress),
) -> Result<HealthProgress, String> {
    let shas = first_parent_window(root, sha, since)?;
    let missing = store.missing_hunks(&shas)?;
    let total = shas.len();
    let already = total - missing.len();
    let mut done = 0;
    progress(report(total, already + done));
    for batch in missing.chunks(HUNK_BATCH) {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_owned());
        }
        let records = read_hunks(root, batch, cancel)?;
        store.insert_hunks(&records)?;
        done += batch.len();
        progress(report(total, already + done));
    }
    Ok(report(total, already + done))
}

/// Parses the output of `read_hunks`: `git log -p -U0 -M` with the `HUNK_FLAGS`. A commit
/// starts at its `\x01<sha>` line. Within a commit, a file block starts at `diff --git `. Its
/// paths come from the `---`/`+++` lines before its first `@@` (`/dev/null` for an added or
/// deleted side), or from `rename from`/`rename to` when there are none. Each `@@` line is a
/// hunk; every other line, body lines included, is skipped. A block with no hunk and no rename
/// (a mode change, a binary file) yields nothing; a pure rename yields one zero hunk.
pub fn parse_hunks(bytes: &[u8]) -> Result<Vec<FirstParentCommit>, String> {
    let mut commits: Vec<FirstParentCommit> = Vec::new();
    let mut file: Option<FileBlock> = None;
    for line in bytes.split(|&byte| byte == b'\n') {
        if let Some(sha) = line.strip_prefix(&[COMMIT_START]) {
            finish_file(file.take(), &mut commits)?;
            commits.push(FirstParentCommit {
                sha: text(sha).trim().to_owned(),
                hunks: Vec::new(),
            });
        } else if line.starts_with(b"diff --git ") {
            finish_file(file.take(), &mut commits)?;
            file = Some(FileBlock {
                in_header: true,
                ..FileBlock::default()
            });
        } else if let Some(block) = file.as_mut() {
            block.line(line)?;
        }
    }
    finish_file(file.take(), &mut commits)?;
    Ok(commits)
}

/// A file block of one commit's diff, as far as the replay needs it.
#[derive(Default)]
struct FileBlock {
    /// From `--- a/<old>`; `None` for `/dev/null`.
    old: Option<String>,
    /// From `+++ b/<new>`; `None` for `/dev/null`.
    new: Option<String>,
    rename_from: Option<String>,
    rename_to: Option<String>,
    /// True until the first `@@`: only the header lines before it are read.
    in_header: bool,
    /// `(old_start, old_count, new_start, new_count)` of each hunk.
    hunks: Vec<(u32, u32, u32, u32)>,
    /// The diff says `Binary files ... differ`: the file was binary on one side.
    binary: bool,
}

impl FileBlock {
    fn line(&mut self, line: &[u8]) -> Result<(), String> {
        if line.starts_with(b"@@ ") {
            self.in_header = false;
            self.hunks.push(parse_hunk_header(line)?);
        } else if self.in_header {
            // Only the header lines before the first hunk are read: a body line that
            // starts with `---` or `+++` is content.
            if let Some(rest) = line.strip_prefix(b"--- ") {
                self.old = side(rest, "a/");
            } else if let Some(rest) = line.strip_prefix(b"+++ ") {
                self.new = side(rest, "b/");
            } else if let Some(rest) = line.strip_prefix(b"rename from ") {
                self.rename_from = Some(path(rest));
            } else if let Some(rest) = line.strip_prefix(b"rename to ") {
                self.rename_to = Some(path(rest));
            } else if let Some(rest) = line.strip_prefix(b"Binary files ") {
                // A binary diff has no `---`/`+++` lines: its paths are on this line.
                self.binary = true;
                let (old, new) = binary_sides(rest);
                self.old = self.old.take().or(old);
                self.new = self.new.take().or(new);
            }
        }
        Ok(())
    }
}

/// Adds the hunks of a finished file block to the last commit of `commits`.
fn finish_file(block: Option<FileBlock>, commits: &mut [FirstParentCommit]) -> Result<(), String> {
    let Some(block) = block else {
        return Ok(());
    };
    if block.hunks.is_empty() && block.rename_from.is_none() && !block.binary {
        return Ok(());
    }
    let old = block.rename_from.clone().or(block.old.clone());
    let new = block.rename_to.clone().or(block.new.clone());
    let Some(path) = new.clone().or_else(|| old.clone()) else {
        if block.hunks.is_empty() {
            return Ok(());
        }
        return Err("git log output: a hunk of a file without a path".to_owned());
    };
    let old_path = match (old, new) {
        (Some(old), Some(new)) if old != new => Some(old),
        _ => None,
    };
    let commit = commits
        .last_mut()
        .ok_or_else(|| "git log output: a diff before any commit".to_owned())?;
    if block.hunks.is_empty() {
        // A rename with no content change is one zero hunk that carries the old path. A binary
        // file on one side is a marker (all zeros, no old path) that forgets the file's lines.
        if old_path.is_some() {
            commit.hunks.push(zero_hunk(path.clone(), old_path.clone()));
        }
        if block.binary {
            commit.hunks.push(zero_hunk(path.clone(), None));
        }
    }
    commit.hunks.extend(block.hunks.into_iter().map(
        |(old_start, old_count, new_start, new_count)| Hunk {
            path: path.clone(),
            old_path: old_path.clone(),
            old_start,
            old_count,
            new_start,
            new_count,
        },
    ));
    Ok(())
}

fn zero_hunk(path: String, old_path: Option<String>) -> Hunk {
    Hunk {
        path,
        old_path,
        old_start: 0,
        old_count: 0,
        new_start: 0,
        new_count: 0,
    }
}

/// The sides of a `Binary files A and B differ` line (the text after `Binary files `): `a/…` and
/// `b/…` paths, or `/dev/null`. When both sides name one path, each is half of the text.
fn binary_sides(rest: &[u8]) -> (Option<String>, Option<String>) {
    let Some(inner) = rest.strip_suffix(b" differ") else {
        return (None, None);
    };
    if let Some(new) = inner.strip_prefix(b"/dev/null and ") {
        return (None, side(new, "b/"));
    }
    if let Some(old) = inner.strip_suffix(b" and /dev/null") {
        return (side(old, "a/"), None);
    }
    let len = inner.len();
    if len < 5 || (len - 5) % 2 != 0 {
        return (None, None);
    }
    let half = (len - 5) / 2;
    if &inner[half..half + 5] != b" and " {
        return (None, None);
    }
    (side(&inner[..half], "a/"), side(&inner[half + 5..], "b/"))
}

/// `@@ -a[,b] +c[,d] @@ ...`: a missing count is 1.
fn parse_hunk_header(line: &[u8]) -> Result<(u32, u32, u32, u32), String> {
    let text = String::from_utf8_lossy(line);
    let mut parts = text.split_whitespace().skip(1);
    let old = parts.next().and_then(|part| part.strip_prefix('-'));
    let new = parts.next().and_then(|part| part.strip_prefix('+'));
    let range = |spec: Option<&str>| {
        let spec = spec?;
        let (start, count) = spec.split_once(',').unwrap_or((spec, "1"));
        Some((start.parse::<u32>().ok()?, count.parse::<u32>().ok()?))
    };
    match (range(old), range(new)) {
        (Some((old_start, old_count)), Some((new_start, new_count))) => {
            Ok((old_start, old_count, new_start, new_count))
        }
        _ => Err(format!("git log output: bad hunk header {text:?}")),
    }
}

/// The path of a `---`/`+++` line without its side prefix; `None` for `/dev/null`.
fn side(raw: &[u8], prefix: &str) -> Option<String> {
    let path = path(raw);
    if path == "/dev/null" {
        return None;
    }
    Some(path.strip_prefix(prefix).unwrap_or(&path).to_owned())
}

/// A path as git writes it: C-quoted when it has special characters, and followed by a tab
/// on `---`/`+++` lines when it has a space.
fn path(raw: &[u8]) -> String {
    let raw = raw.strip_suffix(b"\t").unwrap_or(raw);
    String::from_utf8_lossy(&unquote(raw)).into_owned()
}

/// Undoes git's C quoting of a path: `"..."` with `\a \b \t \n \v \f \r \" \\`, and `\NNN`
/// octal bytes. A path that is not quoted is returned as it is.
fn unquote(raw: &[u8]) -> Vec<u8> {
    let Some(inner) = raw
        .strip_prefix(b"\"")
        .and_then(|rest| rest.strip_suffix(b"\""))
    else {
        return raw.to_vec();
    };
    let mut out = Vec::with_capacity(inner.len());
    let mut at = 0;
    while at < inner.len() {
        let byte = inner[at];
        at += 1;
        if byte != b'\\' || at >= inner.len() {
            out.push(byte);
            continue;
        }
        let escape = inner[at];
        at += 1;
        match escape {
            b'a' => out.push(7),
            b'b' => out.push(8),
            b't' => out.push(b'\t'),
            b'n' => out.push(b'\n'),
            b'v' => out.push(11),
            b'f' => out.push(12),
            b'r' => out.push(b'\r'),
            b'0'..=b'7' => {
                let mut value = u32::from(escape - b'0');
                let mut digits = 1;
                while digits < 3 && at < inner.len() && (b'0'..=b'7').contains(&inner[at]) {
                    value = value * 8 + u32::from(inner[at] - b'0');
                    at += 1;
                    digits += 1;
                }
                out.push(u8::try_from(value).unwrap_or(b'?'));
            }
            other => out.push(other),
        }
    }
    out
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

    fn parsed(text: &str) -> Vec<FirstParentCommit> {
        parse_hunks(text.as_bytes()).unwrap()
    }

    fn span(path: &str, old: (u32, u32), new: (u32, u32)) -> Hunk {
        Hunk {
            path: path.to_owned(),
            old_path: None,
            old_start: old.0,
            old_count: old.1,
            new_start: new.0,
            new_count: new.1,
        }
    }

    #[test]
    fn a_modified_file_gives_one_hunk_per_header_with_missing_counts_as_one() {
        let commits = parsed(
            "\x01aa\n\
             diff --git a/f.txt b/f.txt\n\
             index 1..2 100644\n\
             --- a/f.txt\n\
             +++ b/f.txt\n\
             @@ -2,3 +2,4 @@ fn ctx()\n\
             -x\n\
             +y\n\
             @@ -9 +9,0 @@\n\
             -gone\n",
        );
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].sha, "aa");
        assert_eq!(
            commits[0].hunks,
            vec![span("f.txt", (2, 3), (2, 4)), span("f.txt", (9, 1), (9, 0))]
        );
    }

    #[test]
    fn an_added_and_a_deleted_file_take_the_path_of_the_side_that_exists() {
        let commits = parsed(
            "\x01aa\n\
             diff --git a/new.txt b/new.txt\n\
             --- /dev/null\n\
             +++ b/new.txt\n\
             @@ -0,0 +1,2 @@\n\
             +a\n\
             +b\n\
             diff --git a/gone.txt b/gone.txt\n\
             --- a/gone.txt\n\
             +++ /dev/null\n\
             @@ -1,2 +0,0 @@\n\
             -a\n\
             -b\n",
        );
        assert_eq!(
            commits[0].hunks,
            vec![
                span("new.txt", (0, 0), (1, 2)),
                span("gone.txt", (1, 2), (0, 0)),
            ]
        );
    }

    #[test]
    fn a_rename_without_content_change_is_one_zero_hunk_with_the_old_path() {
        let commits = parsed(
            "\x01aa\n\
             diff --git a/old.txt b/new.txt\n\
             similarity index 100%\n\
             rename from old.txt\n\
             rename to new.txt\n",
        );
        assert_eq!(
            commits[0].hunks,
            vec![Hunk {
                path: "new.txt".to_owned(),
                old_path: Some("old.txt".to_owned()),
                old_start: 0,
                old_count: 0,
                new_start: 0,
                new_count: 0,
            }]
        );
    }

    #[test]
    fn a_rename_with_a_change_carries_the_old_path_on_its_hunks() {
        let commits = parsed(
            "\x01aa\n\
             diff --git a/a.txt b/b.txt\n\
             similarity index 80%\n\
             rename from a.txt\n\
             rename to b.txt\n\
             --- a/a.txt\n\
             +++ b/b.txt\n\
             @@ -4 +4 @@\n\
             -x\n\
             +y\n",
        );
        assert_eq!(
            commits[0].hunks,
            vec![Hunk {
                path: "b.txt".to_owned(),
                old_path: Some("a.txt".to_owned()),
                old_start: 4,
                old_count: 1,
                new_start: 4,
                new_count: 1,
            }]
        );
    }

    #[test]
    fn a_binary_file_gives_a_marker_and_a_mode_change_gives_nothing() {
        let commits = parsed(
            "\x01aa\n\
             diff --git a/logo.png b/logo.png\n\
             index 1..2 100644\n\
             Binary files a/logo.png and b/logo.png differ\n\
             diff --git a/run.sh b/run.sh\n\
             old mode 100644\n\
             new mode 100755\n",
        );
        assert_eq!(commits.len(), 1);
        assert_eq!(
            commits[0].hunks,
            vec![zero_hunk("logo.png".to_owned(), None)]
        );
    }

    #[test]
    fn a_binary_rename_gives_the_rename_then_a_marker_on_the_new_path() {
        let commits = parsed(
            "\x01aa\n\
             diff --git a/old.png b/new.png\n\
             similarity index 40%\n\
             rename from old.png\n\
             rename to new.png\n\
             index 1..2 100644\n\
             Binary files a/old.png and b/new.png differ\n",
        );
        assert_eq!(
            commits[0].hunks,
            vec![
                zero_hunk("new.png".to_owned(), Some("old.png".to_owned())),
                zero_hunk("new.png".to_owned(), None),
            ]
        );
    }

    #[test]
    fn a_merge_keeps_the_hunks_against_its_first_parent_and_each_commit_its_own() {
        let commits = parsed(
            "\x01mm\n\
             diff --git a/side.txt b/side.txt\n\
             --- /dev/null\n\
             +++ b/side.txt\n\
             @@ -0,0 +1,4 @@\n\
             +a\n\
             \x01c1\n\
             diff --git a/root.txt b/root.txt\n\
             --- /dev/null\n\
             +++ b/root.txt\n\
             @@ -0,0 +1 @@\n\
             +r\n",
        );
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "mm");
        assert_eq!(commits[0].hunks, vec![span("side.txt", (0, 0), (1, 4))]);
        assert_eq!(commits[1].sha, "c1");
        assert_eq!(commits[1].hunks, vec![span("root.txt", (0, 0), (1, 1))]);
    }

    #[test]
    fn quoted_paths_are_unquoted_with_their_c_escapes() {
        let commits = parsed(
            "\x01aa\n\
             diff --git \"a/x\\ty\" \"b/x\\ty\"\n\
             --- \"a/x\\ty\"\n\
             +++ \"b/x\\ty\"\n\
             @@ -1 +1 @@\n\
             -a\n\
             +b\n\
             diff --git \"a/\\303\\274 \\\"q\\\".txt\" \"b/\\303\\274 \\\"q\\\".txt\"\n\
             --- \"a/\\303\\274 \\\"q\\\".txt\"\n\
             +++ \"b/\\303\\274 \\\"q\\\".txt\"\n\
             @@ -0,0 +1 @@\n\
             +c\n\
             diff --git a/with space.txt b/with space.txt\n\
             --- a/with space.txt\t\n\
             +++ b/with space.txt\t\n\
             @@ -2 +2 @@\n\
             -a\n\
             +b\n",
        );
        let paths: Vec<&str> = commits[0].hunks.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, ["x\ty", "\u{fc} \"q\".txt", "with space.txt"]);
    }

    #[test]
    fn a_body_line_that_looks_like_a_header_is_content() {
        // The second hunk body has a removed `--- old` and an added `+++ new`: they must not
        // change the paths of the file.
        let commits = parsed(
            "\x01aa\n\
             diff --git a/f.txt b/f.txt\n\
             --- a/f.txt\n\
             +++ b/f.txt\n\
             @@ -1,2 +1,2 @@\n\
             --- old\n\
             +++ new\n\
             @@ -5 +5 @@\n\
             -x\n\
             +++ y\n",
        );
        assert_eq!(
            commits[0].hunks,
            vec![span("f.txt", (1, 2), (1, 2)), span("f.txt", (5, 1), (5, 1))]
        );
    }

    #[test]
    fn a_malformed_hunk_header_is_an_error() {
        assert!(parse_hunks(b"\x01aa\ndiff --git a/f b/f\n@@ -x +1 @@\n").is_err());
        assert!(parse_hunks(b"diff --git a/f b/f\n@@ -1 +1 @@\n").is_err());
    }

    #[test]
    fn empty_hunk_output_is_no_commits_and_empty_list_reads_nothing() {
        assert!(parse_hunks(b"").unwrap().is_empty());
        let cancel = AtomicBool::new(false);
        let commits = read_hunks(Path::new("/nonexistent"), &[], &cancel).unwrap();
        assert!(commits.is_empty());
    }

    #[test]
    fn a_binary_add_and_delete_give_a_marker_on_the_side_that_exists() {
        let commits = parsed(
            "\x01aa\n\
             diff --git a/new one.png b/new one.png\n\
             new file mode 100644\n\
             Binary files /dev/null and b/new one.png differ\n\
             diff --git a/gone.png b/gone.png\n\
             deleted file mode 100644\n\
             Binary files a/gone.png and /dev/null differ\n",
        );
        assert_eq!(
            commits[0].hunks,
            vec![
                zero_hunk("new one.png".to_owned(), None),
                zero_hunk("gone.png".to_owned(), None),
            ]
        );
    }
}

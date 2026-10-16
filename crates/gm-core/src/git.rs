//! `GitPort` over the `git` executable. Status uses porcelain v2 with `-z`:
//! stable, machine-readable, and it honors fsmonitor and the untracked cache.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use git2::{ErrorCode, IndexEntry, IndexTime, ObjectType, Oid, Repository};
use gm_schema::{
    BranchInfo, Change, DiffSide, FileDiff, Hunk, HunkId, RepoStatus, StageTarget, StatusEntry,
};

use crate::diff::parse_unified;
use crate::stage_math;
use crate::{GitError, GitPort};

/// `login_path` is the user's login-shell PATH, filled in once from a
/// background thread. Until it is set, git runs with the process's own PATH.
/// Passing it per command avoids writing the process environment while other
/// threads run.
#[derive(Clone, Default)]
pub struct SubprocessGit {
    pub login_path: Arc<OnceLock<String>>,
}

impl SubprocessGit {
    fn git(&self, dir: &Path) -> Command {
        let mut command = Command::new("git");
        if let Some(path) = self.login_path.get() {
            command.env("PATH", path);
        }
        // --no-optional-locks: never take index.lock against an agent running git.
        command
            .arg("--no-optional-locks")
            .current_dir(dir)
            .env("LC_ALL", "C");
        command
    }

    fn run(command: Command) -> Result<Vec<u8>, GitError> {
        Self::run_allowing(command, &[0])
    }

    /// Like `run`, but the exit codes in `ok_codes` count as success.
    fn run_allowing(mut command: Command, ok_codes: &[i32]) -> Result<Vec<u8>, GitError> {
        let output = command
            .output()
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        if output.status.code().is_some_and(|c| ok_codes.contains(&c)) {
            Ok(output.stdout)
        } else {
            Err(GitError(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ))
        }
    }
}

impl GitPort for SubprocessGit {
    fn resolve_root(&self, path: &Path) -> Result<PathBuf, GitError> {
        let mut command = self.git(path);
        command.args(["rev-parse", "--show-toplevel"]);
        let out = Self::run(command)?;
        Ok(PathBuf::from(
            String::from_utf8_lossy(&out).trim_end_matches('\n'),
        ))
    }

    fn status(&self, root: &Path) -> Result<RepoStatus, GitError> {
        let mut command = self.git(root);
        command.args([
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=normal",
        ]);
        let out = Self::run(command)?;
        let mut status = parse_status(&out)?;
        status.root = root.to_string_lossy().into_owned();
        Ok(status)
    }

    fn diff(&self, root: &Path, path: &str, side: DiffSide) -> Result<FileDiff, GitError> {
        let untracked = side == DiffSide::Unstaged && !is_tracked(&open(root)?, path)?;
        let mut command = self.git(root);
        command.args([
            "-c",
            "core.quotepath=off",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--histogram",
            "-U3",
        ]);
        let raw = if untracked {
            // `--no-index` exits with 1 when the files differ, which is the normal case here.
            command.args(["--no-index", "--", "/dev/null", path]);
            Self::run_allowing(command, &[0, 1])?
        } else {
            match side {
                DiffSide::Staged => command.args(["--cached", "-M", "--", path]),
                DiffSide::Unstaged => command.args(["--", path]),
            };
            Self::run(command)?
        };
        let mut diff = parse_unified(path, &raw)?;
        diff.old_path = None;
        Ok(diff)
    }

    fn stage(&self, root: &Path, target: &StageTarget) -> Result<(), GitError> {
        let repo = open(root)?;
        check_index_lock(&repo)?;
        let mut index = repo.index().map_err(|e| git_err(&e))?;
        match target {
            StageTarget::File { path } => {
                stage_file(root, &mut index, path)?;
            }
            StageTarget::Hunk { path, hunk } => {
                let diff = self.diff(root, path, DiffSide::Unstaged)?;
                let hunk = find_hunk(&diff, hunk)?;
                let (mode, base) = index_base(&repo, &index, root, path)?;
                let next = stage_math::stage_whole_hunk(&base, hunk).map_err(GitError)?;
                store_staged(root, &repo, &mut index, path, mode, &next)?;
            }
            StageTarget::Lines { path, hunk, lines } => {
                let diff = self.diff(root, path, DiffSide::Unstaged)?;
                let hunk = find_hunk(&diff, hunk)?;
                let (mode, base) = index_base(&repo, &index, root, path)?;
                let next = stage_math::stage_selection(&base, hunk, lines).map_err(GitError)?;
                store_staged(root, &repo, &mut index, path, mode, &next)?;
            }
        }
        index.write().map_err(|e| git_err(&e))
    }

    fn unstage(&self, root: &Path, target: &StageTarget) -> Result<(), GitError> {
        let repo = open(root)?;
        check_index_lock(&repo)?;
        let mut index = repo.index().map_err(|e| git_err(&e))?;
        match target {
            StageTarget::File { path } => match head_entry(&repo, path)? {
                Some((mode, id, size)) => {
                    index
                        .add(&zeroed_entry(path, mode, id, size)?)
                        .map_err(|e| git_err(&e))?;
                }
                None => {
                    if index.get_path(Path::new(path), 0).is_some() {
                        index
                            .remove_path(Path::new(path))
                            .map_err(|e| git_err(&e))?;
                    }
                }
            },
            StageTarget::Hunk { path, hunk } => {
                let diff = self.diff(root, path, DiffSide::Staged)?;
                let hunk = find_hunk(&diff, hunk)?;
                let selected = change_indices(hunk);
                unstage_hunk(&repo, &mut index, path, hunk, &selected)?;
            }
            StageTarget::Lines { path, hunk, lines } => {
                let diff = self.diff(root, path, DiffSide::Staged)?;
                let hunk = find_hunk(&diff, hunk)?;
                unstage_hunk(&repo, &mut index, path, hunk, lines)?;
            }
        }
        index.write().map_err(|e| git_err(&e))
    }

    fn commit(&self, root: &Path, message: &str) -> Result<String, GitError> {
        let repo = open(root)?;
        check_index_lock(&repo)?;
        if message.trim().is_empty() {
            return Err(GitError("commit message is empty".into()));
        }
        let signature = repo
            .signature()
            .map_err(|_| GitError("set user.name and user.email before committing".into()))?;
        let mut index = repo.index().map_err(|e| git_err(&e))?;
        let tree_id = index.write_tree_to(&repo).map_err(|e| git_err(&e))?;
        let tree = repo.find_tree(tree_id).map_err(|e| git_err(&e))?;
        let parent = match repo.head() {
            Ok(head) => Some(head.peel_to_commit().map_err(|e| git_err(&e))?),
            Err(e) if e.code() == ErrorCode::UnbornBranch => None,
            Err(e) => return Err(git_err(&e)),
        };
        let parent_tree = match &parent {
            Some(commit) => commit.tree_id(),
            None => repo
                .treebuilder(None)
                .and_then(|builder| builder.write())
                .map_err(|e| git_err(&e))?,
        };
        if tree_id == parent_tree {
            return Err(GitError("nothing staged to commit".into()));
        }
        let mut text = message.to_owned();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
        let id = repo
            .commit(Some("HEAD"), &signature, &signature, &text, &tree, &parents)
            .map_err(|e| git_err(&e))?;
        Ok(id.to_string())
    }
}

fn git_err(error: &git2::Error) -> GitError {
    GitError(error.message().to_owned())
}

fn open(root: &Path) -> Result<Repository, GitError> {
    Repository::open(root).map_err(|e| git_err(&e))
}

/// git2 writes `index.lock` itself. An existing lock means another git process
/// is writing the index, so fail now rather than wait or race it.
fn check_index_lock(repo: &Repository) -> Result<(), GitError> {
    let lock = repo.path().join("index.lock");
    if lock.exists() {
        return Err(GitError(format!(
            "index is locked ({}); another git process may be running",
            lock.display()
        )));
    }
    Ok(())
}

fn is_tracked(repo: &Repository, path: &str) -> Result<bool, GitError> {
    let index = repo.index().map_err(|e| git_err(&e))?;
    Ok(index.get_path(Path::new(path), 0).is_some())
}

fn find_hunk<'a>(diff: &'a FileDiff, id: &HunkId) -> Result<&'a Hunk, GitError> {
    diff.hunks
        .iter()
        .find(|hunk| hunk.id == *id)
        .ok_or_else(|| GitError(format!("hunk not found or changed: {}", id.0)))
}

/// Indices of every added and removed line of `hunk`.
fn change_indices(hunk: &Hunk) -> Vec<u32> {
    hunk.lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.kind != gm_schema::LineKind::Context)
        .filter_map(|(i, _)| u32::try_from(i).ok())
        .collect()
}

/// An index entry with zeroed stat data. Git then re-reads the working tree
/// file to decide whether it is modified, so a partial stage never looks clean.
fn zeroed_entry(path: &str, mode: u32, id: Oid, size: usize) -> Result<IndexEntry, GitError> {
    Ok(IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: u32::try_from(size)
            .map_err(|_| GitError(format!("file too large for the index: {path}")))?,
        id,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    })
}

/// Mode and content of the index entry for `path`. `None` when the path is not
/// in the index. A conflicted path is an error.
fn index_content(
    repo: &Repository,
    index: &git2::Index,
    path: &str,
) -> Result<Option<(u32, Vec<u8>)>, GitError> {
    let p = Path::new(path);
    if (1..=3).any(|stage| index.get_path(p, stage).is_some()) {
        return Err(GitError(format!("path has unresolved conflicts: {path}")));
    }
    let Some(entry) = index.get_path(p, 0) else {
        return Ok(None);
    };
    let blob = repo.find_blob(entry.id).map_err(|e| git_err(&e))?;
    Ok(Some((entry.mode, blob.content().to_vec())))
}

/// The base a stage action works on: the index content, or for a path not in
/// the index, empty content with the working tree file's mode.
fn index_base(
    repo: &Repository,
    index: &git2::Index,
    root: &Path,
    path: &str,
) -> Result<(u32, Vec<u8>), GitError> {
    match index_content(repo, index, path)? {
        Some(found) => Ok(found),
        None => Ok((worktree_mode(root, path)?, Vec::new())),
    }
}

/// Stores the staged content of a file. Empty content of a file that is gone
/// from the working tree means the deletion is staged, so the entry goes.
fn store_staged(
    root: &Path,
    repo: &Repository,
    index: &mut git2::Index,
    path: &str,
    mode: u32,
    content: &[u8],
) -> Result<(), GitError> {
    if content.is_empty() && root.join(path).symlink_metadata().is_err() {
        if index.get_path(Path::new(path), 0).is_some() {
            index
                .remove_path(Path::new(path))
                .map_err(|e| git_err(&e))?;
        }
        return Ok(());
    }
    write_index_content(repo, index, path, mode, content)
}

fn write_index_content(
    repo: &Repository,
    index: &mut git2::Index,
    path: &str,
    mode: u32,
    content: &[u8],
) -> Result<(), GitError> {
    let id = repo.blob(content).map_err(|e| git_err(&e))?;
    index
        .add(&zeroed_entry(path, mode, id, content.len())?)
        .map_err(|e| git_err(&e))
}

/// Stages the whole working tree state of `path`, or its deletion.
fn stage_file(root: &Path, index: &mut git2::Index, path: &str) -> Result<(), GitError> {
    let p = Path::new(path);
    if root.join(p).symlink_metadata().is_ok() {
        index.add_path(p).map_err(|e| git_err(&e))
    } else if index.get_path(p, 0).is_some() {
        index.remove_path(p).map_err(|e| git_err(&e))
    } else {
        Err(GitError(format!("pathspec did not match any file: {path}")))
    }
}

/// Removes the `selected` changes of a staged `hunk` from the index entry.
/// A path that is not in HEAD and ends up empty leaves the index entirely.
fn unstage_hunk(
    repo: &Repository,
    index: &mut git2::Index,
    path: &str,
    hunk: &Hunk,
    selected: &[u32],
) -> Result<(), GitError> {
    let (mode, current) = index_content(repo, index, path)?
        .ok_or_else(|| GitError(format!("{path} is not in the index")))?;
    let next = stage_math::unstage_selection(&current, hunk, selected).map_err(GitError)?;
    let in_head = head_entry(repo, path)?.is_some();
    if next.is_empty() && !in_head {
        index.remove_path(Path::new(path)).map_err(|e| git_err(&e))
    } else {
        write_index_content(repo, index, path, mode, &next)
    }
}

/// Mode, blob id and size of `path` in HEAD. `None` when HEAD is unborn or the
/// path is not a file in HEAD.
fn head_entry(repo: &Repository, path: &str) -> Result<Option<(u32, Oid, usize)>, GitError> {
    let head = match repo.head() {
        Ok(head) => head,
        Err(e) if e.code() == ErrorCode::UnbornBranch => return Ok(None),
        Err(e) => return Err(git_err(&e)),
    };
    let tree = head.peel_to_tree().map_err(|e| git_err(&e))?;
    let Ok(entry) = tree.get_path(Path::new(path)) else {
        return Ok(None);
    };
    if entry.kind() != Some(ObjectType::Blob) {
        return Ok(None);
    }
    let size = repo.find_blob(entry.id()).map_err(|e| git_err(&e))?.size();
    let mode = u32::try_from(entry.filemode())
        .map_err(|_| GitError(format!("unexpected file mode in HEAD: {path}")))?;
    Ok(Some((mode, entry.id(), size)))
}

/// Mode of a working tree file, as git would record it when adding it.
fn worktree_mode(root: &Path, path: &str) -> Result<u32, GitError> {
    let meta = root
        .join(path)
        .symlink_metadata()
        .map_err(|e| GitError(format!("cannot read {path}: {e}")))?;
    if meta.file_type().is_symlink() {
        return Ok(0o120_000);
    }
    Ok(file_mode(&meta))
}

#[cfg(unix)]
fn file_mode(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if meta.permissions().mode() & 0o111 != 0 {
        0o100_755
    } else {
        0o100_644
    }
}

#[cfg(not(unix))]
fn file_mode(_meta: &std::fs::Metadata) -> u32 {
    0o100_644
}

fn change(code: u8) -> Option<Change> {
    match code {
        b'A' => Some(Change::Added),
        b'M' => Some(Change::Modified),
        b'D' => Some(Change::Deleted),
        b'R' => Some(Change::Renamed),
        b'C' => Some(Change::Copied),
        b'T' => Some(Change::TypeChanged),
        _ => None,
    }
}

/// Parses `git status --porcelain=v2 -z --branch`. `root` is left empty.
pub fn parse_status(raw: &[u8]) -> Result<RepoStatus, GitError> {
    let mut status = RepoStatus::default();
    let mut records = raw.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        let line = String::from_utf8_lossy(record);
        let bad = || GitError(format!("unexpected status record: {line}"));
        if let Some(header) = line.strip_prefix("# ") {
            parse_header(header, &mut status.branch);
        } else if let Some(path) = line.strip_prefix("? ") {
            status.entries.push(StatusEntry {
                path: path.to_owned(),
                orig_path: None,
                staged: None,
                unstaged: None,
                untracked: true,
                conflicted: false,
            });
        } else if line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("u ") {
            // Fixed fields before the path: 1 has 8, 2 has 9, u has 10.
            let kind = line.as_bytes()[0];
            let fields = match kind {
                b'1' => 8,
                b'2' => 9,
                _ => 10,
            };
            let mut parts = line.splitn(fields + 1, ' ');
            let xy = parts.nth(1).ok_or_else(bad)?.as_bytes();
            let path = parts.nth(fields - 2).ok_or_else(bad)?.to_owned();
            if xy.len() != 2 {
                return Err(bad());
            }
            // A rename's source path is the next NUL-separated record.
            let orig_path = if kind == b'2' {
                Some(String::from_utf8_lossy(records.next().ok_or_else(bad)?).into_owned())
            } else {
                None
            };
            let conflicted = kind == b'u';
            status.entries.push(StatusEntry {
                path,
                orig_path,
                staged: if conflicted { None } else { change(xy[0]) },
                unstaged: if conflicted { None } else { change(xy[1]) },
                untracked: false,
                conflicted,
            });
        }
        // "! " (ignored) records are not requested; anything else is skipped.
    }
    Ok(status)
}

fn parse_header(header: &str, branch: &mut BranchInfo) {
    if let Some(oid) = header.strip_prefix("branch.oid ") {
        branch.oid = (oid != "(initial)").then(|| oid.to_owned());
    } else if let Some(head) = header.strip_prefix("branch.head ") {
        branch.head = (head != "(detached)").then(|| head.to_owned());
    } else if let Some(upstream) = header.strip_prefix("branch.upstream ") {
        branch.upstream = Some(upstream.to_owned());
    } else if let Some(ab) = header.strip_prefix("branch.ab ") {
        let mut it = ab.split(' ');
        branch.ahead = it
            .next()
            .and_then(|a| a.trim_start_matches('+').parse().ok())
            .unwrap_or(0);
        branch.behind = it
            .next()
            .and_then(|b| b.trim_start_matches('-').parse().ok())
            .unwrap_or(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_record_kind() {
        let raw = b"# branch.oid abc123\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa bbb src/a.rs\0\
1 A. N... 000000 100644 100644 000 bbb new file.txt\0\
2 R. N... 100644 100644 100644 aaa bbb R100 to.rs\0from.rs\0\
u UU N... 100644 100644 100644 100644 a b c both.rs\0\
? untracked.txt\0";
        let status = parse_status(raw).unwrap();
        assert_eq!(status.branch.head.as_deref(), Some("main"));
        assert_eq!((status.branch.ahead, status.branch.behind), (2, 1));
        assert_eq!(status.entries.len(), 5);
        assert_eq!(status.entries[0].unstaged, Some(Change::Modified));
        assert_eq!(status.entries[0].staged, None);
        assert_eq!(status.entries[1].path, "new file.txt");
        assert_eq!(status.entries[1].staged, Some(Change::Added));
        assert_eq!(status.entries[2].path, "to.rs");
        assert_eq!(status.entries[2].orig_path.as_deref(), Some("from.rs"));
        assert!(status.entries[3].conflicted);
        assert!(status.entries[4].untracked);
    }

    #[test]
    fn detached_and_unborn_heads_have_no_name_or_oid() {
        let status = parse_status(b"# branch.oid (initial)\0# branch.head (detached)\0").unwrap();
        assert_eq!(status.branch.head, None);
        assert_eq!(status.branch.oid, None);
    }

    #[test]
    fn a_truncated_record_is_an_error_not_a_panic() {
        assert!(parse_status(b"1 .M N...\0").is_err());
    }
}

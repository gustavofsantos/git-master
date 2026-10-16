//! `GitPort` over the `git` executable. Status uses porcelain v2 with `-z`:
//! stable, machine-readable, and it honors fsmonitor and the untracked cache.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use gm_schema::{BranchInfo, Change, RepoStatus, StatusEntry};

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

    fn run(mut command: Command) -> Result<Vec<u8>, GitError> {
        let output = command
            .output()
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        if output.status.success() {
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

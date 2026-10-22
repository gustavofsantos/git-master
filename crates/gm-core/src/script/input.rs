//! The `input.repo` value of a run (docs/reference/workflow-api.md, D105): a read-only snapshot of
//! the repository when the run starts. The run, the dry run and the tests build it the same way.

use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};

use crate::GitPort;

/// `staged_diff` is the diff text up to this many bytes. A longer diff is cut at a line break
/// and ends with `[diff truncated at 1 MiB]`.
pub const STAGED_DIFF_INLINE: usize = 1024 * 1024;

const TRUNCATED: &str = "[diff truncated at 1 MiB]";

/// The snapshot, and the temporary file that holds the whole diff when it was cut. Dropping this
/// deletes the file, so keep it until the run ends.
pub struct RepoInput {
    /// `{ root, branch, head, staged_diff }`, plus `staged_diff_path` (the whole diff, in a
    /// temporary file) when `staged_diff` was cut. A piece that cannot be read is `null`.
    pub value: Value,
    pub diff_file: Option<tempfile::NamedTempFile>,
}

/// Reads the snapshot of the repository at `root` through `git`.
#[must_use]
pub fn repo_input(git: &dyn GitPort, root: &Path) -> RepoInput {
    let (branch, head) = match git.status(root) {
        Ok(status) => (status.branch.head, status.branch.oid),
        Err(_) => (None, None),
    };
    let mut value = json!({
        "root": root.to_string_lossy(),
        "branch": branch,
        "head": head,
        "staged_diff": Value::Null,
    });
    let mut diff_file = None;
    if let Ok(diff) = git.staged_diff(root) {
        if diff.len() > STAGED_DIFF_INLINE {
            value["staged_diff"] = json!(truncated(&diff));
            // When the temporary file cannot be written, the inline text is still truncated and
            // no path is given.
            if let Ok(file) = spill(&diff) {
                value["staged_diff_path"] = json!(file.path().to_string_lossy());
                diff_file = Some(file);
            }
        } else {
            value["staged_diff"] = json!(diff);
        }
    }
    RepoInput { value, diff_file }
}

/// `value` for a directory that is not a repository: every piece `null`.
#[must_use]
pub fn no_repo() -> Value {
    json!({"root": null, "branch": null, "head": null, "staged_diff": null})
}

/// The inline text of a diff longer than the limit: cut after the last line break at or before
/// the limit (at the limit itself when there is none), then the marker line.
fn truncated(diff: &str) -> String {
    let mut end = STAGED_DIFF_INLINE;
    while !diff.is_char_boundary(end) {
        end -= 1;
    }
    let kept = diff[..end].rfind('\n').map_or(end, |i| i + 1);
    let mut text = diff[..kept].to_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(TRUNCATED);
    text
}

/// Writes the whole diff to a temporary file that lives as long as the returned handle.
fn spill(diff: &str) -> std::io::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(diff.as_bytes())?;
    file.flush()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;
    use crate::GitError;
    use crate::git::SubprocessGit;

    /// A repository whose status and staged diff are given, for the piece-by-piece cases.
    struct FakeRepo {
        head: Option<String>,
        oid: Option<String>,
        status_fails: bool,
        diff: Result<String, GitError>,
    }

    impl GitPort for FakeRepo {
        fn resolve_root(&self, path: &Path) -> Result<PathBuf, GitError> {
            Ok(path.to_path_buf())
        }

        fn status(&self, _root: &Path) -> Result<gm_schema::RepoStatus, GitError> {
            if self.status_fails {
                return Err(GitError("no status".into()));
            }
            Ok(gm_schema::RepoStatus {
                branch: gm_schema::BranchInfo {
                    head: self.head.clone(),
                    oid: self.oid.clone(),
                    ..Default::default()
                },
                ..Default::default()
            })
        }

        fn staged_diff(&self, _root: &Path) -> Result<String, GitError> {
            self.diff.clone()
        }
    }

    fn fake(diff: Result<String, GitError>) -> FakeRepo {
        FakeRepo {
            head: Some("main".into()),
            oid: Some("abc123".into()),
            status_fails: false,
            diff,
        }
    }

    #[test]
    fn branch_head_and_staged_diff_come_from_git() {
        let input = repo_input(&fake(Ok("+x\n".into())), Path::new("/repo"));
        assert_eq!(input.value["root"], "/repo");
        assert_eq!(input.value["branch"], "main");
        assert_eq!(input.value["head"], "abc123");
        assert_eq!(input.value["staged_diff"], "+x\n");
        assert!(input.value.get("staged_diff_path").is_none());
        assert!(input.diff_file.is_none());
    }

    #[test]
    fn a_piece_that_cannot_be_read_is_null() {
        let repo = FakeRepo {
            status_fails: true,
            ..fake(Err(GitError("no diff".into())))
        };
        let input = repo_input(&repo, Path::new("/repo"));
        assert_eq!(input.value["root"], "/repo");
        assert_eq!(input.value["branch"], Value::Null);
        assert_eq!(input.value["head"], Value::Null);
        assert_eq!(input.value["staged_diff"], Value::Null);
    }

    #[test]
    fn a_long_diff_is_cut_at_a_line_break_and_written_whole_to_a_file() {
        let line = "+0123456789 abcdefghijklmnopqrstuvwxyz\n";
        let diff = line.repeat(STAGED_DIFF_INLINE / line.len() + 10);
        let input = repo_input(&fake(Ok(diff.clone())), Path::new("/repo"));
        let inline = input.value["staged_diff"].as_str().unwrap();
        let body = inline.strip_suffix(TRUNCATED).expect("marker line");
        assert!(body.ends_with('\n'), "cut at a line break");
        assert!(body.len() <= STAGED_DIFF_INLINE);
        let file = input.diff_file.as_ref().expect("the whole diff is kept");
        assert_eq!(std::fs::read_to_string(file.path()).unwrap(), diff);
        assert_eq!(
            input.value["staged_diff_path"],
            file.path().to_string_lossy().as_ref()
        );
    }

    #[test]
    fn a_cut_never_splits_a_character() {
        // A two-byte character straddles the limit.
        let mut diff = "a".repeat(STAGED_DIFF_INLINE - 1);
        diff.push('é');
        diff.push_str("\nrest");
        let input = repo_input(&fake(Ok(diff)), Path::new("/repo"));
        let inline = input.value["staged_diff"].as_str().unwrap();
        assert!(inline.ends_with(TRUNCATED));
    }

    #[test]
    fn no_repo_has_every_piece_null() {
        let value = no_repo();
        for key in ["root", "branch", "head", "staged_diff"] {
            assert_eq!(value[key], Value::Null, "{key}");
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
            ])
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn a_real_repository_gives_its_branch_head_and_staged_diff() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        git(root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(root, &["add", "a.txt"]);
        git(root, &["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        git(root, &["add", "a.txt"]);

        let input = repo_input(&SubprocessGit::default(), root);
        let head = git(root, &["rev-parse", "HEAD"]);
        assert_eq!(input.value["branch"], "main");
        assert_eq!(input.value["head"], head.as_str());
        assert!(
            input.value["staged_diff"]
                .as_str()
                .unwrap()
                .contains("+two")
        );
    }
}

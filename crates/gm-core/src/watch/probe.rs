//! The git reads the watcher needs. They run on the watcher thread, never on
//! the UI thread, and never write: `--no-optional-locks`, and no index refresh.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};

pub struct GitDirs {
    /// This worktree's git dir (`.git`, or `.git/worktrees/<name>` for a linked one).
    pub git_dir: PathBuf,
    /// Where refs and packed refs live; equal to `git_dir` in a plain repository.
    pub common_dir: PathBuf,
}

fn git(root: &Path, login_path: &OnceLock<String>) -> Command {
    let mut command = Command::new("git");
    if let Some(path) = login_path.get() {
        command.env("PATH", path);
    }
    command
        .arg("--no-optional-locks")
        .current_dir(root)
        .env("LC_ALL", "C");
    command
}

fn output(mut command: Command, ok_codes: &[i32]) -> Result<Vec<u8>, String> {
    let output = command
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    match output.status.code() {
        Some(code) if ok_codes.contains(&code) => Ok(output.stdout),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

pub fn git_dirs(root: &Path, login_path: &Arc<OnceLock<String>>) -> Result<GitDirs, String> {
    let mut command = git(root, login_path);
    command.args([
        "rev-parse",
        "--path-format=absolute",
        "--git-dir",
        "--git-common-dir",
    ]);
    let out = output(command, &[0])?;
    let text = String::from_utf8_lossy(&out);
    let mut lines = text.lines();
    match (lines.next(), lines.next()) {
        (Some(git_dir), Some(common_dir)) => Ok(GitDirs {
            git_dir: PathBuf::from(git_dir),
            common_dir: PathBuf::from(common_dir),
        }),
        _ => Err("git rev-parse printed no git dir".to_owned()),
    }
}

/// Every tracked path, NUL-separated (`git ls-files -z`).
pub fn tracked_listing(root: &Path, login_path: &Arc<OnceLock<String>>) -> Result<Vec<u8>, String> {
    let mut command = git(root, login_path);
    command.args(["ls-files", "-z"]);
    output(command, &[0])
}

/// The paths among `paths` (relative to `root`) that are ignored. Tracked
/// paths are never ignored: `check-ignore` without `--no-index` looks at the
/// index first, so a tracked file that matches a pattern still counts.
pub fn ignored(
    root: &Path,
    login_path: &Arc<OnceLock<String>>,
    paths: &[String],
) -> Result<HashSet<String>, String> {
    let mut command = git(root, login_path);
    command
        .args(["check-ignore", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let input: Vec<u8> = paths
        .iter()
        .flat_map(|p| p.as_bytes().iter().copied().chain(std::iter::once(0)))
        .collect();
    let result = std::thread::scope(|scope| {
        // Written from another thread: git may fill stdout before it reads all of stdin.
        scope.spawn(move || {
            let _ = stdin.write_all(&input);
        });
        child.wait_with_output()
    })
    .map_err(|e| format!("cannot run git: {e}"))?;
    // Exit 1 means none of the paths is ignored.
    match result.status.code() {
        Some(0) => Ok(result
            .stdout
            .split(|&byte| byte == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect()),
        Some(1) => Ok(HashSet::new()),
        _ => Err(String::from_utf8_lossy(&result.stderr).trim().to_owned()),
    }
}

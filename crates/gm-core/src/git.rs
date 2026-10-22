//! `GitPort` over the `git` executable. Status uses porcelain v2 with `-z`:
//! stable, machine-readable, and it honors fsmonitor and the untracked cache.

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use git2::{BranchType, ErrorCode, ObjectType, Oid, Repository};
use gm_schema::{
    BranchEntry, BranchInfo, Change, CommitFile, DiffSide, FileDiff, HookScope, Hunk, HunkId,
    MergedBranch, PruneCandidate, ReflogEntry, RepoStatus, StageTarget, StashEntry, StatusEntry,
    WorktreeEntry, WorktreeMode,
};

use crate::branch;
use crate::command_log::{CommandLog, CommandRecord, Source, bounded_text};
use crate::commit_detail;
use crate::commit_diff;
use crate::diff::{hunk_patch, parse_unified};
use crate::file_index::FileIndex;
use crate::grep::{Collector, Flow, GrepRequest};
use crate::history::{self as log_parse, Canceller, HistoryPage, HistoryStream};
use crate::hook::ConfigValue;
use crate::port::{DetailRead, FileListRead, GrepRead, RemoteRun, StackRead};
use crate::reflog;
use crate::remote;
use crate::stage_math;
use crate::stash;
use crate::worktree;
use crate::{GitError, GitPort};

/// `login_path` is the user's login-shell PATH, filled in once from a
/// background thread. Until it is set, git runs with the process's own PATH.
/// Passing it per command avoids writing the process environment while other
/// threads run.
#[derive(Clone, Default)]
pub struct SubprocessGit {
    pub login_path: Arc<OnceLock<String>>,
    /// Receives every command that changes the repository. Reads are not logged.
    pub log: Option<CommandLog>,
    /// Reads the user-scope hook config from this file instead of the real
    /// global config (`GIT_CONFIG_GLOBAL`, for the tests).
    pub global_config: Option<PathBuf>,
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
        Self::checked(output, ok_codes)
    }

    /// Runs `git` in `root` with `args` and returns its stdout. Once `stop` is
    /// set the child is killed and the run fails. Its output is read on threads,
    /// so a large answer cannot fill the pipe and stall git.
    fn run_stoppable(
        &self,
        root: &Path,
        args: &[&str],
        stop: &AtomicBool,
    ) -> Result<Vec<u8>, GitError> {
        const POLL: Duration = Duration::from_millis(5);
        let superseded = || GitError("the commit read was superseded".into());
        if stop.load(Ordering::SeqCst) {
            return Err(superseded());
        }
        let mut command = self.git(root);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let out = std::thread::spawn(move || read_all(stdout));
        let err = std::thread::spawn(move || read_all(stderr));
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => return Err(GitError(format!("cannot wait for git: {e}"))),
            }
            if stop.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(superseded());
            }
            std::thread::sleep(POLL);
        };
        let output = Output {
            status,
            stdout: out.join().unwrap_or_default(),
            stderr: err.join().unwrap_or_default(),
        };
        Self::checked(output, &[0])
    }

    /// Runs `git grep` for `request` and collects its records under the limits.
    /// The output is read on a thread of its own and handed over a bounded
    /// channel, so this thread can notice `stop` and a full result while Git is
    /// still working, and kill it.
    fn run_grep(
        &self,
        root: &Path,
        request: &GrepRequest,
        stop: &AtomicBool,
    ) -> Result<gm_schema::GrepResult, GitError> {
        use std::io::BufRead;
        use std::sync::mpsc::{RecvTimeoutError, sync_channel};
        const POLL: Duration = Duration::from_millis(5);
        let superseded = || GitError("the text search was superseded".into());
        if stop.load(Ordering::SeqCst) {
            return Err(superseded());
        }
        let mut command = self.git(root);
        command
            .args(request.args())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let (records, arrivals) = sync_channel::<Vec<u8>>(1024);
        let reader = std::thread::spawn(move || {
            let mut input = std::io::BufReader::new(stdout);
            loop {
                let mut record = Vec::new();
                match input.read_until(b'\n', &mut record) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if record.last() == Some(&b'\n') {
                            record.pop();
                        }
                        if records.send(record).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let errors = std::thread::spawn(move || read_all(stderr));
        let mut collector = Collector::new(request);
        let mut full = false;
        loop {
            match arrivals.recv_timeout(POLL) {
                Ok(record) => {
                    if collector.push(&record) == Flow::Full {
                        full = true;
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if stop.load(Ordering::SeqCst) {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(superseded());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if full {
            let _ = child.kill();
        }
        let status = child
            .wait()
            .map_err(|e| GitError(format!("cannot wait for git: {e}")))?;
        drop(arrivals);
        let _ = reader.join();
        let message = errors.join().unwrap_or_default();
        if !full && !matches!(status.code(), Some(0 | 1)) {
            let text = String::from_utf8_lossy(&message);
            let first = text.lines().map(str::trim).find(|line| !line.is_empty());
            return Err(GitError(first.unwrap_or("git grep failed").to_owned()));
        }
        Ok(collector.finish())
    }

    /// Runs `command`, with `input` on its stdin when given.
    fn execute(command: &mut Command, input: Option<&[u8]>) -> std::io::Result<Output> {
        let Some(input) = input else {
            return command.output();
        };
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().expect("stdin was piped");
        std::thread::scope(|scope| {
            // A write error means git exited early; its stderr says why.
            scope.spawn(move || {
                let _ = stdin.write_all(input);
            });
            child.wait_with_output()
        })
    }

    /// Runs a command that changes the repository and records it in the log,
    /// whatever its exit code. The caller decides what a failure means.
    fn logged_output(
        &self,
        mut command: Command,
        input: Option<&[u8]>,
    ) -> Result<Output, GitError> {
        let started = Instant::now();
        let output = Self::execute(&mut command, input)
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        if let Some(log) = &self.log {
            log.record(CommandRecord {
                source: Source::Git,
                program: "git".to_owned(),
                dir: command
                    .get_current_dir()
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
                args: command
                    .get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .filter(|arg| arg != "--no-optional-locks")
                    .collect(),
                code: output.status.code(),
                stdout_bytes: output.stdout.len(),
                stdout: bounded_text(&output.stdout),
                stderr: bounded_text(&output.stderr),
                took: started.elapsed(),
            });
        }
        Ok(output)
    }

    /// Runs a command that changes the repository and records it in the log.
    fn logged(&self, command: Command, input: Option<&[u8]>) -> Result<Vec<u8>, GitError> {
        let output = self.logged_output(command, input)?;
        Self::checked(output, &[0])
    }

    fn checked(output: Output, ok_codes: &[i32]) -> Result<Vec<u8>, GitError> {
        if output.status.code().is_some_and(|c| ok_codes.contains(&c)) {
            Ok(output.stdout)
        } else {
            Err(GitError(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ))
        }
    }

    /// The run of one remote command. It is built here and started on its own
    /// thread. Git never prompts: a credential or host question fails the run
    /// with Git's text instead of waiting for an answer nobody can give.
    fn remote_run(&self, root: &Path, args: Vec<String>) -> RemoteRun {
        let git = self.clone();
        let root = root.to_path_buf();
        Box::new(move || {
            let mut command = git.git(&root);
            command
                .args(&args)
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin(Stdio::null());
            git.logged(command, None).map(drop)
        })
    }

    /// One logged `git` write of `path` for `discard_file`. The path comes after
    /// `--` and the run is `--literal-pathspecs`, so a name such as `:(glob)*`
    /// is never read as a pattern.
    fn discard_run(&self, root: &Path, args: &[&str], path: &str) -> Result<(), GitError> {
        let mut command = self.git(root);
        command
            .arg("--literal-pathspecs")
            .args(args)
            .arg("--")
            .arg(path);
        self.logged(command, None).map(drop)
    }

    /// Deletes the untracked `path`. A folder needs `-d`; a symbolic link is
    /// removed as a link, never followed.
    fn discard_untracked(&self, root: &Path, path: &str) -> Result<(), GitError> {
        let is_dir = std::fs::symlink_metadata(root.join(path.trim_end_matches('/')))
            .is_ok_and(|meta| meta.is_dir());
        let args: &[&str] = if is_dir {
            &["clean", "-fd"]
        } else {
            &["clean", "-f"]
        };
        self.discard_run(root, args, path)
    }

    /// One logged `git stash` write of `stash@{index}`: `apply`, `pop` or `drop`.
    fn stash_command(&self, root: &Path, verb: &str, index: u32) -> Result<(), GitError> {
        let mut command = self.git(root);
        command.args(["stash", verb, &stash_ref(index)]);
        let output = self.logged_output(command, None)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(stash_failure(&output))
        }
    }

    /// The commit HEAD names, `None` on an unborn branch. Never records anything.
    fn head_commit(&self, root: &Path) -> Result<Option<String>, GitError> {
        let mut command = self.git(root);
        command.args(["rev-parse", "--verify", "-q", "HEAD"]);
        // Exit 1 is git's answer for an unborn HEAD.
        let out = Self::run_allowing(command, &[0, 1])?;
        let text = String::from_utf8_lossy(&out).trim().to_owned();
        Ok((!text.is_empty()).then_some(text))
    }

    /// Starts the snapshot index `index` as a copy of the index of the worktree
    /// at `root`, or as HEAD's tree when there is no index yet. The real index is
    /// only read.
    fn seed_index(&self, root: &Path, index: &Path, head: Option<&str>) -> Result<(), GitError> {
        let mut command = self.git(root);
        command.args(["rev-parse", "--path-format=absolute", "--git-path", "index"]);
        let out = Self::run(command)?;
        let real = PathBuf::from(String::from_utf8_lossy(&out).trim_end_matches('\n'));
        if real.is_file() {
            std::fs::copy(&real, index)
                .map_err(|e| GitError(format!("cannot copy the index: {e}")))?;
        } else if let Some(head) = head {
            let mut command = self.git(root);
            command
                .env("GIT_INDEX_FILE", index)
                .args(["read-tree", head]);
            self.logged(command, None)?;
        }
        Ok(())
    }

    /// Whether the index differs from HEAD (from the empty tree on an unborn branch).
    fn has_staged(&self, root: &Path) -> Result<bool, GitError> {
        let mut command = self.git(root);
        command.args(["diff", "--cached", "--quiet"]);
        // `--quiet` exits with 1 when there is a difference.
        let output = command
            .output()
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        match output.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(GitError(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            )),
        }
    }

    /// Writes `content` as a blob and returns its id.
    fn write_blob(&self, root: &Path, content: &[u8]) -> Result<String, GitError> {
        let mut command = self.git(root);
        command.args(["hash-object", "-w", "--stdin"]);
        let out = self.logged(command, Some(content))?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    /// Points the index entry of `path` at blob `id`. The entry carries no stat
    /// data, so git re-reads the working tree file to decide whether it is
    /// modified and a partial stage never looks clean.
    fn put_entry(&self, root: &Path, mode: u32, id: &str, path: &str) -> Result<(), GitError> {
        let mut command = self.git(root);
        command
            .args(["update-index", "--add", "--cacheinfo"])
            .arg(format!("{mode:o}"))
            .args([id, path]);
        self.logged(command, None).map(drop)
    }

    fn remove_entry(&self, root: &Path, path: &str) -> Result<(), GitError> {
        let mut command = self.git(root);
        command.args(["update-index", "--force-remove", "--", path]);
        self.logged(command, None).map(drop)
    }

    fn put_content(
        &self,
        root: &Path,
        mode: u32,
        path: &str,
        content: &[u8],
    ) -> Result<(), GitError> {
        let id = self.write_blob(root, content)?;
        self.put_entry(root, mode, &id, path)
    }

    /// Stores the staged content of a file. Empty content of a file that is gone
    /// from the working tree means the deletion is staged, so the entry goes.
    fn store_staged(
        &self,
        root: &Path,
        index: &git2::Index,
        path: &str,
        mode: u32,
        content: &[u8],
    ) -> Result<(), GitError> {
        if content.is_empty() && root.join(path).symlink_metadata().is_err() {
            if index.get_path(Path::new(path), 0).is_some() {
                self.remove_entry(root, path)?;
            }
            return Ok(());
        }
        self.put_content(root, mode, path, content)
    }

    /// Stages the whole working tree state of `path`, or its deletion.
    fn stage_file(&self, root: &Path, index: &git2::Index, path: &str) -> Result<(), GitError> {
        let p = Path::new(path);
        if root.join(p).symlink_metadata().is_err() && index.get_path(p, 0).is_none() {
            return Err(GitError(format!("pathspec did not match any file: {path}")));
        }
        let mut command = self.git(root);
        command.args(["--literal-pathspecs", "add", "--", path]);
        self.logged(command, None).map(drop)
    }

    /// Removes the `selected` changes of a staged `hunk` from the index entry.
    /// A path that is not in HEAD and ends up empty leaves the index entirely.
    fn unstage_hunk(
        &self,
        root: &Path,
        repo: &Repository,
        index: &git2::Index,
        path: &str,
        hunk: &Hunk,
        selected: &[u32],
    ) -> Result<(), GitError> {
        let (mode, current) = index_content(repo, index, path)?
            .ok_or_else(|| GitError(format!("{path} is not in the index")))?;
        let next = stage_math::unstage_selection(&current, hunk, selected).map_err(GitError)?;
        if next.is_empty() && head_entry(repo, path)?.is_none() {
            self.remove_entry(root, path)
        } else {
            self.put_content(root, mode, path, &next)
        }
    }
}

impl GitPort for SubprocessGit {
    fn login_path(&self) -> Arc<OnceLock<String>> {
        Arc::clone(&self.login_path)
    }

    fn command_log(&self) -> Option<CommandLog> {
        self.log.clone()
    }

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
        let index = repo.index().map_err(|e| git_err(&e))?;
        match target {
            StageTarget::File { path } => self.stage_file(root, &index, path),
            StageTarget::Hunk { path, hunk } => {
                let diff = self.diff(root, path, DiffSide::Unstaged)?;
                let hunk = find_hunk(&diff, hunk)?;
                let (mode, base) = index_base(&repo, &index, root, path)?;
                let next = stage_math::stage_whole_hunk(&base, hunk).map_err(GitError)?;
                self.store_staged(root, &index, path, mode, &next)
            }
            StageTarget::Lines { path, hunk, lines } => {
                let diff = self.diff(root, path, DiffSide::Unstaged)?;
                let hunk = find_hunk(&diff, hunk)?;
                let (mode, base) = index_base(&repo, &index, root, path)?;
                let next = stage_math::stage_selection(&base, hunk, lines).map_err(GitError)?;
                self.store_staged(root, &index, path, mode, &next)
            }
        }
    }

    fn unstage(&self, root: &Path, target: &StageTarget) -> Result<(), GitError> {
        let repo = open(root)?;
        check_index_lock(&repo)?;
        let index = repo.index().map_err(|e| git_err(&e))?;
        match target {
            StageTarget::File { path } => match head_entry(&repo, path)? {
                Some((mode, id)) => self.put_entry(root, mode, &id.to_string(), path),
                None if index.get_path(Path::new(path), 0).is_some() => {
                    self.remove_entry(root, path)
                }
                None => Ok(()),
            },
            StageTarget::Hunk { path, hunk } => {
                let diff = self.diff(root, path, DiffSide::Staged)?;
                let hunk = find_hunk(&diff, hunk)?;
                let selected = change_indices(hunk);
                self.unstage_hunk(root, &repo, &index, path, hunk, &selected)
            }
            StageTarget::Lines { path, hunk, lines } => {
                let diff = self.diff(root, path, DiffSide::Staged)?;
                let hunk = find_hunk(&diff, hunk)?;
                self.unstage_hunk(root, &repo, &index, path, hunk, lines)
            }
        }
    }

    fn discard_hunk(&self, root: &Path, path: &str, hunk: &HunkId) -> Result<(), GitError> {
        let repo = open(root)?;
        if !is_tracked(&repo, path)? {
            return Err(GitError(format!(
                "{path} is untracked: its chunks cannot be discarded"
            )));
        }
        let diff = self.diff(root, path, DiffSide::Unstaged)?;
        let hunk = find_hunk(&diff, hunk)?;
        // Reversed against the working tree only: the index keeps what it has.
        let mut command = self.git(root);
        command.args(["apply", "--reverse"]);
        self.logged(command, Some(hunk_patch(path, hunk).as_bytes()))
            .map(drop)
    }

    fn discard_file(&self, root: &Path, path: &str, include_staged: bool) -> Result<(), GitError> {
        check_index_lock(&open(root)?)?;
        let status = self.status(root)?;
        let entry = status
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| GitError(format!("{path} has no changes to discard")))?;
        if entry.conflicted {
            return Err(GitError(format!(
                "{path} is conflicted: resolve the conflict first"
            )));
        }
        if entry.untracked {
            return self.discard_untracked(root, path);
        }
        if !include_staged {
            return match entry.unstaged {
                None => Err(GitError(format!("{path} has nothing unstaged to discard"))),
                // An intent-to-add file (`git add -N`) has no content in the index to
                // return to, so its whole content is the change: remove it.
                Some(Change::Added) => self.discard_run(root, &["rm", "-f"], path),
                Some(_) => self.discard_run(root, &["restore", "--worktree"], path),
            };
        }
        match entry.staged {
            Some(Change::Added | Change::Copied) => self.discard_run(root, &["rm", "-f"], path),
            Some(Change::Renamed) => {
                // The new path is an addition; the old one is still in HEAD.
                self.discard_run(root, &["rm", "-f"], path)?;
                match &entry.orig_path {
                    Some(orig) => self.discard_run(
                        root,
                        &["restore", "--source=HEAD", "--staged", "--worktree"],
                        orig,
                    ),
                    None => Ok(()),
                }
            }
            _ => self.discard_run(
                root,
                &["restore", "--source=HEAD", "--staged", "--worktree"],
                path,
            ),
        }
    }

    fn commit(&self, root: &Path, message: &str) -> Result<String, GitError> {
        let repo = open(root)?;
        check_index_lock(&repo)?;
        if message.trim().is_empty() {
            return Err(GitError("commit message is empty".into()));
        }
        if !self.has_staged(root)? {
            return Err(GitError("nothing staged to commit".into()));
        }
        // Native hooks run here (D18): the message goes in on stdin, so nothing
        // is written to the working tree, and a failing hook's report is the error.
        let mut command = self.git(root);
        command.args(["commit", "-F", "-"]);
        self.logged(command, Some(message.as_bytes()))?;
        let mut command = self.git(root);
        command.args(["rev-parse", "HEAD"]);
        let out = Self::run(command)?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    fn stage_all(&self, root: &Path) -> Result<(), GitError> {
        check_index_lock(&open(root)?)?;
        let mut command = self.git(root);
        command.args(["add", "-A"]);
        self.logged(command, None).map(drop)
    }

    fn stashes(&self, root: &Path) -> Result<Vec<StashEntry>, GitError> {
        let mut command = self.git(root);
        command.args(["stash", "list", stash::LIST_FORMAT]);
        stash::parse_list(&Self::run(command)?)
    }

    fn resolve_stash(&self, root: &Path, index: u32) -> Result<Option<String>, GitError> {
        let mut command = self.git(root);
        command.args(["rev-parse", "--verify", "-q", &stash_ref(index)]);
        // Exit 1 is git's answer for a stash that is not there.
        let out = Self::run_allowing(command, &[0, 1])?;
        let text = String::from_utf8_lossy(&out).trim().to_owned();
        Ok((!text.is_empty()).then_some(text))
    }

    fn create_stash(
        &self,
        root: &Path,
        message: Option<&str>,
        include_untracked: bool,
    ) -> Result<(), GitError> {
        check_index_lock(&open(root)?)?;
        // Git exits 0 and says so on stdout when nothing is stashed. That is
        // checked here first, so a refusal is not recorded as a write.
        let changed = self.status(root)?.entries.iter().any(|entry| {
            entry.staged.is_some()
                || entry.unstaged.is_some()
                || (include_untracked && entry.untracked)
        });
        if !changed {
            return Err(GitError(NOTHING_TO_STASH.into()));
        }
        let mut command = self.git(root);
        command.args(["stash", "push"]);
        if include_untracked {
            command.arg("--include-untracked");
        }
        if let Some(message) = message {
            command.args(["-m", message]);
        }
        let out = self.logged(command, None)?;
        // A change the status did not show (a file that only looked modified) is
        // still nothing to stash.
        if String::from_utf8_lossy(&out).trim() == NOTHING_TO_STASH {
            return Err(GitError(NOTHING_TO_STASH.into()));
        }
        Ok(())
    }

    fn apply_stash(&self, root: &Path, index: u32) -> Result<(), GitError> {
        check_index_lock(&open(root)?)?;
        self.stash_command(root, "apply", index)
    }

    fn pop_stash(&self, root: &Path, index: u32) -> Result<(), GitError> {
        check_index_lock(&open(root)?)?;
        self.stash_command(root, "pop", index)
    }

    fn drop_stash(&self, root: &Path, index: u32) -> Result<(), GitError> {
        self.stash_command(root, "drop", index)
    }

    fn reflog(&self, root: &Path, skip: u32, limit: usize) -> Result<Vec<ReflogEntry>, GitError> {
        // An unborn HEAD makes `git reflog` fail; asking first keeps Git's wording out.
        let mut head = self.git(root);
        head.args(["rev-parse", "--verify", "-q", "HEAD"]);
        if Self::run_allowing(head, &[0, 1])?.is_empty() {
            return Ok(Vec::new());
        }
        let mut command = self.git(root);
        command
            .args(["reflog", "show", "-z", "--date=unix", reflog::LIST_FORMAT])
            .arg(format!("-n{limit}"))
            .arg(format!("--skip={skip}"))
            .arg("HEAD");
        reflog::parse_list(&Self::run(command)?, skip)
    }

    fn snapshot_worktree(&self, root: &Path) -> Result<String, GitError> {
        let common = self.common_dir(root)?;
        let tmp = common.join("gitmaster").join("tmp");
        std::fs::create_dir_all(&tmp)
            .map_err(|e| GitError(format!("cannot create {}: {e}", tmp.display())))?;
        // The folder and the copy of the index in it go when `scratch` drops, on
        // every way out of this function.
        let scratch = tempfile::TempDir::new_in(&tmp)
            .map_err(|e| GitError(format!("cannot create a snapshot folder: {e}")))?;
        let index = scratch.path().join("index");
        let head = self.head_commit(root)?;
        self.seed_index(root, &index, head.as_deref())?;
        let mut add = self.git(root);
        add.env("GIT_INDEX_FILE", &index).args(["add", "-A"]);
        self.logged(add, None)?;
        let mut write = self.git(root);
        write.env("GIT_INDEX_FILE", &index).arg("write-tree");
        let tree = String::from_utf8_lossy(&self.logged(write, None)?)
            .trim()
            .to_owned();
        let mut commit = self.git(root);
        commit.args(["-c", "commit.gpgsign=false", "commit-tree", &tree]);
        if let Some(head) = &head {
            commit.args(["-p", head]);
        }
        commit
            .args(["-m", "gitmaster review snapshot"])
            .env("GIT_AUTHOR_NAME", SNAPSHOT_NAME)
            .env("GIT_AUTHOR_EMAIL", SNAPSHOT_EMAIL)
            .env("GIT_COMMITTER_NAME", SNAPSHOT_NAME)
            .env("GIT_COMMITTER_EMAIL", SNAPSHOT_EMAIL);
        let out = self.logged(commit, None)?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    fn set_review_ref(&self, root: &Path, comment: i64, oid: &str) -> Result<(), GitError> {
        let mut command = self.git(root);
        command.args(["update-ref", &review_ref(comment), oid]);
        self.logged(command, None).map(drop)
    }

    fn delete_review_ref(&self, root: &Path, comment: i64) -> Result<(), GitError> {
        let mut command = self.git(root);
        command.args(["update-ref", "-d", &review_ref(comment)]);
        self.logged(command, None).map(drop)
    }

    fn file_at_commit(
        &self,
        root: &Path,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitError> {
        let mut list = self.git(root);
        list.args([
            "--literal-pathspecs",
            "ls-tree",
            "-z",
            "--full-tree",
            oid,
            "--",
            path,
        ]);
        let out = Self::run(list)?;
        // One record: `<mode> SP <type> SP <object> TAB <path>`, then a NUL.
        let record = out.split(|byte| *byte == 0).next().unwrap_or_default();
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            return Ok(None);
        };
        let header = String::from_utf8_lossy(&record[..tab]).into_owned();
        let mut fields = header.split(' ').skip(1);
        let (Some("blob"), Some(object)) = (fields.next(), fields.next()) else {
            return Ok(None);
        };
        let mut cat = self.git(root);
        cat.args(["cat-file", "blob", object]);
        Self::run(cat).map(Some)
    }

    fn common_dir(&self, root: &Path) -> Result<PathBuf, GitError> {
        let mut command = self.git(root);
        command.args(["rev-parse", "--path-format=absolute", "--git-common-dir"]);
        let out = Self::run(command)?;
        Ok(PathBuf::from(
            String::from_utf8_lossy(&out).trim_end_matches('\n'),
        ))
    }

    fn config_values(&self, root: &Path, key: &str) -> Result<Vec<String>, GitError> {
        let mut command = self.git(root);
        command.args(["config", "--includes", "--null", "--get-all", key]);
        // Exit 1 is git's answer for a key that is not set.
        let out = Self::run_allowing(command, &[0, 1])?;
        Ok(String::from_utf8_lossy(&out)
            .split('\0')
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect())
    }

    fn user_email(&self, root: &Path) -> Result<Option<String>, GitError> {
        let mut command = self.git(root);
        command.args(["config", "user.email"]);
        // Exit 1 is git's answer for an unset key, not a failure.
        let out = Self::run_allowing(command, &[0, 1])?;
        Ok(clean_email(&String::from_utf8_lossy(&out)))
    }

    fn branches(&self, root: &Path) -> Result<Vec<BranchEntry>, GitError> {
        let repo = open(root)?;
        let current = current_branch(&repo)?;
        let mut local = Vec::new();
        for item in repo
            .branches(Some(BranchType::Local))
            .map_err(|e| git_err(&e))?
        {
            let (branch, _) = item.map_err(|e| git_err(&e))?;
            let name = branch_name(&branch)?;
            let upstream = branch
                .upstream()
                .ok()
                .map(|up| branch_name(&up))
                .transpose()?;
            local.push(BranchEntry {
                is_current: current.as_deref() == Some(name.as_str()),
                author_email: tip_author_email(&branch),
                name,
                is_remote: false,
                upstream,
            });
        }
        let mut remote = Vec::new();
        for item in repo
            .branches(Some(BranchType::Remote))
            .map_err(|e| git_err(&e))?
        {
            let (branch, _) = item.map_err(|e| git_err(&e))?;
            let name = branch_name(&branch)?;
            // `origin/HEAD` is a symbolic alias of the default branch, not a branch.
            if name.ends_with("/HEAD") {
                continue;
            }
            remote.push(BranchEntry {
                author_email: tip_author_email(&branch),
                name,
                is_remote: true,
                is_current: false,
                upstream: None,
            });
        }
        local.sort_by(|a, b| a.name.cmp(&b.name));
        remote.sort_by(|a, b| a.name.cmp(&b.name));
        local.extend(remote);
        Ok(local)
    }

    /// Starts `git log --topo-order` for HEAD and returns its walk, read a page
    /// at a time (D32). `None` when HEAD is unborn. Git reads a commit-graph
    /// when one exists and never writes one. Runs without logging (D28).
    fn history_stream(&self, root: &Path) -> Result<Option<Box<dyn HistoryStream>>, GitError> {
        // An unborn HEAD makes `git log` fail; asking first keeps Git's wording out.
        let mut head = self.git(root);
        head.args(["rev-parse", "--verify", "-q", "HEAD"]);
        if Self::run_allowing(head, &[0, 1])?.is_empty() {
            return Ok(None);
        }
        let mut log = self.git(root);
        log.args([
            "log",
            "--no-color",
            "--no-show-signature",
            "--decorate=short",
            "--topo-order",
            log_parse::FORMAT,
            "HEAD",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Nothing reads stderr while the walk is paused, so a full pipe would stall git.
        .stderr(Stdio::null());
        let mut child = log
            .spawn()
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        let stdout = child.stdout.take().expect("stdout was piped");
        Ok(Some(Box::new(LogStream {
            child: Arc::new(Mutex::new(child)),
            input: BufReader::new(stdout),
        })))
    }

    /// The detail of commit `oid`: its header, then its files against the first
    /// parent. Two `git show` runs, neither logged (D28), both stoppable.
    fn commit_detail(&self, root: &Path, oid: &str) -> Result<DetailRead, GitError> {
        require_commit_id(oid)?;
        let git = self.clone();
        let root = root.to_path_buf();
        let oid = oid.to_owned();
        Ok(Box::new(move |stop: &AtomicBool| {
            let header = git.run_stoppable(
                &root,
                &[
                    "show",
                    "-s",
                    "--no-color",
                    "--no-show-signature",
                    commit_detail::HEADER_FORMAT,
                    &oid,
                ],
                stop,
            )?;
            let mut detail = commit_detail::parse_header(&header)?;
            // `--diff-merges=first-parent` compares a merge with its first parent (Git 2.31+).
            let files = git.run_stoppable(
                &root,
                &[
                    "show",
                    "--raw",
                    "-z",
                    "--numstat",
                    "--format=",
                    "--diff-merges=first-parent",
                    "-M",
                    &oid,
                ],
                stop,
            )?;
            let (files, more) = commit_detail::parse_files(&files)?;
            detail.files = files;
            detail.more_files = more;
            Ok(detail)
        }))
    }

    /// The files of the worktree, from one `git ls-files`: tracked (`--cached`)
    /// and untracked-not-ignored (`--others --exclude-standard`). `-v` tags each
    /// path so the files a sparse checkout leaves out can be dropped. Stoppable
    /// and not logged (D28).
    fn file_list(&self, root: &Path) -> Result<FileListRead, GitError> {
        let git = self.clone();
        let root = root.to_path_buf();
        Ok(Box::new(move |stop: &AtomicBool| {
            let raw = git.run_stoppable(
                &root,
                &[
                    "ls-files",
                    "-z",
                    "-v",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                ],
                stop,
            )?;
            Ok(FileIndex::from_ls_files(raw))
        }))
    }

    /// A text search of the worktree with `git grep` (N3). Git's records are read
    /// as they come, and Git is killed as soon as the result limit is passed or
    /// `stop` is set. Not logged (D28). Exit 1 means no match, not a failure.
    fn grep_text(&self, root: &Path, request: &GrepRequest) -> Result<GrepRead, GitError> {
        let git = self.clone();
        let root = root.to_path_buf();
        let request = request.clone();
        Ok(Box::new(move |stop: &AtomicBool| {
            git.run_grep(&root, &request, stop)
        }))
    }

    /// The diffs of every file of commit `oid` against its first parent, or
    /// against nothing for a root commit, in one `git show` (H4c). The patch
    /// lists the files in the order of the detail's `--raw` listing, so
    /// `commit_diff::parse_stack` pairs them by position. Git is stoppable and
    /// the call is not logged (D28).
    fn commit_stack(
        &self,
        root: &Path,
        oid: &str,
        files: &[CommitFile],
    ) -> Result<StackRead, GitError> {
        require_commit_id(oid)?;
        let git = self.clone();
        let root = root.to_path_buf();
        let oid = oid.to_owned();
        let files = files.to_vec();
        Ok(Box::new(move |stop: &AtomicBool| {
            let args = [
                "show",
                "--no-color",
                "--no-ext-diff",
                "--no-show-signature",
                "--histogram",
                "-U3",
                "--format=",
                "--patch",
                "--diff-merges=first-parent",
                "--root",
                "-M",
                oid.as_str(),
            ];
            let raw = git.run_stoppable(&root, &args, stop)?;
            commit_diff::parse_stack(&files, &raw)
        }))
    }

    fn worktrees(&self, root: &Path) -> Result<Vec<WorktreeEntry>, GitError> {
        // `-z` needs git 2.36; older gits get the newline form, which cannot
        // tell a path with a newline from two lines.
        let mut command = self.git(root);
        command.args(["worktree", "list", "--porcelain", "-z"]);
        if let Ok(out) = Self::run(command) {
            return worktree::parse(&out, true);
        }
        let mut command = self.git(root);
        command.args(["worktree", "list", "--porcelain"]);
        let out = Self::run(command)?;
        worktree::parse(&out, false)
    }

    fn create_worktree(
        &self,
        root: &Path,
        path: &str,
        mode: &WorktreeMode,
    ) -> Result<(), GitError> {
        // A leading dash would be read as an option by `git worktree add`.
        let refs = match mode {
            WorktreeMode::Existing { branch } => vec![branch],
            WorktreeMode::New { branch, base } => vec![branch, base],
            WorktreeMode::Detached { commit } => vec![commit],
        };
        if path.is_empty()
            || path.starts_with('-')
            || refs.iter().any(|r| r.is_empty() || r.starts_with('-'))
        {
            return Err(GitError("invalid worktree path or revision".into()));
        }
        // Git would accept an empty folder that already exists; the dialog
        // promises a new one, so the refusal is ours, worded as Git words it.
        if root.join(path).exists() {
            return Err(GitError(format!("'{path}' already exists")));
        }
        let mut command = self.git(root);
        match mode {
            WorktreeMode::Existing { branch } => {
                command.args(["worktree", "add", "--", path, branch]);
            }
            WorktreeMode::New { branch, base } => {
                command.args(["worktree", "add", "-b", branch, "--", path, base]);
            }
            WorktreeMode::Detached { commit } => {
                command.args(["worktree", "add", "--detach", "--", path, commit]);
            }
        }
        self.logged(command, None).map(drop)
    }

    fn unpushed_commits(&self, path: &Path, branch: Option<&str>) -> Result<u32, GitError> {
        let repo = open(path)?;
        if repo.head().is_err() {
            // An unborn HEAD has no commits to lose.
            return Ok(0);
        }
        // HEAD's commits that no other local branch or remote has. The
        // worktree's own branch is left out, so its own commits are counted.
        // The refs are listed here, not with `--exclude`, whose matching
        // differs by namespace.
        let own = branch.map(|b| format!("refs/heads/{b}"));
        let mut input = String::from("HEAD\n");
        for reference in repo.references().map_err(|e| git_err(&e))? {
            let reference = reference.map_err(|e| git_err(&e))?;
            let Some(name) = reference.name() else {
                continue;
            };
            let shared = name.starts_with("refs/heads/") || name.starts_with("refs/remotes/");
            if shared && own.as_deref() != Some(name) {
                input.push('^');
                input.push_str(name);
                input.push('\n');
            }
        }
        let mut command = self.git(path);
        command.args(["rev-list", "--count", "--stdin"]);
        let output = Self::execute(&mut command, Some(input.as_bytes()))
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        let mut count = parse_count(&Self::checked(output, &[0])?)?;
        // Commits ahead of the upstream, when the branch has one.
        if let Some(branch) = branch {
            let upstream = repo
                .find_branch(branch, BranchType::Local)
                .ok()
                .and_then(|local| local.upstream().ok())
                .and_then(|upstream| upstream.get().name().map(str::to_owned));
            if let Some(name) = upstream {
                let mut command = self.git(path);
                command.args(["rev-list", "--count", &format!("{name}..HEAD")]);
                count = count.max(parse_count(&Self::run(command)?)?);
            }
        }
        Ok(count)
    }

    fn remove_worktree(&self, root: &Path, path: &str, force: u8) -> Result<(), GitError> {
        if path.is_empty() || path.starts_with('-') {
            return Err(GitError("invalid worktree path".into()));
        }
        // Git deletes the folder itself. Force is one `--force`, or two to take a locked one.
        let mut command = self.git(root);
        command.args(["worktree", "remove"]);
        for _ in 0..force.min(2) {
            command.arg("--force");
        }
        command.args(["--", path]);
        self.logged(command, None).map(drop)
    }

    fn lock_worktree(&self, root: &Path, path: &str, reason: &str) -> Result<(), GitError> {
        if path.is_empty() || path.starts_with('-') {
            return Err(GitError("invalid worktree path".into()));
        }
        let mut command = self.git(root);
        command.args(["worktree", "lock"]);
        if !reason.is_empty() {
            command.arg(format!("--reason={reason}"));
        }
        command.args(["--", path]);
        self.logged(command, None).map(drop)
    }

    fn unlock_worktree(&self, root: &Path, path: &str) -> Result<(), GitError> {
        if path.is_empty() || path.starts_with('-') {
            return Err(GitError("invalid worktree path".into()));
        }
        let mut command = self.git(root);
        command.args(["worktree", "unlock", "--", path]);
        self.logged(command, None).map(drop)
    }

    fn prunable(&self, root: &Path) -> Result<Vec<PruneCandidate>, GitError> {
        // Dry run only. Git prints the records on stdout or stderr depending on
        // the version, so both are read.
        let mut command = self.git(root);
        command.args(["worktree", "prune", "-n", "-v"]);
        let output = command
            .output()
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        if !output.status.success() {
            return Err(GitError(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        let mut text = output.stdout;
        text.push(b'\n');
        text.extend_from_slice(&output.stderr);
        Ok(worktree::prune_candidates(&text))
    }

    fn prune_worktrees(&self, root: &Path) -> Result<(), GitError> {
        let mut command = self.git(root);
        command.args(["worktree", "prune"]);
        self.logged(command, None).map(drop)
    }

    fn checkout(&self, root: &Path, branch: &str) -> Result<(), GitError> {
        // A leading dash would be read as an option by `git switch`.
        if branch.is_empty() || branch.starts_with('-') {
            return Err(GitError(format!("invalid branch name: {branch}")));
        }
        let repo = open(root)?;
        check_index_lock(&repo)?;
        let mut command = self.git(root);
        if has_ref(&repo, &format!("refs/heads/{branch}")) {
            command.args(["switch", "--", branch]);
        } else if has_ref(&repo, &format!("refs/remotes/{branch}")) {
            // A remote-only branch: reuse the local branch of the same short
            // name when there is one, otherwise create it tracking the remote.
            let local = branch.split_once('/').map(|(_, rest)| rest);
            match local {
                Some(local) if has_ref(&repo, &format!("refs/heads/{local}")) => {
                    command.args(["switch", "--", local]);
                }
                _ => {
                    command.args(["switch", "--track", "--", branch]);
                }
            }
        } else {
            // Unknown name: git reports it.
            command.args(["switch", "--", branch]);
        }
        self.logged(command, None).map(drop)
    }

    fn checkout_commit(&self, root: &Path, oid: &str) -> Result<(), GitError> {
        // The same rule the engine applies: only a hex id reaches Git.
        if let Some(problem) = reflog::oid_problem(oid) {
            return Err(GitError(problem.into()));
        }
        let repo = open(root)?;
        check_index_lock(&repo)?;
        let mut command = self.git(root);
        command.args(["checkout", "--detach", oid]);
        self.logged(command, None).map(drop)
    }

    fn create_branch(
        &self,
        root: &Path,
        name: &str,
        start_point: Option<&str>,
        checkout: bool,
    ) -> Result<(), GitError> {
        // The same rules the engine applies: a name or start point read as an option is refused here too.
        if let Some(problem) =
            branch::name_problem(name).or_else(|| start_point.and_then(branch::start_point_problem))
        {
            return Err(GitError(problem.into()));
        }
        let mut command = self.git(root);
        if checkout {
            let repo = open(root)?;
            check_index_lock(&repo)?;
            match start_point {
                Some(start) => command.args(["checkout", "-b", name, start]),
                None => command.args(["checkout", "-b", name]),
            };
        } else {
            match start_point {
                Some(start) => command.args(["branch", name, start]),
                None => command.args(["branch", name]),
            };
        }
        self.logged(command, None).map(drop)
    }

    fn delete_branch(&self, root: &Path, name: &str, force: bool) -> Result<(), GitError> {
        if let Some(problem) = branch::name_problem(name) {
            return Err(GitError(problem.into()));
        }
        let mut command = self.git(root);
        command.args(["branch", if force { "-D" } else { "-d" }, name]);
        self.logged(command, None).map(drop)
    }

    fn origin_head(&self, root: &Path) -> Result<Option<String>, GitError> {
        // Git fails when origin/HEAD is not a symbolic ref, which is an answer here.
        let mut command = self.git(root);
        command.args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]);
        Ok(Self::run(command).ok().and_then(|out| {
            let text = String::from_utf8_lossy(&out).trim().to_owned();
            (!text.is_empty()).then_some(text)
        }))
    }

    fn merged_branches(&self, root: &Path, base: &str) -> Result<Vec<MergedBranch>, GitError> {
        // NUL separates the fields, so no subject can be mistaken for a separator.
        let mut command = self.git(root);
        command.args([
            "branch",
            "--merged",
            base,
            "--format=%(refname:short)%00%(objectname:short)%00%(contents:subject)",
        ]);
        let out = Self::run(command)?;
        // `git branch -d` checks a branch against HEAD, not against the base, so
        // a branch merged only into the base would be listed and then refused.
        // Only the branches merged into HEAD as well are offered. An unborn HEAD
        // merges nothing.
        let mut into_head = self.git(root);
        into_head.args(["branch", "--merged", "HEAD", "--format=%(refname:short)"]);
        let head_out = Self::run(into_head).unwrap_or_default();
        let head_merged: Vec<String> = String::from_utf8_lossy(&head_out)
            .lines()
            .map(str::to_owned)
            .collect();
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(3, '\0');
                let name = fields.next()?.to_owned();
                let tip = fields.next()?.to_owned();
                let subject = fields.next().unwrap_or_default().to_owned();
                Some(MergedBranch { name, tip, subject })
            })
            .filter(|merged| head_merged.contains(&merged.name))
            .collect())
    }

    fn fetch(&self, root: &Path) -> Result<RemoteRun, GitError> {
        Ok(self.remote_run(root, remote::fetch_args()))
    }

    fn pull(&self, root: &Path) -> Result<RemoteRun, GitError> {
        let repo = open(root)?;
        let branch = current_branch(&repo)?;
        let upstream = match &branch {
            Some(branch) => upstream_of(&repo, branch)?,
            None => None,
        };
        let args = remote::pull_args(branch.as_deref(), upstream.is_some()).map_err(GitError)?;
        Ok(self.remote_run(root, args))
    }

    fn push_with_lease(&self, root: &Path) -> Result<RemoteRun, GitError> {
        let repo = open(root)?;
        let branch = current_branch(&repo)?;
        let upstream = match &branch {
            Some(branch) => upstream_of(&repo, branch)?,
            None => None,
        };
        let names = remote_names(&repo)?;
        let args = remote::push_args(
            branch.as_deref(),
            upstream
                .as_ref()
                .map(|(remote, merge)| (remote.as_str(), merge.as_str())),
            &names,
        )
        .map_err(GitError)?;
        Ok(self.remote_run(root, args))
    }

    fn hook_config(&self, root: &Path, scope: HookScope) -> Result<Vec<ConfigValue>, GitError> {
        // `--includes` follows `includeIf` and `include`: for the user scope
        // that is the point (D17), and for the repo scope it is why such hooks
        // still need approval.
        let flag = match scope {
            HookScope::User => "--global",
            HookScope::Repo => "--local",
        };
        let mut command = self.git(root);
        command.args([
            "config",
            flag,
            "--includes",
            "--null",
            "--get-regexp",
            HOOK_KEYS,
        ]);
        if let (HookScope::User, Some(path)) = (scope, &self.global_config) {
            command.env("GIT_CONFIG_GLOBAL", path);
        }
        // Exit 1 means no key matches.
        let bytes = Self::run_allowing(command, &[0, 1])?;
        Ok(parse_null_config(&bytes))
    }

    fn staged_diff(&self, root: &Path) -> Result<String, GitError> {
        let mut command = self.git(root);
        command.args([
            "diff",
            "--cached",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
        ]);
        Ok(String::from_utf8_lossy(&Self::run(command)?).into_owned())
    }
}

/// Keys of every prepare-commit hook section, as a regular expression.
const HOOK_KEYS: &str = r"^gitmaster\.prepare-commit\.";

/// `git config --null --get-regexp` output: each record is `key\nvalue`, or
/// just `key` for a bare key.
#[must_use]
pub fn parse_null_config(bytes: &[u8]) -> Vec<ConfigValue> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let text = String::from_utf8_lossy(record);
            match text.split_once('\n') {
                Some((key, value)) => ConfigValue {
                    key: key.to_owned(),
                    value: Some(value.to_owned()),
                },
                None => ConfigValue {
                    key: text.into_owned(),
                    value: None,
                },
            }
        })
        .collect()
}

/// What `git stash push` says when there is nothing to stash (English: the
/// commands run with `LC_ALL=C`).
const NOTHING_TO_STASH: &str = "No local changes to save";

/// The revision of `stash@{index}`.
/// The private ref that keeps the snapshot of review comment `comment` alive (D97).
fn review_ref(comment: i64) -> String {
    format!("refs/gitmaster/review/{comment}/before")
}

/// The identity of snapshot commits: they are the engine's, not the user's work.
const SNAPSHOT_NAME: &str = "GitMaster";
const SNAPSHOT_EMAIL: &str = "gitmaster@localhost";

fn stash_ref(index: u32) -> String {
    format!("stash@{{{index}}}")
}

/// Git's text for a failed stash apply or pop. A conflict is printed on stdout
/// (the `CONFLICT` lines, and the note that the stash is kept), so those lines
/// are the message; any other refusal is on stderr.
fn stash_failure(output: &Output) -> GitError {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let conflicts: Vec<&str> = stdout
        .lines()
        .filter(|line| line.starts_with("CONFLICT ") || line.starts_with("The stash entry is kept"))
        .collect();
    if conflicts.is_empty() {
        GitError(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    } else {
        GitError(conflicts.join("\n"))
    }
}

fn git_err(error: &git2::Error) -> GitError {
    GitError(error.message().to_owned())
}

fn parse_count(out: &[u8]) -> Result<u32, GitError> {
    String::from_utf8_lossy(out)
        .trim()
        .parse()
        .map_err(|_| GitError("unexpected rev-list output".into()))
}

fn branch_name(branch: &git2::Branch<'_>) -> Result<String, GitError> {
    branch
        .name()
        .map_err(|e| git_err(&e))?
        .map(str::to_owned)
        .ok_or_else(|| GitError("branch name is not valid UTF-8".into()))
}

/// The branch HEAD points at, also on an unborn branch. `None` when detached.
fn current_branch(repo: &Repository) -> Result<Option<String>, GitError> {
    let head = repo.find_reference("HEAD").map_err(|e| git_err(&e))?;
    Ok(head
        .symbolic_target()
        .and_then(|target| target.strip_prefix("refs/heads/"))
        .map(str::to_owned))
}

/// The author email of the commit a branch points at. A tip that cannot be
/// read gives `None` rather than failing the whole listing.
fn tip_author_email(branch: &git2::Branch<'_>) -> Option<String> {
    let commit = branch.get().peel_to_commit().ok()?;
    clean_email(commit.author().email()?)
}

/// An email as the listing shows it: trimmed, without the angle brackets Git
/// prints around it, and `None` when nothing is left.
fn clean_email(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or(trimmed)
        .trim();
    (!inner.is_empty()).then(|| inner.to_owned())
}

/// The remote and the full name of the remote branch that `branch` tracks, when
/// both `branch.<name>.remote` and `branch.<name>.merge` are set.
fn upstream_of(repo: &Repository, branch: &str) -> Result<Option<(String, String)>, GitError> {
    let config = repo.config().map_err(|e| git_err(&e))?;
    let remote = config.get_string(&format!("branch.{branch}.remote")).ok();
    let merge = config.get_string(&format!("branch.{branch}.merge")).ok();
    Ok(remote.zip(merge))
}

fn remote_names(repo: &Repository) -> Result<Vec<String>, GitError> {
    let names = repo.remotes().map_err(|e| git_err(&e))?;
    Ok(names.iter().flatten().map(str::to_owned).collect())
}

fn has_ref(repo: &Repository, name: &str) -> bool {
    repo.find_reference(name).is_ok()
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

/// Mode and blob id of `path` in HEAD. `None` when HEAD is unborn or the path
/// is not a file in HEAD.
fn head_entry(repo: &Repository, path: &str) -> Result<Option<(u32, Oid)>, GitError> {
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
    let mode = u32::try_from(entry.filemode())
        .map_err(|_| GitError(format!("unexpected file mode in HEAD: {path}")))?;
    Ok(Some((mode, entry.id())))
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

/// Everything the reader gives until its end. A read that fails keeps what it had.
fn read_all(mut reader: impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = reader.read_to_end(&mut buffer);
    buffer
}

fn lock(child: &Mutex<Child>) -> MutexGuard<'_, Child> {
    // A poisoned lock still holds the child, which must be stopped and reaped anyway.
    child
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The `git log` child behind a history walk. Dropping the walk stops the
/// child and reaps it.
struct LogStream {
    child: Arc<Mutex<Child>>,
    input: BufReader<ChildStdout>,
}

impl LogStream {
    /// Waits for git once its output has ended; a failed walk is an error.
    fn finish(&self) -> Result<(), GitError> {
        let status = lock(&self.child)
            .wait()
            .map_err(|e| GitError(format!("cannot wait for git: {e}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(GitError(format!("git log failed ({status})")))
        }
    }
}

impl HistoryStream for LogStream {
    fn next_page(&mut self, limit: usize) -> Result<HistoryPage, GitError> {
        let (commits, done) = log_parse::read_page(&mut self.input, limit)?;
        if done {
            self.finish()?;
        }
        Ok(HistoryPage {
            commits,
            done,
            unborn: false,
        })
    }

    fn skip_past(&mut self, oid: &str) -> Result<(), GitError> {
        if log_parse::skip_past(&mut self.input, oid)? {
            return Ok(());
        }
        self.finish()?;
        Err(GitError(format!("commit {oid} is not in the history")))
    }

    fn canceller(&self) -> Canceller {
        let child = Arc::clone(&self.child);
        Arc::new(move || {
            // The child may have exited already; then there is nothing to stop.
            let _ = lock(&child).kill();
        })
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        let mut child = lock(&self.child);
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// A full or abbreviated id is hex; anything else never reaches git's arguments.
fn require_commit_id(oid: &str) -> Result<(), GitError> {
    if oid.is_empty() || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitError(format!("not a commit id: {oid:?}")));
    }
    Ok(())
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

    #[test]
    fn clean_email_strips_the_angle_brackets() {
        assert_eq!(
            clean_email("<ana@example.com>").as_deref(),
            Some("ana@example.com")
        );
        assert_eq!(
            clean_email("  ana@example.com \n").as_deref(),
            Some("ana@example.com")
        );
    }

    #[test]
    fn clean_email_gives_none_for_nothing() {
        assert_eq!(clean_email(""), None);
        assert_eq!(clean_email(" \n"), None);
        assert_eq!(clean_email("<>"), None);
        assert_eq!(clean_email("< >"), None);
    }
}

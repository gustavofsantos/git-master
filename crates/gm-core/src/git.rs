//! `GitPort` over the `git` executable. Status uses porcelain v2 with `-z`:
//! stable, machine-readable, and it honors fsmonitor and the untracked cache.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use git2::{BranchType, ConfigLevel, ErrorCode, ObjectType, Oid, Repository};
use gm_schema::{
    BranchEntry, BranchInfo, Change, CommitFile, ConflictFileRead, ConflictOperation, DiffSide,
    FileDiff, HookScope, Hunk, HunkId, MergedBranch, OWN_COMMITS_LIMIT, PruneCandidate,
    RecordedResolution, ReflogEntry, RepoStatus, StageTarget, StashEntry, StatusEntry,
    WorktreeEntry, WorktreeMode,
};

use crate::blame;
use crate::branch;
use crate::command_log::{CommandLog, CommandRecord, Source, bounded_text};
use crate::commit_detail;
use crate::commit_diff;
use crate::conflict;
use crate::conflict_sides::{self, SideCommit, SideFacts};
use crate::diff::{hunk_patch, parse_unified};
use crate::file_index::FileIndex;
use crate::grep::{Collector, Flow, GrepRequest};
use crate::history::{self as log_parse, Canceller, HistoryPage, HistoryStream};
use crate::hook::ConfigValue;
use crate::port::{
    BlameRead, CommitPresence, CommitSnapshot, DetailRead, FileListRead, GrepRead, HistoryRevs,
    RemoteRun, Rewrite, StackRead,
};
use crate::port::{DiscardOutcome, FileMerge, RefMove, ReviewSide, TreeMerge};
use crate::reflog;
use crate::remote;
use crate::restack::{ReplayKind, Replayed};
use crate::stack::{self, EdgeFacts, StackLink};
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
    /// Makes every command read this file instead of the real global config
    /// (`GIT_CONFIG_GLOBAL`, for the tests), so the user's own settings, such as
    /// `rerere.autoupdate`, cannot change what a test sees.
    pub global_config: Option<PathBuf>,
}

impl SubprocessGit {
    fn git(&self, dir: &Path) -> Command {
        let mut command = Command::new("git");
        if let Some(path) = self.login_path.get() {
            command.env("PATH", path);
        }
        if let Some(path) = &self.global_config {
            command.env("GIT_CONFIG_GLOBAL", path);
        }
        // --no-optional-locks: never take index.lock against an agent running git.
        command
            .arg("--no-optional-locks")
            .current_dir(dir)
            .env("LC_ALL", "C");
        command
    }

    fn all_worktrees(&self, root: &Path) -> Result<Vec<WorktreeEntry>, GitError> {
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

    /// The commit's parents, author, dates and message, as a replay keeps them (D142).
    fn replay_metadata(&self, root: &Path, commit: &str) -> Result<ReplayMetadata, GitError> {
        // One read: parents, author, author date (raw keeps the offset) and message.
        let mut command = self.git(root);
        command.args([
            "log",
            "-1",
            "--no-show-signature",
            "--date=raw",
            "--format=%P%x00%an%x00%ae%x00%ad%x00%cd%x00%B",
            commit,
        ]);
        let out = Self::run(command)?;
        let text = String::from_utf8_lossy(&out);
        let mut fields = text.splitn(6, '\0');
        let mut next = || fields.next().unwrap_or_default().to_owned();
        let (parents, name, email, date, committed, message) =
            (next(), next(), next(), next(), next(), next());
        Ok(ReplayMetadata {
            parents: parents.split_whitespace().map(str::to_owned).collect(),
            name,
            email,
            date,
            committed,
            message,
        })
    }

    /// Writes `tree` as a commit on `parent` with the metadata of `meta`: the author and both
    /// dates are kept, the committer is the user's own identity (D142). Only an object is
    /// written: no ref moves.
    fn commit_replayed(
        &self,
        dir: &Path,
        tree: &str,
        parent: &str,
        meta: &ReplayMetadata,
    ) -> Result<String, GitError> {
        let mut command = self.git(dir);
        command
            .args(["commit-tree", tree, "-p", parent])
            .env("GIT_AUTHOR_NAME", &meta.name)
            .env("GIT_AUTHOR_EMAIL", &meta.email)
            .env("GIT_AUTHOR_DATE", &meta.date)
            .env("GIT_COMMITTER_DATE", &meta.committed);
        let output = Self::execute(&mut command, Some(meta.message.as_bytes()))
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        let new = Self::checked(output, &[0])?;
        Ok(String::from_utf8_lossy(&new).trim().to_owned())
    }

    /// The worktree that resolves a restack conflict (D154), inside the common dir.
    fn resolution_worktree(&self, root: &Path) -> Result<PathBuf, GitError> {
        Ok(self
            .common_dir(root)?
            .join("gitmaster")
            .join("restack-resolve"))
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
        self.verify_rev(root, "HEAD")
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
            self.read_tree(root, index, head, true)?;
        }
        Ok(())
    }

    /// Fills the snapshot index `index` with the tree `tree` (`read-tree`).
    /// `record` puts the run in the command log.
    fn read_tree(
        &self,
        root: &Path,
        index: &Path,
        tree: &str,
        record: bool,
    ) -> Result<(), GitError> {
        let mut command = self.git(root);
        command
            .env("GIT_INDEX_FILE", index)
            .args(["read-tree", tree]);
        self.snapshot_step(command, record).map(drop)
    }

    /// Runs one plumbing step of a snapshot. Recorded in the command log when
    /// `record` is set; otherwise it only writes objects and is not.
    fn snapshot_step(&self, command: Command, record: bool) -> Result<Vec<u8>, GitError> {
        if record {
            self.logged(command, None)
        } else {
            Self::run(command)
        }
    }

    /// A folder under `<common_dir>/gitmaster/tmp` for scratch files. It and its
    /// contents go when the returned value drops, on every way out.
    fn scratch_dir(&self, root: &Path) -> Result<tempfile::TempDir, GitError> {
        let common = self.common_dir(root)?;
        let tmp = common.join("gitmaster").join("tmp");
        std::fs::create_dir_all(&tmp)
            .map_err(|e| GitError(format!("cannot create {}: {e}", tmp.display())))?;
        tempfile::TempDir::new_in(&tmp)
            .map_err(|e| GitError(format!("cannot create a scratch folder: {e}")))
    }

    /// The id `rev` names, `None` when it names nothing. Never records anything.
    fn verify_rev(&self, root: &Path, rev: &str) -> Result<Option<String>, GitError> {
        let mut command = self.git(root);
        command.args(["rev-parse", "--verify", "-q", rev]);
        // Exit 1 is git's answer for a revision that is not there.
        let out = Self::run_allowing(command, &[0, 1])?;
        let text = String::from_utf8_lossy(&out).trim().to_owned();
        Ok((!text.is_empty()).then_some(text))
    }

    /// The commit id that `rev` names, such as `HEAD` or `refs/heads/main`, so
    /// that only object ids reach a command that takes them as arguments. A
    /// name that starts with `-` is refused as an option. Never records anything.
    fn commit_of(&self, root: &Path, rev: &str) -> Result<String, GitError> {
        if rev.starts_with('-') {
            return Err(GitError(format!("not a revision: {rev:?}")));
        }
        self.verify_rev(root, &format!("{rev}^{{commit}}"))?
            .ok_or_else(|| GitError(format!("not a commit: {rev}")))
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
    /// Builds a commit of the working tree (`paths` of `None`) or of HEAD with
    /// only `paths` taken from the working tree (`Some`), for `message`. The work
    /// is done in a copy of the index, which is removed afterwards.
    fn snapshot(
        &self,
        root: &Path,
        paths: Option<&[String]>,
        message: &str,
    ) -> Result<String, GitError> {
        // The folder and the copy of the index in it go when `scratch` drops, on
        // every way out of this function.
        let scratch = self.scratch_dir(root)?;
        let index = scratch.path().join("index");
        let head = self.head_commit(root)?;
        // A move snapshot writes unreferenced objects only, so it is not recorded;
        // the ref that later points at it is (D109).
        let record = paths.is_none();
        match paths {
            None => self.seed_index(root, &index, head.as_deref())?,
            // A move snapshot holds HEAD everywhere except the chosen paths, so it
            // starts from HEAD rather than from the real index.
            Some(_) => {
                if let Some(head) = head.as_deref() {
                    self.read_tree(root, &index, head, record)?;
                }
            }
        }
        let mut add = self.git(root);
        add.env("GIT_INDEX_FILE", &index);
        match paths {
            None => add.args(["add", "-A"]),
            Some(paths) => add
                .args(["--literal-pathspecs", "add", "-A", "--"])
                .args(paths),
        };
        self.snapshot_step(add, record)?;
        let mut write = self.git(root);
        write.env("GIT_INDEX_FILE", &index).arg("write-tree");
        let tree = String::from_utf8_lossy(&self.snapshot_step(write, record)?)
            .trim()
            .to_owned();
        let mut commit = self.git(root);
        commit.args(["-c", "commit.gpgsign=false", "commit-tree", &tree]);
        if let Some(head) = &head {
            commit.args(["-p", head]);
        }
        commit
            .args(["-m", message])
            .env("GIT_AUTHOR_NAME", SNAPSHOT_NAME)
            .env("GIT_AUTHOR_EMAIL", SNAPSHOT_EMAIL)
            .env("GIT_COMMITTER_NAME", SNAPSHOT_NAME)
            .env("GIT_COMMITTER_EMAIL", SNAPSHOT_EMAIL);
        let out = self.snapshot_step(commit, record)?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
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
        self.snapshot(root, None, "gitmaster review snapshot")
    }

    fn snapshot_paths(&self, root: &Path, paths: &[String]) -> Result<String, GitError> {
        self.snapshot(root, Some(paths), "gitmaster move snapshot")
    }

    fn git_version(&self) -> Result<String, GitError> {
        // Not in a repository: the process's own folder is as good as any.
        let mut command = self.git(Path::new("."));
        command.arg("--version");
        Ok(String::from_utf8_lossy(&Self::run(command)?)
            .trim()
            .to_owned())
    }

    fn merge_trees(
        &self,
        root: &Path,
        base: &str,
        ours: &str,
        theirs: &str,
    ) -> Result<TreeMerge, GitError> {
        let (base, ours, theirs) = (
            self.commit_of(root, base)?,
            self.commit_of(root, ours)?,
            self.commit_of(root, theirs)?,
        );
        let mut command = self.git(root);
        command
            .args(["merge-tree", "--write-tree", "--name-only", "-z"])
            .arg(format!("--merge-base={base}"))
            .args([ours, theirs]);
        // Not logged: it writes objects only, no ref and no index.
        let output = command
            .output()
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        match output.status.code() {
            Some(0) => parse_merge_tree(&output.stdout, false),
            Some(1) => parse_merge_tree(&output.stdout, true),
            _ => Err(GitError(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            )),
        }
    }

    fn commit_tree(
        &self,
        root: &Path,
        tree: &str,
        parent: Option<&str>,
        message: &str,
    ) -> Result<String, GitError> {
        require_commit_id(tree)?;
        if let Some(parent) = parent {
            require_commit_id(parent)?;
        }
        // The user's own identity: no snapshot identity is set here.
        let mut command = self.git(root);
        command.args(["commit-tree", tree]);
        if let Some(parent) = parent {
            command.args(["-p", parent]);
        }
        command.args(["-m", message]);
        let out = self.logged(command, None)?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    fn update_ref(
        &self,
        root: &Path,
        name: &str,
        new: &str,
        old: Option<&str>,
    ) -> Result<(), GitError> {
        if !name.starts_with("refs/") {
            return Err(GitError(format!("{name} is not a ref under refs/")));
        }
        require_commit_id(new)?;
        if let Some(old) = old {
            require_commit_id(old)?;
        }
        // An empty old value makes Git require that the ref does not exist.
        let mut command = self.git(root);
        command.args(["update-ref", name, new, old.unwrap_or("")]);
        self.logged(command, None).map(drop)
    }

    fn merge_file(
        &self,
        root: &Path,
        base: &[u8],
        ours: &[u8],
        theirs: &[u8],
    ) -> Result<FileMerge, GitError> {
        if [base, ours, theirs]
            .iter()
            .any(|content| looks_binary(content))
        {
            return Ok(FileMerge::Binary);
        }
        let scratch = self.scratch_dir(root)?;
        let write = |name: &str, content: &[u8]| -> Result<PathBuf, GitError> {
            let path = scratch.path().join(name);
            std::fs::write(&path, content)
                .map_err(|e| GitError(format!("cannot write {}: {e}", path.display())))?;
            Ok(path)
        };
        let base_path = write("base", base)?;
        let ours_path = write("ours", ours)?;
        let theirs_path = write("theirs", theirs)?;
        // Not logged: a computation that changes nothing in the repository.
        let mut command = self.git(root);
        command
            .args(["merge-file", "-p"])
            .arg(&ours_path)
            .arg(&base_path)
            .arg(&theirs_path);
        let output = command
            .output()
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        match output.status.code() {
            Some(0) => Ok(FileMerge::Clean(output.stdout)),
            // merge-file exits with the number of conflicts, at most 127.
            Some(1..=127) => Ok(FileMerge::Conflict),
            _ => Err(GitError(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            )),
        }
    }

    fn discard_moved(
        &self,
        root: &Path,
        path: &str,
        snapshot: &str,
    ) -> Result<DiscardOutcome, GitError> {
        require_commit_id(snapshot)?;
        check_worktree_path(root, path)?;
        let in_snapshot = self.verify_rev(root, &format!("{snapshot}:{path}"))?;
        let file = root.join(path);
        let on_disk = match file.symlink_metadata() {
            Ok(meta) if meta.is_file() => {
                let mut command = self.git(root);
                command.args(["hash-object", "--", path]);
                Some(
                    String::from_utf8_lossy(&Self::run(command)?)
                        .trim()
                        .to_owned(),
                )
            }
            // A folder or another non-file now sits at `path`: it is not the file
            // the snapshot holds, so nothing is discarded.
            Ok(_) => {
                return Ok(DiscardOutcome::Kept {
                    reason: MOVED_CHANGED.into(),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(GitError(format!("cannot read {path}: {e}"))),
        };
        if in_snapshot != on_disk {
            return Ok(DiscardOutcome::Kept {
                reason: MOVED_CHANGED.into(),
            });
        }
        self.discard_file(root, path, true)?;
        Ok(DiscardOutcome::Discarded)
    }

    fn set_review_ref(
        &self,
        root: &Path,
        comment: i64,
        side: ReviewSide,
        oid: &str,
    ) -> Result<(), GitError> {
        let mut command = self.git(root);
        command.args(["update-ref", &review_ref(comment, side), oid]);
        self.logged(command, None).map(drop)
    }

    fn delete_review_ref(
        &self,
        root: &Path,
        comment: i64,
        side: ReviewSide,
    ) -> Result<(), GitError> {
        let mut command = self.git(root);
        command.args(["update-ref", "-d", &review_ref(comment, side)]);
        self.logged(command, None).map(drop)
    }

    fn diff_commits(
        &self,
        root: &Path,
        from: &str,
        to: &str,
        path: &str,
    ) -> Result<FileDiff, GitError> {
        let from = self.commit_of(root, from)?;
        let to = self.commit_of(root, to)?;
        let mut command = self.git(root);
        command.args([
            "--literal-pathspecs",
            "-c",
            "core.quotepath=off",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            "--histogram",
            "-U0",
            &from,
            &to,
            "--",
            path,
        ]);
        let raw = Self::run(command)?;
        parse_unified(path, &raw)
    }

    fn resolve_commit(&self, root: &Path, rev: &str) -> Result<String, GitError> {
        self.commit_of(root, rev)
    }

    fn commit_snapshot(&self, root: &Path, rev: &str) -> Result<CommitSnapshot, GitError> {
        let id = self.commit_of(root, rev)?;
        let mut log = self.git(root);
        log.args([
            "log",
            "-1",
            "--no-show-signature",
            "--format=%s%x00%an%x00%ct",
            id.as_str(),
            "--",
        ]);
        let out = Self::run(log)?;
        let text = String::from_utf8_lossy(&out);
        let mut fields = text.trim_end_matches('\n').split('\0');
        let (Some(subject), Some(author), Some(seconds)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(GitError(format!("no commit {id}")));
        };
        let seconds: i64 = seconds
            .trim()
            .parse()
            .map_err(|_| GitError(format!("no commit date for {id}")))?;
        Ok(CommitSnapshot {
            id,
            subject: subject.to_owned(),
            author: author.to_owned(),
            committed_at: seconds.saturating_mul(1000),
        })
    }

    fn commit_snapshots(
        &self,
        root: &Path,
        revs: &[String],
    ) -> Result<Vec<(String, CommitSnapshot)>, GitError> {
        // Names that could read as options, or hold a line break, never reach Git.
        let asked: Vec<&String> = revs
            .iter()
            .filter(|rev| !rev.starts_with('-') && !rev.contains(char::is_whitespace))
            .collect();
        if asked.is_empty() {
            return Ok(Vec::new());
        }
        let mut input = Vec::new();
        for rev in &asked {
            input.extend_from_slice(rev.as_bytes());
            input.push(b'\n');
        }
        let mut cat = self.git(root);
        cat.args(["cat-file", "--batch"]);
        let output = Self::execute(&mut cat, Some(&input))
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        let out = Self::checked(output, &[0])?;
        // One answer per name, in order: `<oid> <type> <size>` and the object, or `<name> missing`.
        let mut found = Vec::new();
        let mut at = 0;
        for rev in asked {
            let Some(end) = out[at..].iter().position(|byte| *byte == b'\n') else {
                break;
            };
            let header = String::from_utf8_lossy(&out[at..at + end]).into_owned();
            at += end + 1;
            // A missing or ambiguous name has two fields and no object after it.
            let mut fields = header.split(' ');
            let (Some(oid), Some(kind), Some(size)) = (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let Ok(size) = size.parse::<usize>() else {
                break;
            };
            let Some(body) = out.get(at..at + size) else {
                break;
            };
            at += size + 1;
            if kind == "commit"
                && let Some(snapshot) = parse_commit_snapshot(oid, body)
            {
                found.push((rev.clone(), snapshot));
            }
        }
        Ok(found)
    }

    fn file_at_commit(
        &self,
        root: &Path,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitError> {
        let Some(object) = self.blob_at_commit(root, oid, path)? else {
            return Ok(None);
        };
        let mut cat = self.git(root);
        cat.args(["cat-file", "blob", object.as_str()]);
        Self::run(cat).map(Some)
    }

    fn blob_at_commit(
        &self,
        root: &Path,
        oid: &str,
        path: &str,
    ) -> Result<Option<String>, GitError> {
        // `HEAD` and the like are fine; an option is not.
        if oid.starts_with('-') {
            return Err(GitError(format!("not a revision: {oid:?}")));
        }
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
        Ok(Some(object.to_owned()))
    }

    fn blame_lines(
        &self,
        root: &Path,
        path: &str,
        from: u32,
        to: u32,
    ) -> Result<BlameRead, GitError> {
        if from == 0 || to < from {
            return Err(GitError(format!("not a line range: {from} to {to}")));
        }
        let git = self.clone();
        let root = root.to_path_buf();
        let path = path.to_owned();
        Ok(Box::new(move |stop: &AtomicBool| {
            let range = format!("{from},{to}");
            // `-M -C` finds lines moved or copied from other files (D144). The path comes
            // last, after `--`, so no name is read as an option.
            let args = [
                "blame",
                "--line-porcelain",
                "-M",
                "-C",
                "-L",
                range.as_str(),
                "--",
                path.as_str(),
            ];
            let raw = git.run_stoppable(&root, &args, stop)?;
            blame::parse_line_porcelain(&raw).map_err(GitError)
        }))
    }

    fn commit_presence(&self, root: &Path, oid: &str) -> Result<CommitPresence, GitError> {
        let id = object_id_arg(oid)?;
        let mut exists = self.git(root);
        exists.args(["cat-file", "-e", &format!("{id}^{{commit}}")]);
        if !succeeds(exists) {
            return Ok(CommitPresence::Missing);
        }
        let mut contains = self.git(root);
        contains.args([
            "for-each-ref",
            "--count=1",
            "--format=%(refname)",
            "--contains",
            id.as_str(),
        ]);
        if !Self::run(contains)?.is_empty() {
            return Ok(CommitPresence::Reachable);
        }
        // HEAD is not a ref that `for-each-ref` lists, so a detached HEAD is checked on its own.
        let mut head = self.git(root);
        head.args(["merge-base", "--is-ancestor", id.as_str(), "HEAD"]);
        Ok(if succeeds(head) {
            CommitPresence::Reachable
        } else {
            CommitPresence::Unreachable
        })
    }

    fn rewrite_candidates(
        &self,
        root: &Path,
        branch: Option<&str>,
    ) -> Result<Vec<String>, GitError> {
        let mut found: Vec<String> = Vec::new();
        let mut orig = self.git(root);
        orig.args(["rev-parse", "--verify", "-q", "ORIG_HEAD^{commit}"]);
        if let Ok(out) = Self::run_allowing(orig, &[0, 1]) {
            let text = String::from_utf8_lossy(&out).trim().to_owned();
            if !text.is_empty() {
                found.push(text);
            }
        }
        let reference = match branch {
            Some(name) if name.starts_with('-') => {
                return Err(GitError(format!("not a branch: {name:?}")));
            }
            Some(name) => format!("refs/heads/{name}"),
            None => "HEAD".to_owned(),
        };
        let mut reflog = self.git(root);
        reflog.args([
            "log",
            "-g",
            "--format=%H",
            "-n",
            "20",
            "--no-show-signature",
            reference.as_str(),
            "--",
        ]);
        // A branch with no reflog (or none yet) only adds nothing.
        let entries = Self::run(reflog).unwrap_or_default();
        for line in String::from_utf8_lossy(&entries).lines() {
            let id = line.trim();
            if !id.is_empty() && !found.iter().any(|known| known == id) {
                found.push(id.to_owned());
            }
        }
        Ok(found)
    }

    fn rewritten_to(
        &self,
        root: &Path,
        commit: &str,
        candidates: &[String],
        new_tip: &str,
    ) -> Result<Rewrite, GitError> {
        let id = object_id_arg(commit)?;
        let new_id = object_id_arg(new_tip)?;
        // The old tip is the first candidate that has the commit in its history. A candidate Git
        // no longer has (pruned) cannot hold it, so it is skipped.
        let mut old = None;
        for candidate in candidates {
            let Ok(candidate) = object_id_arg(candidate) else {
                continue;
            };
            let mut ancestor = self.git(root);
            ancestor.args([
                "merge-base",
                "--is-ancestor",
                id.as_str(),
                candidate.as_str(),
            ]);
            if succeeds(ancestor) {
                old = Some(candidate);
                break;
            }
        }
        let Some(old) = old else {
            return Ok(Rewrite::NoMatch);
        };
        let mut shared = self.git(root);
        shared.args(["merge-base", old.as_str(), new_id.as_str()]);
        let Ok(base_out) = Self::run(shared) else {
            // No shared history: nothing in the new tip can be the commit's patch.
            return Ok(Rewrite::NoMatch);
        };
        let base = String::from_utf8_lossy(&base_out).trim().to_owned();
        if base.is_empty() {
            return Ok(Rewrite::NoMatch);
        }
        let old_patches = patch_ids(self, root, &base, &old)?;
        let Some(patch) = old_patches
            .iter()
            .find(|(_, found)| *found == id)
            .map(|(patch, _)| patch.clone())
        else {
            return Err(GitError(format!(
                "{id} has no patch to compare (a merge commit?)"
            )));
        };
        let new_patches = patch_ids(self, root, &base, &new_id)?;
        let matches: Vec<&String> = new_patches
            .iter()
            .filter(|(found, _)| *found == patch)
            .map(|(_, commit)| commit)
            .collect();
        Ok(match matches.as_slice() {
            [] => Rewrite::NoMatch,
            [only] => Rewrite::Unique((*only).clone()),
            _ => Rewrite::Ambiguous,
        })
    }

    fn renames_to_head(&self, root: &Path) -> Result<Vec<(String, String)>, GitError> {
        let mut log = self.git(root);
        log.args([
            "-c",
            "core.quotePath=false",
            "log",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-show-signature",
            "-M",
            "--diff-filter=R",
            "--name-status",
            "--format=",
            "--reverse",
            "--max-count=5000",
            "HEAD",
            "--",
        ]);
        let out = Self::run(log)?;
        let mut renames = Vec::new();
        for line in String::from_utf8_lossy(&out).lines() {
            let mut fields = line.split('\t');
            let (Some(status), Some(old), Some(new)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if status.starts_with('R') {
                renames.push((old.to_owned(), new.to_owned()));
            }
        }
        Ok(renames)
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

    /// Starts `git log --topo-order` for `revs` and returns its walk, read a page
    /// at a time (D32, D139). `None` when HEAD is unborn and the walk is of HEAD.
    /// Git reads a commit-graph when one exists and never writes one. Runs
    /// without logging (D28).
    fn history_stream(
        &self,
        root: &Path,
        revs: &HistoryRevs,
    ) -> Result<Option<Box<dyn HistoryStream>>, GitError> {
        // An unborn HEAD makes `git log` fail; asking first keeps Git's wording out.
        let mut head = self.git(root);
        head.args(["rev-parse", "--verify", "-q", "HEAD"]);
        let born = !Self::run_allowing(head, &[0, 1])?.is_empty();
        if !born && *revs == HistoryRevs::Head {
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
        ]);
        match revs {
            HistoryRevs::Head => {
                log.arg("HEAD");
            }
            HistoryRevs::All => {
                log.args(["--branches", "--remotes", "--tags"]);
                if born {
                    log.arg("HEAD");
                }
            }
            HistoryRevs::Refs { refs, with_head } => {
                // After this, nothing Git reads is an option, whatever a name starts with.
                log.arg("--end-of-options").args(refs);
                if *with_head && born {
                    log.arg("HEAD");
                }
            }
        }
        // The paths that would follow: none, so no ref is mistaken for a path.
        log.arg("--")
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
        // The folders under `<common dir>/gitmaster/` are internal (D154): not listed.
        let internal = canonical_or_as_is(&self.common_dir(root)?).join("gitmaster");
        let mut entries = self.all_worktrees(root)?;
        entries.retain(|entry| !canonical_or_as_is(Path::new(&entry.path)).starts_with(&internal));
        Ok(entries)
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

    fn stack_links(&self, root: &Path) -> Result<Vec<StackLink>, GitError> {
        let repo = open(root)?;
        let config = repo.config().map_err(|e| git_err(&e))?;
        // Only the repository's own file holds stack links (D13).
        let local = match config.open_level(ConfigLevel::Local) {
            Ok(local) => local,
            Err(e) if e.code() == ErrorCode::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(git_err(&e)),
        };
        let mut entries = Vec::new();
        let mut iter = local
            .entries(Some(r"^branch\..*\.stackparent(base)?$"))
            .map_err(|e| git_err(&e))?;
        while let Some(entry) = iter.next() {
            let entry = entry.map_err(|e| git_err(&e))?;
            if let (Some(name), Some(value)) = (entry.name(), entry.value()) {
                entries.push((name.to_owned(), value.to_owned()));
            }
        }
        Ok(stack::links_from_config(&entries))
    }

    fn stack_edge(
        &self,
        root: &Path,
        branch: &str,
        parent: &str,
        base: Option<&str>,
    ) -> Result<EdgeFacts, GitError> {
        let repo = open(root)?;
        let tip = local_tip(&repo, branch)?
            .ok_or_else(|| GitError(format!("there is no local branch {branch}")))?;
        let parent_tip = local_tip(&repo, parent)?
            .ok_or_else(|| GitError(format!("there is no local branch {parent}")))?;
        let (ahead, behind) = repo
            .graph_ahead_behind(tip, parent_tip)
            .map_err(|e| git_err(&e))?;
        let merge_base = optional_merge_base(&repo, tip, parent_tip)?;
        // A base that is not a commit of this repository any more is not in the branch.
        let base = base
            .and_then(|text| Oid::from_str(text).ok())
            .filter(|oid| repo.find_commit(*oid).is_ok());
        let own = match base {
            Some(base) => commits_between(&repo, tip, base, OWN_COMMITS_LIMIT)?
                .into_iter()
                .map(|oid| oid.to_string())
                .collect(),
            None => Vec::new(),
        };
        let (base_in_branch, merge_base_past_base) = match base {
            None => (false, false),
            Some(base) => {
                let in_branch = base == tip || descends(&repo, tip, base)?;
                let past = match merge_base {
                    Some(merge_base) if merge_base != base => descends(&repo, merge_base, base)?,
                    _ => false,
                };
                (in_branch, past)
            }
        };
        Ok(EdgeFacts {
            parent_tip: parent_tip.to_string(),
            ahead: u32::try_from(ahead).unwrap_or(u32::MAX),
            behind: u32::try_from(behind).unwrap_or(u32::MAX),
            base_in_branch,
            merge_base_past_base,
            own,
        })
    }

    fn branch_landed(
        &self,
        root: &Path,
        branch: &str,
        own_base: Option<&str>,
        into: &str,
    ) -> Result<bool, GitError> {
        let repo = open(root)?;
        let (Some(tip), Some(into_tip)) = (local_tip(&repo, branch)?, local_tip(&repo, into)?)
        else {
            return Ok(false);
        };
        let Some(merge_base) = optional_merge_base(&repo, tip, into_tip)? else {
            return Ok(false);
        };
        let own_base = own_base.and_then(|text| Oid::from_str(text).ok());
        if merge_base == tip {
            // Reachable from `into`: merged when the branch had commits of its own.
            return Ok(match own_base {
                Some(base) => base != tip && descends(&repo, tip, base)?,
                None => false,
            });
        }
        let own = commits_between(&repo, tip, merge_base, usize::MAX)?;
        let landed: HashSet<Oid> = commits_between(&repo, into_tip, merge_base, LANDED_SCAN)?
            .into_iter()
            .filter_map(|oid| commit_patch_id(&repo, oid).transpose())
            .collect::<Result<_, _>>()?;
        if landed.is_empty() {
            return Ok(false);
        }
        let mut every_commit = true;
        for oid in own {
            match commit_patch_id(&repo, oid)? {
                Some(id) if landed.contains(&id) => {}
                // A merge commit of the branch has no patch of its own to look for.
                None => {}
                Some(_) => {
                    every_commit = false;
                    break;
                }
            }
        }
        if every_commit {
            return Ok(true);
        }
        let squash = tree_patch_id(&repo, merge_base, tip)?;
        Ok(squash.is_some_and(|id| landed.contains(&id)))
    }

    fn branch_tip(&self, root: &Path, name: &str) -> Result<Option<String>, GitError> {
        let repo = open(root)?;
        Ok(local_tip(&repo, name)?.map(|oid| oid.to_string()))
    }

    fn local_tips(&self, root: &Path) -> Result<Vec<(String, String)>, GitError> {
        let repo = open(root)?;
        let mut tips = Vec::new();
        for item in repo
            .branches(Some(BranchType::Local))
            .map_err(|e| git_err(&e))?
        {
            let (branch, _) = item.map_err(|e| git_err(&e))?;
            let name = branch_name(&branch)?;
            if let Some(oid) = branch.get().target() {
                tips.push((name, oid.to_string()));
            }
        }
        tips.sort();
        Ok(tips)
    }

    fn branch_merge_base(
        &self,
        root: &Path,
        one: &str,
        two: &str,
    ) -> Result<Option<String>, GitError> {
        let repo = open(root)?;
        let (Some(one), Some(two)) = (local_tip(&repo, one)?, local_tip(&repo, two)?) else {
            return Ok(None);
        };
        Ok(optional_merge_base(&repo, one, two)?.map(|oid| oid.to_string()))
    }

    fn update_refs(&self, root: &Path, moves: &[RefMove]) -> Result<(), GitError> {
        let mut input = String::from("start\n");
        for item in moves {
            if !item.name.starts_with("refs/") || item.name.contains(['\n', '\0', ' ']) {
                return Err(GitError(format!("{} is not a ref under refs/", item.name)));
            }
            require_commit_id(&item.new)?;
            require_commit_id(&item.old)?;
            let _ = writeln!(input, "update {} {} {}", item.name, item.new, item.old);
        }
        input.push_str("prepare\ncommit\n");
        let mut command = self.git(root);
        command.args(["update-ref", "--stdin"]);
        self.logged(command, Some(input.as_bytes())).map(drop)
    }

    fn refresh_checkout(&self, root: &Path, old: &str, new: &str) -> Result<(), GitError> {
        require_commit_id(old)?;
        require_commit_id(new)?;
        let mut command = self.git(root);
        command.args(["read-tree", "-m", "-u", old, new]);
        self.logged(command, None).map(drop)
    }

    fn restack_replay(&self, root: &Path, onto: &str, commit: &str) -> Result<Replayed, GitError> {
        require_commit_id(onto)?;
        require_commit_id(commit)?;
        let meta = self.replay_metadata(root, commit)?;
        let subject = meta.message.lines().next().unwrap_or_default().to_owned();
        let [parent] = meta.parents.as_slice() else {
            return Ok(Replayed {
                subject,
                kind: ReplayKind::NotLinear,
            });
        };
        let kind = match self.merge_trees(root, parent, onto, commit)? {
            TreeMerge::Conflict { paths } => ReplayKind::Conflict { paths },
            TreeMerge::Clean { tree } => {
                let mut command = self.git(root);
                command.args(["rev-parse", &format!("{onto}^{{tree}}")]);
                let onto_tree = String::from_utf8_lossy(&Self::run(command)?)
                    .trim()
                    .to_owned();
                if tree == onto_tree {
                    ReplayKind::Dropped
                } else {
                    ReplayKind::Clean {
                        new: self.commit_replayed(root, &tree, onto, &meta)?,
                    }
                }
            }
        };
        Ok(Replayed { subject, kind })
    }

    fn restack_resolution_open(
        &self,
        root: &Path,
        onto: &str,
        commit: &str,
    ) -> Result<PathBuf, GitError> {
        require_commit_id(onto)?;
        require_commit_id(commit)?;
        let path = self.resolution_worktree(root)?;
        // A worktree left by an earlier resolution is replaced, not reused.
        self.restack_resolution_close(root)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| GitError(format!("cannot create {}: {e}", parent.display())))?;
        }
        let mut command = self.git(root);
        command
            .args(["worktree", "add", "--detach", "--"])
            .arg(&path)
            .arg(onto);
        self.logged(command, None)?;
        // Exit 1 is a conflict, which is what this is for; the index keeps the stages.
        let mut command = self.git(&path);
        command.args([
            "-c",
            "rerere.enabled=true",
            "cherry-pick",
            "--no-commit",
            commit,
        ]);
        let picked = self
            .logged_output(command, None)
            .and_then(|output| Self::checked(output, &[0, 1]).map(drop));
        if let Err(error) = picked {
            let _ = self.restack_resolution_close(root);
            return Err(error);
        }
        Ok(path)
    }

    fn restack_resolution_unmerged(&self, worktree: &Path) -> Result<Vec<String>, GitError> {
        let mut command = self.git(worktree);
        command.args(["ls-files", "-u", "-z"]);
        // Each record is `mode oid stage<TAB>path`; a path repeats once per stage.
        let out = Self::run(command)?;
        let mut paths: Vec<String> = out
            .split(|byte| *byte == 0)
            .filter_map(|record| {
                String::from_utf8_lossy(record)
                    .split_once('\t')
                    .map(|(_, path)| path.to_owned())
            })
            .collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    fn restack_resolution_commit(
        &self,
        root: &Path,
        worktree: &Path,
        onto: &str,
        commit: &str,
    ) -> Result<String, GitError> {
        require_commit_id(onto)?;
        require_commit_id(commit)?;
        let unmerged = self.restack_resolution_unmerged(worktree)?;
        if !unmerged.is_empty() {
            return Err(GitError(format!(
                "resolve the conflicts first: {}",
                unmerged.join(", ")
            )));
        }
        let mut command = self.git(worktree);
        command.arg("write-tree");
        let tree = String::from_utf8_lossy(&Self::run(command)?)
            .trim()
            .to_owned();
        let meta = self.replay_metadata(root, commit)?;
        self.commit_replayed(worktree, &tree, onto, &meta)
    }

    fn restack_resolution_close(&self, root: &Path) -> Result<(), GitError> {
        let path = self.resolution_worktree(root)?;
        let wanted = canonical_or_as_is(&path);
        let registered = self
            .all_worktrees(root)?
            .iter()
            .any(|entry| canonical_or_as_is(Path::new(&entry.path)) == wanted);
        if registered {
            let mut command = self.git(root);
            command
                .args(["worktree", "remove", "--force", "--"])
                .arg(&path);
            let removed = self.logged(command, None).map(drop);
            // A folder deleted by hand leaves nothing to remove; that is not a failure.
            if removed.is_err() && path.exists() {
                return removed;
            }
        }
        if path.exists() {
            std::fs::remove_dir_all(&path)
                .map_err(|e| GitError(format!("cannot remove {}: {e}", path.display())))?;
        }
        self.prune_worktrees(root)
    }

    fn set_ref(&self, root: &Path, name: &str, oid: &str) -> Result<(), GitError> {
        ref_under_refs(name)?;
        require_commit_id(oid)?;
        let mut command = self.git(root);
        command.args(["update-ref", name, oid]);
        self.logged(command, None).map(drop)
    }

    fn delete_ref(&self, root: &Path, name: &str) -> Result<(), GitError> {
        ref_under_refs(name)?;
        let mut command = self.git(root);
        command.args(["update-ref", "-d", name]);
        self.logged(command, None).map(drop)
    }

    fn write_stack_link(
        &self,
        root: &Path,
        branch: &str,
        link: Option<(&str, &str)>,
    ) -> Result<(), GitError> {
        if let Some(problem) = branch::name_problem(branch) {
            return Err(GitError(problem.into()));
        }
        let parent_key = format!("branch.{branch}.{}", stack::PARENT_KEY);
        let base_key = format!("branch.{branch}.{}", stack::BASE_KEY);
        match link {
            Some((parent, base)) => {
                for (key, value) in [(&parent_key, parent), (&base_key, base)] {
                    let mut command = self.git(root);
                    command.args(["config", "--local", key.as_str(), value]);
                    self.logged(command, None)?;
                }
            }
            None => {
                for key in [&parent_key, &base_key] {
                    let mut command = self.git(root);
                    command.args(["config", "--local", "--unset-all", key.as_str()]);
                    // Exit 5 is git's answer for a key that is not set.
                    let output = self.logged_output(command, None)?;
                    Self::checked(output, &[0, 5])?;
                }
            }
        }
        Ok(())
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

    fn conflict_operation(&self, root: &Path) -> Result<ConflictOperation, GitError> {
        self.operation_in_progress(root)
    }

    /// Merges the stages of `path` with `git merge-file`, so the text does not
    /// depend on `merge.conflictStyle`. It is built from the stages, not the
    /// working tree: edits already made in the file are not in it.
    fn read_conflict(&self, root: &Path, path: &str) -> Result<ConflictFileRead, GitError> {
        self.read_conflict_labelled(root, path, None)
    }

    /// The restack conflict is read as a rebase: the sides are the new parent `parent`
    /// on `onto` and the commit being replayed (D154, D22).
    fn read_restack_conflict(
        &self,
        worktree: &Path,
        path: &str,
        parent: &str,
        onto: &str,
        commit: &str,
    ) -> Result<ConflictFileRead, GitError> {
        require_commit_id(onto)?;
        require_commit_id(commit)?;
        let onto_side = SideCommit {
            branch: Some(parent.to_owned()),
            short: short_id(onto),
            subject: self.subject(worktree, onto)?,
        };
        let facts = SideFacts {
            operation: ConflictOperation::Rebase,
            head: onto_side.clone(),
            incoming: Some(SideCommit {
                branch: None,
                short: short_id(commit),
                subject: self.subject(worktree, commit)?,
            }),
            onto: Some(onto_side),
        };
        self.read_conflict_labelled(worktree, path, Some(facts))
    }

    /// Refuses text that still holds conflict markers, then writes it into the
    /// working tree file (atomically, keeping its mode) and stages it.
    fn resolve_conflict(&self, root: &Path, path: &str, text: &str) -> Result<(), GitError> {
        check_worktree_path(root, path)?;
        let entries = self.unmerged_entries(root, path)?;
        if entries.is_empty() {
            return Err(GitError(format!("{path} is already resolved")));
        }
        match conflict::parse(text) {
            Ok(doc) if doc.conflict_count() == 0 => {}
            Ok(doc) => {
                return Err(GitError(format!(
                    "{path} still has {} conflict(s) to resolve",
                    doc.conflict_count()
                )));
            }
            Err(error) => {
                return Err(GitError(format!(
                    "{path} still has a conflict marker ({error}); remove it before resolving"
                )));
            }
        }
        // Records the conflict before its markers are replaced, so the resolution
        // can be recorded below (rerere). A failure here fails nothing.
        let _ = self.rerere(root);
        let file = root.join(path);
        let mode = match file.metadata() {
            Ok(meta) => Some(meta.permissions()),
            Err(_) => entries
                .iter()
                .find(|entry| entry.stage == 2)
                .and_then(|entry| default_permissions(&entry.mode)),
        };
        let parent = file.parent().unwrap_or(root);
        let mut temp = tempfile::NamedTempFile::new_in(parent)
            .map_err(|e| GitError(format!("cannot write {path}: {e}")))?;
        temp.write_all(text.as_bytes())
            .and_then(|()| temp.as_file().sync_all())
            .map_err(|e| GitError(format!("cannot write {path}: {e}")))?;
        if let Some(mode) = mode {
            temp.as_file()
                .set_permissions(mode)
                .map_err(|e| GitError(format!("cannot write {path}: {e}")))?;
        }
        temp.persist(&file)
            .map_err(|e| GitError(format!("cannot write {path}: {}", e.error)))?;
        let mut command = self.git(root);
        command.args(["--literal-pathspecs", "add", "--", path]);
        self.logged(command, None)?;
        // Records the resolution for the next identical conflict (rerere). The file
        // is staged already, so a failure here does not fail the resolution: the
        // conflict then comes back unrecorded and is resolved from the start.
        let _ = self.rerere(root);
        Ok(())
    }

    /// Forgets the recorded resolution of `path` and restores its conflict markers.
    /// `rerere forget` drops the postimage; `checkout --merge` recreates the
    /// conflict from the stages. Both are logged.
    fn forget_recorded_resolution(&self, root: &Path, path: &str) -> Result<(), GitError> {
        check_worktree_path(root, path)?;
        if self.unmerged_entries(root, path)?.is_empty() {
            return Err(GitError(format!("{path} is not conflicted")));
        }
        // Applies a recorded resolution first, so the check sees the file as the
        // resolver would (rerere).
        self.rerere(root)?;
        if self.recorded_resolution(root, path)?.is_none() {
            return Err(GitError(format!("{path} has no recorded resolution")));
        }
        let mut command = self.git(root);
        command.args([
            "-c",
            "rerere.enabled=true",
            "--literal-pathspecs",
            "rerere",
            "forget",
            "--",
            path,
        ]);
        self.logged(command, None)?;
        let mut command = self.git(root);
        command.args(["--literal-pathspecs", "checkout", "--merge", "--", path]);
        self.logged(command, None).map(drop)
    }
}

impl SubprocessGit {
    /// Reads `path` for the resolver. The sides come from `facts` when given, else from the
    /// operation in progress at `root`.
    fn read_conflict_labelled(
        &self,
        root: &Path,
        path: &str,
        facts: Option<SideFacts>,
    ) -> Result<ConflictFileRead, GitError> {
        check_worktree_path(root, path)?;
        let entries = self.unmerged_entries(root, path)?;
        if entries.is_empty() {
            return Err(GitError(format!("{path} is not conflicted")));
        }
        let stage = |number: u8| entries.iter().find(|entry| entry.stage == number);
        let (Some(ours), Some(theirs)) = (stage(2), stage(3)) else {
            return Err(GitError(format!(
                "{path} is a conflict with a deleted side; the resolver handles only files edited on both sides"
            )));
        };
        let base = stage(1);
        let ours_content = self.blob(root, &ours.oid)?;
        let theirs_content = self.blob(root, &theirs.oid)?;
        let base_content = match base {
            Some(entry) => self.blob(root, &entry.oid)?,
            None => Vec::new(),
        };
        if [&ours_content, &theirs_content, &base_content]
            .iter()
            .any(|content| looks_binary(content))
        {
            return Err(GitError(format!(
                "{path} is binary; the resolver handles text files only"
            )));
        }
        // Records the conflicts not yet known and applies a recorded resolution to
        // the working tree, so the file below is read after that (rerere).
        self.rerere(root)?;
        let (operation, facts) = if let Some(facts) = facts {
            (ConflictOperation::Rebase, facts)
        } else {
            let operation = self.operation_in_progress(root)?;
            (operation, self.side_facts(root, operation)?)
        };
        let labels = conflict_sides::conflict_labels(&facts);
        let text = self.merge_with_markers(
            root,
            &base_content,
            &ours_content,
            &theirs_content,
            [&labels.left.branch, "base", &labels.right.branch],
        )?;
        let recorded = self.recorded_resolution(root, path)?;
        Ok(ConflictFileRead {
            path: path.to_owned(),
            operation,
            left: labels.left,
            right: labels.right,
            text,
            has_base: base.is_some(),
            recorded,
        })
    }

    /// `git rerere` with rerere on for this process only: the user's config is
    /// never written. Logged, since it can change the rerere cache and the working
    /// tree. It records the conflicts not yet known, records the resolution of the
    /// resolved ones and applies a recorded resolution to the conflicts it knows.
    fn rerere(&self, root: &Path) -> Result<(), GitError> {
        let mut command = self.git(root);
        command.args(["-c", "rerere.enabled=true", "rerere"]);
        self.logged(command, None).map(drop)
    }

    /// The text of the working tree file of `path` when it is a recorded
    /// resolution: it holds no conflict and is the postimage of a rerere entry.
    /// Git writes the postimage into the file when it applies a resolution, and the
    /// file stays unmerged in the index until it is staged. Never records anything.
    fn recorded_resolution(
        &self,
        root: &Path,
        path: &str,
    ) -> Result<Option<RecordedResolution>, GitError> {
        let bytes = match std::fs::read(root.join(path)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(GitError(format!("cannot read {path}: {error}"))),
        };
        let mut command = self.git(root);
        command.args(["rev-parse", "--git-path", "rr-cache"]);
        let out = String::from_utf8_lossy(&Self::run(command)?).into_owned();
        let cache = root.join(out.trim());
        let entries = match std::fs::read_dir(&cache) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(GitError(format!("cannot read the rerere cache: {error}"))),
        };
        let recorded = entries.flatten().any(|entry| {
            std::fs::read(entry.path().join("postimage")).is_ok_and(|postimage| postimage == bytes)
        });
        Ok(recorded
            .then(|| String::from_utf8(bytes).ok())
            .flatten()
            .map(|text| RecordedResolution { text }))
    }
}

/// One unmerged index entry of a path: the stage (1 is the base, 2 the current
/// side, 3 the other side), its mode and the id of its blob.
struct Unmerged {
    mode: String,
    oid: String,
    stage: u8,
}

impl SubprocessGit {
    /// The unmerged entries of `path`, none when it is not conflicted. Never
    /// records anything.
    fn unmerged_entries(&self, root: &Path, path: &str) -> Result<Vec<Unmerged>, GitError> {
        let mut command = self.git(root);
        command.args(["--literal-pathspecs", "ls-files", "-u", "-z", "--", path]);
        let out = Self::run(command)?;
        // One record per stage: `<mode> SP <object> SP <stage> TAB <path>`, then a NUL.
        let mut entries = Vec::new();
        for record in out.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
            let text = String::from_utf8_lossy(record);
            let Some((header, file)) = text.split_once('\t') else {
                return Err(GitError(format!("unexpected index record: {text}")));
            };
            if file != path {
                continue;
            }
            let mut fields = header.split(' ');
            let (Some(mode), Some(oid), Some(stage)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err(GitError(format!("unexpected index record: {text}")));
            };
            let stage = stage
                .parse::<u8>()
                .map_err(|_| GitError(format!("unexpected index record: {text}")))?;
            entries.push(Unmerged {
                mode: mode.to_owned(),
                oid: oid.to_owned(),
                stage,
            });
        }
        Ok(entries)
    }

    /// The content of blob `oid`. Never records anything.
    fn blob(&self, root: &Path, oid: &str) -> Result<Vec<u8>, GitError> {
        let mut command = self.git(root);
        command.args(["cat-file", "blob", oid]);
        Self::run(command)
    }

    /// The path of the git directory entry `name` (`MERGE_HEAD`, `rebase-merge`
    /// and so on) for the worktree at `root`. Per worktree for these names.
    fn git_path(&self, root: &Path, name: &str) -> Result<PathBuf, GitError> {
        let mut command = self.git(root);
        command.args(["rev-parse", "--path-format=absolute", "--git-path", name]);
        let out = Self::run(command)?;
        Ok(PathBuf::from(
            String::from_utf8_lossy(&out).trim_end_matches('\n'),
        ))
    }

    /// The trimmed text of the state file `name`, `None` when it is not there.
    fn state_text(&self, root: &Path, name: &str) -> Result<Option<String>, GitError> {
        let file = self.git_path(root, name)?;
        match std::fs::read_to_string(&file) {
            Ok(text) => Ok(Some(text.trim().to_owned())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(GitError(format!("cannot read {}: {e}", file.display()))),
        }
    }

    /// Which operation stopped. A rebase is recognised by its state folders, then
    /// merge, cherry-pick and revert by their `*_HEAD` files.
    fn operation_in_progress(&self, root: &Path) -> Result<ConflictOperation, GitError> {
        let present =
            |name: &str| -> Result<bool, GitError> { Ok(self.git_path(root, name)?.exists()) };
        Ok(if present("rebase-merge")? || present("rebase-apply")? {
            ConflictOperation::Rebase
        } else if present("MERGE_HEAD")? {
            ConflictOperation::Merge
        } else if present("CHERRY_PICK_HEAD")? {
            ConflictOperation::CherryPick
        } else if present("REVERT_HEAD")? {
            ConflictOperation::Revert
        } else {
            ConflictOperation::Unknown
        })
    }

    /// Reads what the labels need: HEAD, the commit the operation brings in, and
    /// the rebase's onto. Never records anything.
    fn side_facts(&self, root: &Path, operation: ConflictOperation) -> Result<SideFacts, GitError> {
        let head_oid = self.verify_rev(root, "HEAD")?.unwrap_or_default();
        let head = SideCommit {
            branch: self.current_branch(root)?,
            short: short_id(&head_oid),
            subject: self.subject(root, &head_oid)?,
        };
        let incoming_oid = match operation {
            ConflictOperation::Merge => self.state_text(root, "MERGE_HEAD")?,
            ConflictOperation::CherryPick => self.state_text(root, "CHERRY_PICK_HEAD")?,
            ConflictOperation::Revert => self.state_text(root, "REVERT_HEAD")?,
            ConflictOperation::Rebase => match self.state_text(root, "REBASE_HEAD")? {
                Some(oid) => Some(oid),
                None => match self.state_text(root, "rebase-merge/stopped-sha")? {
                    Some(oid) => Some(oid),
                    None => self.state_text(root, "rebase-apply/original-commit")?,
                },
            },
            ConflictOperation::Unknown => None,
        };
        let incoming = match incoming_oid {
            Some(oid) => Some(self.side_commit(root, &oid)?),
            None => None,
        };
        let onto = match operation {
            ConflictOperation::Rebase => match self.state_text(root, "rebase-merge/onto")? {
                Some(oid) => Some(self.side_commit(root, &oid)?),
                None => match self.state_text(root, "rebase-apply/onto")? {
                    Some(oid) => Some(self.side_commit(root, &oid)?),
                    None => None,
                },
            },
            _ => None,
        };
        Ok(SideFacts {
            operation,
            head,
            incoming,
            onto,
        })
    }

    /// A commit named by a state file: its branch when one names it (best
    /// effort), its short id and its subject.
    fn side_commit(&self, root: &Path, oid: &str) -> Result<SideCommit, GitError> {
        require_commit_id(oid)?;
        let mut command = self.git(root);
        command.args([
            "name-rev",
            "--name-only",
            "--refs=refs/heads/*",
            "--refs=refs/remotes/*",
            oid,
        ]);
        let named = String::from_utf8_lossy(&Self::run(command)?)
            .trim()
            .to_owned();
        // A commit below a branch tip is named `main~2`; the branch is what counts.
        let branch = named
            .split(['~', '^'])
            .next()
            .map(|name| {
                name.trim_start_matches("refs/heads/")
                    .trim_start_matches("refs/remotes/")
            })
            .filter(|name| !name.is_empty() && *name != "undefined")
            .map(str::to_owned);
        Ok(SideCommit {
            branch,
            short: short_id(oid),
            subject: self.subject(root, oid)?,
        })
    }

    /// The short name of the branch HEAD is on, `None` when detached. Never
    /// records anything.
    fn current_branch(&self, root: &Path) -> Result<Option<String>, GitError> {
        let mut command = self.git(root);
        command.args(["symbolic-ref", "-q", "--short", "HEAD"]);
        // Exit 1 is git's answer for a detached HEAD.
        let out = Self::run_allowing(command, &[0, 1])?;
        let name = String::from_utf8_lossy(&out).trim().to_owned();
        Ok((!name.is_empty()).then_some(name))
    }

    /// The subject line of commit `oid`, empty for an empty id.
    fn subject(&self, root: &Path, oid: &str) -> Result<String, GitError> {
        if oid.is_empty() {
            return Ok(String::new());
        }
        require_commit_id(oid)?;
        let mut command = self.git(root);
        command.args(["show", "-s", "--format=%s", oid]);
        Ok(String::from_utf8_lossy(&Self::run(command)?)
            .trim()
            .to_owned())
    }

    /// Merges the three contents with `git merge-file` in zdiff3 form, the labels
    /// given by `labels` (left, base, right). Exit codes 1 to 127 are the number
    /// of conflicts, which is the answer, not an error.
    fn merge_with_markers(
        &self,
        root: &Path,
        base: &[u8],
        ours: &[u8],
        theirs: &[u8],
        labels: [&str; 3],
    ) -> Result<String, GitError> {
        let scratch = self.scratch_dir(root)?;
        let write = |name: &str, content: &[u8]| -> Result<PathBuf, GitError> {
            let path = scratch.path().join(name);
            std::fs::write(&path, content)
                .map_err(|e| GitError(format!("cannot write {}: {e}", path.display())))?;
            Ok(path)
        };
        let base_path = write("base", base)?;
        let ours_path = write("ours", ours)?;
        let theirs_path = write("theirs", theirs)?;
        // Not logged: a computation that changes nothing in the repository.
        let mut command = self.git(root);
        command.args(["merge-file", "-p", "--zdiff3"]);
        for label in labels {
            command.args(["-L", label]);
        }
        command.arg(&ours_path).arg(&base_path).arg(&theirs_path);
        let output = command
            .output()
            .map_err(|e| GitError(format!("cannot run git: {e}")))?;
        match output.status.code() {
            Some(0..=127) => String::from_utf8(output.stdout)
                .map_err(|_| GitError("the conflicted file is not UTF-8 text".into())),
            _ => Err(GitError(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            )),
        }
    }
}

/// The first seven characters of a commit id, or the whole of a shorter one.
/// The snapshot of a commit object's text, as `cat-file` gives it: the author's name, the
/// committer's date in milliseconds and the subject. `None` when a header is missing.
/// The most commits a rewrite is compared over (D145). A longer range is refused, not cut.
const PATCH_RANGE_LIMIT: usize = 1000;

/// A commit id taken as a command argument: hex digits only, so no name reads as an option.
fn object_id_arg(oid: &str) -> Result<String, GitError> {
    if oid.len() < 4 || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitError(format!("not an object id: {oid:?}")));
    }
    Ok(oid.to_ascii_lowercase())
}

/// Whether the command exits 0. A check such as `merge-base --is-ancestor` exits 1 for "no", and
/// a missing object or an unborn HEAD exits 128: each is an answer here, not an error.
fn succeeds(mut command: Command) -> bool {
    command.output().is_ok_and(|output| output.status.success())
}

/// The patch id (`git patch-id --stable`) of each non-merge commit of `base..tip`, as
/// `(patch id, commit)` pairs (D145). The range is refused when it has more than
/// `PATCH_RANGE_LIMIT` commits.
fn patch_ids(
    git: &SubprocessGit,
    root: &Path,
    base: &str,
    tip: &str,
) -> Result<Vec<(String, String)>, GitError> {
    let range = format!("{base}..{tip}");
    let mut count = git.git(root);
    count.args(["rev-list", "--count", "--no-merges", range.as_str(), "--"]);
    let text = String::from_utf8_lossy(&SubprocessGit::run(count)?)
        .trim()
        .to_owned();
    let counted: usize = text
        .parse()
        .map_err(|_| GitError(format!("no commit count for {range}")))?;
    if counted > PATCH_RANGE_LIMIT {
        return Err(GitError(format!(
            "the range has {counted} commits, more than {PATCH_RANGE_LIMIT}"
        )));
    }
    let mut log = git.git(root);
    log.args([
        "log",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-show-signature",
        "--no-merges",
        "-p",
        range.as_str(),
        "--",
    ]);
    let input = SubprocessGit::run(log)?;
    let mut patch = git.git(root);
    patch
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = patch
        .spawn()
        .map_err(|e| GitError(format!("cannot run git: {e}")))?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err(GitError("git patch-id has no input".into()));
    };
    // The input is written on its own thread, so a full output pipe cannot stall the write.
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child
        .wait_with_output()
        .map_err(|e| GitError(format!("cannot run git: {e}")))?;
    let written = writer
        .join()
        .map_err(|_| GitError("the patch-id input thread failed".into()))?;
    let stdout = SubprocessGit::checked(output, &[0])?;
    written.map_err(|e| GitError(format!("cannot write to git patch-id: {e}")))?;
    let mut pairs = Vec::new();
    for line in String::from_utf8_lossy(&stdout).lines() {
        let mut fields = line.split_whitespace();
        if let (Some(patch), Some(commit)) = (fields.next(), fields.next()) {
            pairs.push((patch.to_owned(), commit.to_owned()));
        }
    }
    Ok(pairs)
}

fn parse_commit_snapshot(oid: &str, body: &[u8]) -> Option<CommitSnapshot> {
    let text = String::from_utf8_lossy(body);
    let (headers, message) = text.split_once("\n\n").unwrap_or((text.as_ref(), ""));
    let mut author = None;
    let mut committed_at = None;
    for line in headers.lines() {
        if let Some(rest) = line.strip_prefix("author ") {
            author = rest.rsplit_once(" <").map(|(name, _)| name.to_owned());
        } else if let Some(rest) = line.strip_prefix("committer ") {
            let tail = rest.rsplit_once('>').map(|(_, tail)| tail)?;
            let seconds: i64 = tail.split_whitespace().next()?.parse().ok()?;
            committed_at = Some(seconds.saturating_mul(1000));
        }
    }
    Some(CommitSnapshot {
        id: oid.to_owned(),
        subject: message.lines().next().unwrap_or("").to_owned(),
        author: author?,
        committed_at: committed_at?,
    })
}

fn short_id(oid: &str) -> String {
    oid.chars().take(7).collect()
}

/// The permissions a file of index `mode` gets when it is created: executable
/// for `100755`, `0644` for `100644`, none for anything else (a symlink, say).
/// `None` off Unix.
fn default_permissions(mode: &str) -> Option<std::fs::Permissions> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bits = match mode {
            "100755" => 0o755,
            "100644" => 0o644,
            _ => return None,
        };
        Some(std::fs::Permissions::from_mode(bits))
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        None
    }
}

/// The reason `discard_moved` gives when the file changed after the move started.
const MOVED_CHANGED: &str = "the file changed after the move started";

/// Parses `git merge-tree --write-tree --name-only -z` (D109). The first
/// NUL-terminated token is the merged tree. On a conflict the conflicted file
/// names follow, up to an empty token; the informational messages after that
/// empty token are not used.
fn parse_merge_tree(stdout: &[u8], conflicted: bool) -> Result<TreeMerge, GitError> {
    let mut tokens = stdout.split(|byte| *byte == 0);
    let tree = match tokens.next() {
        Some(tree) if !tree.is_empty() => String::from_utf8_lossy(tree).into_owned(),
        _ => return Err(GitError("merge-tree printed no tree".into())),
    };
    if !conflicted {
        return Ok(TreeMerge::Clean { tree });
    }
    let mut paths = Vec::new();
    for token in tokens {
        if token.is_empty() {
            return Ok(TreeMerge::Conflict { paths });
        }
        paths.push(String::from_utf8_lossy(token).into_owned());
    }
    Err(GitError(
        "merge-tree listed its conflicts without an end".into(),
    ))
}

/// Git's own rule for binary content: a NUL byte in the first 8000 bytes.
fn looks_binary(content: &[u8]) -> bool {
    content.iter().take(8000).any(|byte| *byte == 0)
}

/// Refuses a `path` that is not a plain relative path inside `root`, or that
/// goes through a symbolic link on the way (a folder or the file itself).
fn check_worktree_path(root: &Path, path: &str) -> Result<(), GitError> {
    let relative = Path::new(path);
    let plain = !path.is_empty()
        && relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if !plain {
        return Err(GitError(format!(
            "{path} is not a path in the working tree"
        )));
    }
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        if current
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            return Err(GitError(format!("{path} goes through a symbolic link")));
        }
    }
    Ok(())
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
/// The private ref that keeps the `side` snapshot of review comment `comment`
/// alive (D97, D140).
fn review_ref(comment: i64, side: ReviewSide) -> String {
    let side = match side {
        ReviewSide::Before => "before",
        ReviewSide::After => "after",
    };
    format!("refs/gitmaster/review/{comment}/{side}")
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

/// How many commits of the root `branch_landed` reads for patch ids (D138).
const LANDED_SCAN: usize = 2_000;

/// The tip of local branch `name`; `None` when there is no such branch.
fn local_tip(repo: &Repository, name: &str) -> Result<Option<Oid>, GitError> {
    match repo.find_branch(name, BranchType::Local) {
        Ok(branch) => branch
            .get()
            .peel_to_commit()
            .map(|commit| Some(commit.id()))
            .map_err(|e| git_err(&e)),
        Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
        Err(e) => Err(git_err(&e)),
    }
}

/// The merge base of two commits; `None` when they share no history.
fn optional_merge_base(repo: &Repository, one: Oid, two: Oid) -> Result<Option<Oid>, GitError> {
    match repo.merge_base(one, two) {
        Ok(oid) => Ok(Some(oid)),
        Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
        Err(e) => Err(git_err(&e)),
    }
}

/// Whether `commit` strictly descends from `ancestor`.
fn descends(repo: &Repository, commit: Oid, ancestor: Oid) -> Result<bool, GitError> {
    repo.graph_descendant_of(commit, ancestor)
        .map_err(|e| git_err(&e))
}

/// The commits reachable from `tip` and not from `stop`, newest first, at most `limit`.
fn commits_between(
    repo: &Repository,
    tip: Oid,
    stop: Oid,
    limit: usize,
) -> Result<Vec<Oid>, GitError> {
    let mut walk = repo.revwalk().map_err(|e| git_err(&e))?;
    walk.push(tip).map_err(|e| git_err(&e))?;
    walk.hide(stop).map_err(|e| git_err(&e))?;
    walk.take(limit)
        .map(|oid| oid.map_err(|e| git_err(&e)))
        .collect()
}

/// The patch id of what a commit changed against its parent, as
/// `git patch-id --stable` computes it; `None` for a merge commit.
fn commit_patch_id(repo: &Repository, oid: Oid) -> Result<Option<Oid>, GitError> {
    let commit = repo.find_commit(oid).map_err(|e| git_err(&e))?;
    match commit.parent_count() {
        0 => {
            let tree = commit.tree().map_err(|e| git_err(&e))?;
            let diff = repo
                .diff_tree_to_tree(None, Some(&tree), None)
                .map_err(|e| git_err(&e))?;
            diff.patchid(None).map(Some).map_err(|e| git_err(&e))
        }
        1 => {
            let parent = commit.parent_id(0).map_err(|e| git_err(&e))?;
            tree_patch_id(repo, parent, oid)
        }
        _ => Ok(None),
    }
}

/// The patch id of the diff from commit `from` to commit `to`; `None` when they
/// have the same tree.
fn tree_patch_id(repo: &Repository, from: Oid, to: Oid) -> Result<Option<Oid>, GitError> {
    let old = repo
        .find_commit(from)
        .and_then(|commit| commit.tree())
        .map_err(|e| git_err(&e))?;
    let new = repo
        .find_commit(to)
        .and_then(|commit| commit.tree())
        .map_err(|e| git_err(&e))?;
    if old.id() == new.id() {
        return Ok(None);
    }
    let diff = repo
        .diff_tree_to_tree(Some(&old), Some(&new), None)
        .map_err(|e| git_err(&e))?;
    diff.patchid(None).map(Some).map_err(|e| git_err(&e))
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
/// What a replay keeps of a commit (D142): its parents, author, dates and message.
struct ReplayMetadata {
    parents: Vec<String>,
    name: String,
    email: String,
    date: String,
    committed: String,
    message: String,
}

/// `path` canonicalized, or as given when it cannot be (a folder that is gone).
fn canonical_or_as_is(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A ref name under `refs/`, with no spaces or line breaks that would break `update-ref`.
fn ref_under_refs(name: &str) -> Result<(), GitError> {
    if !name.starts_with("refs/") || name.contains(['\n', '\0', ' ']) {
        return Err(GitError(format!("{name} is not a ref under refs/")));
    }
    Ok(())
}

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
    fn a_clean_merge_tree_answer_is_its_first_token() {
        assert_eq!(
            parse_merge_tree(b"b736e46\0", false),
            Ok(TreeMerge::Clean {
                tree: "b736e46".into()
            })
        );
    }

    #[test]
    fn a_conflicted_merge_tree_answer_lists_the_files_before_the_empty_token() {
        let raw = b"5356250\0f.txt\0g.txt\0\x001\0f.txt\0Auto-merging f.txt\n\0\
1\0f.txt\0CONFLICT (content)\0CONFLICT (content): Merge conflict in f.txt\n\0";
        assert_eq!(
            parse_merge_tree(raw, true),
            Ok(TreeMerge::Conflict {
                paths: vec!["f.txt".into(), "g.txt".into()]
            })
        );
    }

    #[test]
    fn a_merge_tree_answer_without_a_tree_or_an_end_is_an_error() {
        assert!(parse_merge_tree(b"", false).is_err());
        assert!(parse_merge_tree(b"\0", true).is_err());
        assert!(parse_merge_tree(b"tree\0f.txt", true).is_err());
    }

    #[test]
    fn only_a_nul_in_the_first_8000_bytes_makes_content_binary() {
        assert!(looks_binary(b"ab\0cd"));
        let mut late = vec![b'a'; 8000];
        late.push(0);
        assert!(!looks_binary(&late));
        assert!(looks_binary(&[b'a', 0]));
    }

    #[test]
    fn clean_email_gives_none_for_nothing() {
        assert_eq!(clean_email(""), None);
        assert_eq!(clean_email(" \n"), None);
        assert_eq!(clean_email("<>"), None);
        assert_eq!(clean_email("< >"), None);
    }
}

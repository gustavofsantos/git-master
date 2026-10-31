use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use gm_schema::{
    BranchEntry, CommitDetail, CommitFile, ConflictFileRead, ConflictOperation, DiffSide, FileDiff,
    GrepResult, HookScope, HunkId, MergedBranch, PruneCandidate, ReflogEntry, RepoStatus,
    StageTarget, StashEntry, WorktreeEntry, WorktreeMode,
};

use crate::command_log::CommandLog;
use crate::file_index::FileIndex;
use crate::grep::GrepRequest;
use crate::history::HistoryStream;
use crate::hook::ConfigValue;
use crate::restack::Replayed;
use crate::stack::{EdgeFacts, StackLink};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GitError {}

/// The read of one commit's detail, started by `GitPort::commit_detail`. It
/// runs on a thread of its own. Once `stop` is set the read is superseded:
/// it ends early and its answer is not wanted.
pub type DetailRead = Box<dyn FnOnce(&AtomicBool) -> Result<CommitDetail, GitError> + Send>;

/// The read of every file's diff in a commit, started by `GitPort::commit_stack`.
/// It stops the same way a `DetailRead` does (H4c).
pub type StackRead = Box<dyn FnOnce(&AtomicBool) -> Result<Vec<FileDiff>, GitError> + Send>;

/// The read of the worktree's file list, started by `GitPort::file_list`. It
/// runs on a thread of its own and stops like a `DetailRead` (N1).
pub type FileListRead = Box<dyn FnOnce(&AtomicBool) -> Result<FileIndex, GitError> + Send>;

/// The text search started by `GitPort::grep_text`. It runs on a thread of its
/// own, stops early at the result limit, and stops like a `DetailRead` once
/// `stop` is set.
pub type GrepRead = Box<dyn FnOnce(&AtomicBool) -> Result<GrepResult, GitError> + Send>;

/// What a history walk starts from (H6, D139): the engine resolves a scope to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryRevs {
    /// HEAD.
    Head,
    /// Every local branch, remote branch and tag, and HEAD.
    All,
    /// These refs, as Git reads them (full ref names where the engine knows them),
    /// and HEAD when `with_head` is set.
    Refs { refs: Vec<String>, with_head: bool },
}

/// A remote operation the adapter has checked: the Git run that is left. It
/// runs on a thread of its own, and its error is Git's own text.
pub type RemoteRun = Box<dyn FnOnce() -> Result<(), GitError> + Send>;

/// The result of merging two trees without a worktree (`merge_trees`): the
/// merged tree when everything merged, or the paths that conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeMerge {
    Clean { tree: String },
    Conflict { paths: Vec<String> },
}

/// The result of a three-way merge of one file's content (`merge_file`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileMerge {
    /// The merged content, with no conflict markers.
    Clean(Vec<u8>),
    /// The content has conflict markers; the merge did not finish.
    Conflict,
    /// One of the inputs looks binary, so it is not merged.
    Binary,
}

/// What `discard_moved` did to the source working copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscardOutcome {
    Discarded,
    /// The file was left alone; `reason` says why.
    Kept {
        reason: String,
    },
}

/// Which snapshot of a review comment a private ref keeps (D97, D140): the one
/// taken when the comment was made, or the one taken when an agent marked it
/// handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewSide {
    Before,
    After,
}

/// What the core needs from Git. Adapters implement it; the core tests with fakes.
pub trait GitPort: Send {
    /// The working tree root that contains `path`.
    fn resolve_root(&self, path: &Path) -> Result<std::path::PathBuf, GitError>;

    /// Never takes `index.lock`: an agent may be running `git` in the same repo.
    fn status(&self, root: &Path) -> Result<RepoStatus, GitError>;

    /// Diff of one path. An untracked path diffs as all-added.
    fn diff(&self, _root: &Path, _path: &str, _side: DiffSide) -> Result<FileDiff, GitError> {
        Err(GitError("diff is not supported by this adapter".into()))
    }

    /// Moves `target` into the index. A stale hunk id is an error, never a
    /// guess.
    fn stage(&self, _root: &Path, _target: &StageTarget) -> Result<(), GitError> {
        Err(GitError("stage is not supported by this adapter".into()))
    }

    /// Moves `target` out of the index (staged diff, HEAD against index).
    fn unstage(&self, _root: &Path, _target: &StageTarget) -> Result<(), GitError> {
        Err(GitError("unstage is not supported by this adapter".into()))
    }

    /// Reverts the unstaged hunk `hunk` of `path` in the working tree. The
    /// index is not touched. A stale id is an error, never a guess.
    fn discard_hunk(&self, _root: &Path, _path: &str, _hunk: &HunkId) -> Result<(), GitError> {
        Err(GitError("discard is not supported by this adapter".into()))
    }

    /// Discards every change of `path` (B1, D93). Without `include_staged` a
    /// tracked file returns to its index content and the staged side stays; with
    /// it, the file returns to HEAD in the index and the worktree. An untracked
    /// file is deleted. The loss is permanent. A path with no change is an error.
    fn discard_file(
        &self,
        _root: &Path,
        _path: &str,
        _include_staged: bool,
    ) -> Result<(), GitError> {
        Err(GitError("discard is not supported by this adapter".into()))
    }

    /// The stashes, newest first, as `git stash list` has them (B2, D94). Never
    /// records anything (D28).
    fn stashes(&self, _root: &Path) -> Result<Vec<StashEntry>, GitError> {
        Err(GitError("stashes are not supported by this adapter".into()))
    }

    /// The full id of the commit `stash@{index}` names, or `None` when there is
    /// no such stash. Never records anything (D28).
    fn resolve_stash(&self, _root: &Path, _index: u32) -> Result<Option<String>, GitError> {
        Err(GitError("stashes are not supported by this adapter".into()))
    }

    /// Stashes the local changes (`git stash push`), with `message` when given
    /// and the untracked files too when `include_untracked`. Nothing to stash is
    /// refused with Git's text.
    fn create_stash(
        &self,
        _root: &Path,
        _message: Option<&str>,
        _include_untracked: bool,
    ) -> Result<(), GitError> {
        Err(GitError("stash is not supported by this adapter".into()))
    }

    /// Applies `stash@{index}` without `--index`; the stash stays in the list. A
    /// conflict is Git's own text.
    fn apply_stash(&self, _root: &Path, _index: u32) -> Result<(), GitError> {
        Err(GitError("stash is not supported by this adapter".into()))
    }

    /// Applies `stash@{index}` and drops it when Git applied it cleanly. A
    /// conflict leaves the stash in the list, and Git's text is the error.
    fn pop_stash(&self, _root: &Path, _index: u32) -> Result<(), GitError> {
        Err(GitError("stash is not supported by this adapter".into()))
    }

    /// Drops `stash@{index}`; the stashes after it are renumbered.
    fn drop_stash(&self, _root: &Path, _index: u32) -> Result<(), GitError> {
        Err(GitError("stash is not supported by this adapter".into()))
    }

    /// Commits the index; returns the new commit id.
    fn commit(&self, _root: &Path, _message: &str) -> Result<String, GitError> {
        Err(GitError("commit is not supported by this adapter".into()))
    }

    /// Stages every change of the working tree (`git add -A`). Never commits.
    fn stage_all(&self, _root: &Path) -> Result<(), GitError> {
        Err(GitError(
            "stage all is not supported by this adapter".into(),
        ))
    }

    /// The common Git directory (`.git` of the main checkout), which linked
    /// worktrees share. Never records anything.
    fn common_dir(&self, _root: &Path) -> Result<PathBuf, GitError> {
        Err(GitError(
            "common dir is not supported by this adapter".into(),
        ))
    }

    /// Every value of `key` in the configuration the repository sees, in config
    /// order, following includes. Empty when the key is not set. Never records
    /// anything.
    fn config_values(&self, _root: &Path, _key: &str) -> Result<Vec<String>, GitError> {
        Err(GitError(
            "config values is not supported by this adapter".into(),
        ))
    }

    /// Local branches first, then remote ones, each alphabetically. Never
    /// records anything.
    fn branches(&self, _root: &Path) -> Result<Vec<BranchEntry>, GitError> {
        Err(GitError("branches is not supported by this adapter".into()))
    }

    /// The configured `user.email` without angle brackets; `None` when unset.
    /// Never records anything.
    fn user_email(&self, _root: &Path) -> Result<Option<String>, GitError> {
        Ok(None)
    }

    /// The worktrees of the repository, the main checkout first. Never
    /// records anything.
    fn worktrees(&self, _root: &Path) -> Result<Vec<WorktreeEntry>, GitError> {
        Err(GitError(
            "worktrees is not supported by this adapter".into(),
        ))
    }

    /// The walk of the commits reachable from `revs`, newest first in topological
    /// order, or `None` when HEAD is unborn and `revs` is HEAD. Starts a process
    /// and reads nothing yet. Never records anything (D28).
    fn history_stream(
        &self,
        _root: &Path,
        _revs: &HistoryRevs,
    ) -> Result<Option<Box<dyn HistoryStream>>, GitError> {
        Err(GitError("history is not supported by this adapter".into()))
    }

    /// Starts the read of commit `oid` (a full id): its header and its files
    /// against the first parent. Validates the id and starts nothing else; the
    /// returned read does the work. Never records anything (D28).
    fn commit_detail(&self, _root: &Path, _oid: &str) -> Result<DetailRead, GitError> {
        Err(GitError(
            "commit detail is not supported by this adapter".into(),
        ))
    }

    /// Starts the read of the file list of the worktree at `root`: the tracked
    /// files, and the untracked ones `.gitignore` does not hide, minus what a
    /// sparse checkout leaves out. Starts nothing but the returned read. Never
    /// records anything (D28).
    fn file_list(&self, _root: &Path) -> Result<FileListRead, GitError> {
        Err(GitError(
            "the file list is not supported by this adapter".into(),
        ))
    }

    /// Starts a text search of the worktree with `git grep` (N3). Starts nothing
    /// but the returned read. Git is asked for no more than the result limits
    /// need. Never records anything (D28).
    fn grep_text(&self, _root: &Path, _request: &GrepRequest) -> Result<GrepRead, GitError> {
        Err(GitError(
            "text search is not supported by this adapter".into(),
        ))
    }

    /// Starts the read of the diffs of the files of commit `oid` against its
    /// first parent (the root commit against nothing), in one `git show`. The
    /// answer has one diff per entry of `files`, in its order. Validates the id
    /// and starts nothing else. Never records anything (D28).
    fn commit_stack(
        &self,
        _root: &Path,
        _oid: &str,
        _files: &[CommitFile],
    ) -> Result<StackRead, GitError> {
        Err(GitError(
            "commit diffs are not supported by this adapter".into(),
        ))
    }

    /// Creates a linked worktree at `path` (relative paths are taken from
    /// `root`) that checks out `mode`. Refuses a path that already exists.
    /// Git's own refusal is the error.
    fn create_worktree(
        &self,
        _root: &Path,
        _path: &str,
        _mode: &WorktreeMode,
    ) -> Result<(), GitError> {
        Err(GitError(
            "create worktree is not supported by this adapter".into(),
        ))
    }

    /// Commits of HEAD in the worktree at `path` that no other local branch or
    /// remote has, or that are ahead of the upstream: whichever count is higher.
    /// `branch` is the worktree's branch, `None` when detached. Zero on an unborn
    /// HEAD. Never records anything.
    fn unpushed_commits(&self, _path: &Path, _branch: Option<&str>) -> Result<u32, GitError> {
        Err(GitError(
            "unpushed commits is not supported by this adapter".into(),
        ))
    }

    /// Removes the linked worktree at `path` and its folder. `force` is 0 (Git
    /// refuses changes), 1 (`--force`) or 2 (`--force --force`, which also
    /// removes a locked worktree). Git's own refusal is the error.
    fn remove_worktree(&self, _root: &Path, _path: &str, _force: u8) -> Result<(), GitError> {
        Err(GitError(
            "remove worktree is not supported by this adapter".into(),
        ))
    }

    /// Locks the worktree at `path`; an empty `reason` locks it without one.
    fn lock_worktree(&self, _root: &Path, _path: &str, _reason: &str) -> Result<(), GitError> {
        Err(GitError(
            "lock worktree is not supported by this adapter".into(),
        ))
    }

    /// Unlocks the worktree at `path`.
    fn unlock_worktree(&self, _root: &Path, _path: &str) -> Result<(), GitError> {
        Err(GitError(
            "unlock worktree is not supported by this adapter".into(),
        ))
    }

    /// The admin records `git worktree prune` would remove, from its dry run.
    /// Never records anything.
    fn prunable(&self, _root: &Path) -> Result<Vec<PruneCandidate>, GitError> {
        Err(GitError("prunable is not supported by this adapter".into()))
    }

    /// Removes the admin records of the worktrees whose folders are gone.
    fn prune_worktrees(&self, _root: &Path) -> Result<(), GitError> {
        Err(GitError(
            "prune worktrees is not supported by this adapter".into(),
        ))
    }

    /// Switches HEAD to `branch`. A remote-only branch gets a local tracking
    /// branch of the same short name. Git's own refusal is the error.
    fn checkout(&self, _root: &Path, _branch: &str) -> Result<(), GitError> {
        Err(GitError("checkout is not supported by this adapter".into()))
    }

    /// Detaches HEAD at commit `oid` (`git checkout --detach`), B3 (D95). The
    /// id is checked first; Git's own refusal is the error.
    fn checkout_commit(&self, _root: &Path, _oid: &str) -> Result<(), GitError> {
        Err(GitError("checkout is not supported by this adapter".into()))
    }

    /// The entries of the reflog of HEAD after the `skip` newest, at most
    /// `limit`, newest first (B3, D95). Empty for an unborn HEAD. Never records
    /// anything (D28) and never takes `index.lock`.
    fn reflog(
        &self,
        _root: &Path,
        _skip: u32,
        _limit: usize,
    ) -> Result<Vec<ReflogEntry>, GitError> {
        Err(GitError(
            "the reflog is not supported by this adapter".into(),
        ))
    }

    /// Builds a commit of the whole working tree for a review comment (B4, D97):
    /// untracked files are in it, ignored ones are not. It works on a copy of
    /// the index, so the real index, HEAD, the working tree and every public ref
    /// stay as they are. Returns the commit id; nothing points at it yet.
    fn snapshot_worktree(&self, _root: &Path) -> Result<String, GitError> {
        Err(GitError(
            "snapshots are not supported by this adapter".into(),
        ))
    }

    /// Builds a commit that is HEAD with only `paths` taken from the working
    /// tree (`add -A` on those paths alone). It is the move snapshot of D109:
    /// the other paths are HEAD's, whatever the index holds. The real index,
    /// HEAD, the working tree and every public ref stay as they are. Returns the
    /// commit id; nothing points at it yet. Not recorded in the command log: it
    /// writes objects only.
    fn snapshot_paths(&self, _root: &Path, _paths: &[String]) -> Result<String, GitError> {
        Err(GitError(
            "snapshots are not supported by this adapter".into(),
        ))
    }

    /// The version of the `git` the app runs, as `git --version` prints it.
    /// Never records anything.
    fn git_version(&self) -> Result<String, GitError> {
        Err(GitError(
            "git version is not supported by this adapter".into(),
        ))
    }

    /// Merges `ours` and `theirs` over `base` into a tree, without touching the
    /// index or the working tree (`git merge-tree --write-tree`, Git 2.38 or
    /// later). It writes objects but records no command. Never takes
    /// `index.lock`.
    fn merge_trees(
        &self,
        _root: &Path,
        _base: &str,
        _ours: &str,
        _theirs: &str,
    ) -> Result<TreeMerge, GitError> {
        Err(GitError(
            "tree merge is not supported by this adapter".into(),
        ))
    }

    /// Creates a commit of `tree` with `parent` (none for a root commit) and
    /// `message`, under the user's own identity. Git's refusal, for lack of an
    /// identity for one, is the error. Returns the commit id.
    fn commit_tree(
        &self,
        _root: &Path,
        _tree: &str,
        _parent: Option<&str>,
        _message: &str,
    ) -> Result<String, GitError> {
        Err(GitError(
            "commit tree is not supported by this adapter".into(),
        ))
    }

    /// Points the full ref `name` at `new`, if it is at `old`. `old` of `None`
    /// means the ref must not exist yet. `name` must start with `refs/`. Git's
    /// refusal is the error.
    fn update_ref(
        &self,
        _root: &Path,
        _name: &str,
        _new: &str,
        _old: Option<&str>,
    ) -> Result<(), GitError> {
        Err(GitError(
            "update ref is not supported by this adapter".into(),
        ))
    }

    /// Three-way merge of one file's content with `git merge-file`. Content with
    /// a NUL byte near its start is `Binary` and git is not run. Never records
    /// anything: it is a computation.
    fn merge_file(
        &self,
        _root: &Path,
        _base: &[u8],
        _ours: &[u8],
        _theirs: &[u8],
    ) -> Result<FileMerge, GitError> {
        Err(GitError(
            "file merge is not supported by this adapter".into(),
        ))
    }

    /// Discards the change of `path` in the working copy, but only when it still
    /// matches `snapshot` (the move snapshot of D109): the blob of `path` in the
    /// snapshot, or no file in both. Otherwise nothing changes and the outcome
    /// says why. A symlink or a path outside the working tree is an error.
    fn discard_moved(
        &self,
        _root: &Path,
        _path: &str,
        _snapshot: &str,
    ) -> Result<DiscardOutcome, GitError> {
        Err(GitError(
            "discard moved is not supported by this adapter".into(),
        ))
    }

    /// Points the private ref that keeps comment `comment`'s `side` snapshot
    /// alive at `oid` (B4, D97, D140).
    fn set_review_ref(
        &self,
        _root: &Path,
        _comment: i64,
        _side: ReviewSide,
        _oid: &str,
    ) -> Result<(), GitError> {
        Err(GitError(
            "snapshots are not supported by this adapter".into(),
        ))
    }

    /// Deletes the private ref of comment `comment`'s `side` snapshot, so the
    /// snapshot can go.
    fn delete_review_ref(
        &self,
        _root: &Path,
        _comment: i64,
        _side: ReviewSide,
    ) -> Result<(), GitError> {
        Err(GitError(
            "snapshots are not supported by this adapter".into(),
        ))
    }

    /// The commit id that `rev` names, such as `HEAD` or `refs/heads/main`; an
    /// error when it names no commit. Never records anything.
    fn resolve_commit(&self, _root: &Path, _rev: &str) -> Result<String, GitError> {
        Err(GitError(
            "resolve commit is not supported by this adapter".into(),
        ))
    }

    /// The content of `path` in commit `oid`, or `None` when the commit has no
    /// such file. Never records anything (D28).
    fn file_at_commit(
        &self,
        _root: &Path,
        _oid: &str,
        _path: &str,
    ) -> Result<Option<Vec<u8>>, GitError> {
        Err(GitError(
            "snapshots are not supported by this adapter".into(),
        ))
    }

    /// The zero-context diff of `path` from commit `from` to commit `to`
    /// (D140): every hunk with no context lines, so its header says exactly
    /// which lines changed. A path in neither commit, or the same in both, has
    /// no hunks. Never records anything (D28).
    fn diff_commits(
        &self,
        _root: &Path,
        _from: &str,
        _to: &str,
        _path: &str,
    ) -> Result<FileDiff, GitError> {
        Err(GitError(
            "snapshots are not supported by this adapter".into(),
        ))
    }

    /// Creates the local branch `name` at `start_point` (HEAD when `None`), and
    /// checks it out when `checkout` is set. Git's own refusal is the error.
    fn create_branch(
        &self,
        _root: &Path,
        _name: &str,
        _start_point: Option<&str>,
        _checkout: bool,
    ) -> Result<(), GitError> {
        Err(GitError(
            "create branch is not supported by this adapter".into(),
        ))
    }

    /// Deletes the local branch `name`: `git branch -d`, or `-D` when `force`.
    /// Git's own refusal is the error.
    fn delete_branch(&self, _root: &Path, _name: &str, _force: bool) -> Result<(), GitError> {
        Err(GitError(
            "delete branch is not supported by this adapter".into(),
        ))
    }

    /// Every stack link of the repository's own config (D13): each
    /// `branch.<name>.stackParent`, with its `stackParentBase`. Never records
    /// anything.
    fn stack_links(&self, _root: &Path) -> Result<Vec<StackLink>, GitError> {
        Err(GitError("stacks are not supported by this adapter".into()))
    }

    /// The graph facts of the link from local branch `branch` to its existing
    /// local parent `parent`, with the recorded `base` (D138). Never records anything.
    fn stack_edge(
        &self,
        _root: &Path,
        _branch: &str,
        _parent: &str,
        _base: Option<&str>,
    ) -> Result<EdgeFacts, GitError> {
        Err(GitError("stacks are not supported by this adapter".into()))
    }

    /// Whether the changes of local branch `branch`, whose own commits start at
    /// `own_base`, are already in local branch `into`: its tip is reachable from
    /// `into` and it has commits of its own, or its own commits or its whole diff
    /// match commits of `into` by patch id (D138). Never records anything.
    fn branch_landed(
        &self,
        _root: &Path,
        _branch: &str,
        _own_base: Option<&str>,
        _into: &str,
    ) -> Result<bool, GitError> {
        Err(GitError("stacks are not supported by this adapter".into()))
    }

    /// The full id of the tip of local branch `name`; `None` when there is no such
    /// branch. Never records anything.
    fn branch_tip(&self, _root: &Path, _name: &str) -> Result<Option<String>, GitError> {
        Err(GitError("stacks are not supported by this adapter".into()))
    }

    /// The merge base of local branches `one` and `two`; `None` when they share
    /// no history. Never records anything.
    fn branch_merge_base(
        &self,
        _root: &Path,
        _one: &str,
        _two: &str,
    ) -> Result<Option<String>, GitError> {
        Err(GitError("stacks are not supported by this adapter".into()))
    }

    /// Replays `commit` onto `onto` the way a restack will (D142): a three-way
    /// merge of the commit's change onto `onto`, and a new commit with the same
    /// message, author and author date when it is clean. The new commit is not
    /// referenced by any ref. Never recorded in the command log.
    fn restack_replay(
        &self,
        _root: &Path,
        _onto: &str,
        _commit: &str,
    ) -> Result<Replayed, GitError> {
        Err(GitError("restack is not supported by this adapter".into()))
    }

    /// Writes the stack link of `branch` as `(parent, base)` with `git config`,
    /// or removes both keys for `None`. Recorded in the command log.
    fn write_stack_link(
        &self,
        _root: &Path,
        _branch: &str,
        _link: Option<(&str, &str)>,
    ) -> Result<(), GitError> {
        Err(GitError("stacks are not supported by this adapter".into()))
    }

    /// The branch `refs/remotes/origin/HEAD` names, as `git symbolic-ref --short`
    /// prints it (`origin/main`); `None` when that ref is not there. Never records
    /// anything.
    fn origin_head(&self, _root: &Path) -> Result<Option<String>, GitError> {
        Ok(None)
    }

    /// The local branches `git branch --merged <base>` lists, with their tips.
    /// Never records anything.
    fn merged_branches(&self, _root: &Path, _base: &str) -> Result<Vec<MergedBranch>, GitError> {
        Err(GitError(
            "merged branches is not supported by this adapter".into(),
        ))
    }

    /// Checks a `git fetch --all --prune` of the repository at `root` and returns
    /// the run that does it. Starts no process itself. Human only (the engine checks).
    fn fetch(&self, _root: &Path) -> Result<RemoteRun, GitError> {
        Err(GitError("fetch is not supported by this adapter".into()))
    }

    /// Checks a `git pull --ff-only` of the current branch and returns the run.
    /// Refuses a detached HEAD and a branch without an upstream, before Git runs.
    fn pull(&self, _root: &Path) -> Result<RemoteRun, GitError> {
        Err(GitError("pull is not supported by this adapter".into()))
    }

    /// Checks a `git push --force-with-lease` of the current branch alone and
    /// returns the run. Refuses a detached HEAD, and a branch with no upstream
    /// when there is no remote or more than one.
    fn push_with_lease(&self, _root: &Path) -> Result<RemoteRun, GitError> {
        Err(GitError(
            "push with lease is not supported by this adapter".into(),
        ))
    }

    /// The `gitmaster.prepare-commit.*` keys of one scope, in config order.
    /// `User` is global config only; `Repo` is the repository's own config.
    /// Never records anything.
    fn hook_config(&self, _root: &Path, _scope: HookScope) -> Result<Vec<ConfigValue>, GitError> {
        Err(GitError(
            "hook config is not supported by this adapter".into(),
        ))
    }

    /// The staged diff, as `git diff --cached` prints it. Never records anything.
    fn staged_diff(&self, _root: &Path) -> Result<String, GitError> {
        Err(GitError(
            "staged diff is not supported by this adapter".into(),
        ))
    }

    /// The operation that stopped on conflicts (D114), from the state files Git
    /// keeps in the git directory of the worktree at `root`. `Unknown` when none
    /// is there. Never records anything.
    fn conflict_operation(&self, _root: &Path) -> Result<ConflictOperation, GitError> {
        Err(GitError(
            "conflict resolution is not supported by this adapter".into(),
        ))
    }

    /// Reads the conflicted file `path` for the resolver: its three stages merged
    /// into zdiff3 text, with the sides named by role. Runs rerere, which records
    /// new conflicts and applies a recorded resolution to the working tree file:
    /// that is the only change it makes, and the answer says whether there is one.
    fn read_conflict(&self, _root: &Path, _path: &str) -> Result<ConflictFileRead, GitError> {
        Err(GitError(
            "conflict resolution is not supported by this adapter".into(),
        ))
    }

    /// Writes `text` as the content of the conflicted file `path` and stages it.
    /// Refused while `text` still holds conflict markers. Recorded in the command
    /// log: it changes the index.
    fn resolve_conflict(&self, _root: &Path, _path: &str, _text: &str) -> Result<(), GitError> {
        Err(GitError(
            "conflict resolution is not supported by this adapter".into(),
        ))
    }

    /// Forgets the resolution Git recorded for the conflicted file `path` (rerere)
    /// and restores its conflict markers. Refused when nothing is recorded. Recorded
    /// in the command log: it changes the working tree and the rerere cache.
    fn forget_recorded_resolution(&self, _root: &Path, _path: &str) -> Result<(), GitError> {
        Err(GitError(
            "conflict resolution is not supported by this adapter".into(),
        ))
    }

    /// The login-shell PATH, which git and the hook scripts run with. Empty
    /// until it is read; an adapter without one keeps the process's PATH.
    fn login_path(&self) -> Arc<OnceLock<String>> {
        Arc::default()
    }

    /// The log that hook steps and git writes are recorded in, when the
    /// adapter keeps one. Reads are never recorded.
    fn command_log(&self) -> Option<CommandLog> {
        None
    }
}

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::conflict::ConflictFileRead;
use crate::diff::{DiffSide, FileDiff, HunkId, StageTarget};
use crate::files::{DirListing, FileSearch, FileView};
use crate::grep::{GrepOptions, GrepResult};
use crate::history::{CommitDetail, CommitFile, History};
use crate::hook::{HookFailure, HookInfo};
use crate::move_changes::{MoveOutcome, MovePreview, MoveTarget};
use crate::reflog::ReflogEntry;
use crate::review::ReviewComment;
use crate::stash::StashEntry;
use crate::status::{
    BranchEntry, BranchFailure, MergedBranch, PruneCandidate, RepoStatus, WorktreeEntry,
    WorktreeInspection,
};
use crate::syntax::HighlightSpan;
use crate::workflow::{
    WorkflowDiagnostic, WorkflowDryRun, WorkflowEnd, WorkflowInfo, WorkflowRunDetail,
    WorkflowRunStatus, WorkflowRunSummary,
};

/// The refusal of `DeleteMergedBranches` when the merged list no longer matches
/// the one the human confirmed. The UI matches it to list the branches again.
pub const MERGED_LIST_CHANGED: &str =
    "the merged branches changed since they were listed; list them again";

/// The refusal of an apply, pop or drop whose `stash@{index}` no longer names the
/// stash the human picked. The UI matches it to list the stashes again.
pub const STASH_LIST_CHANGED: &str = "the stashes changed since they were listed; list them again";

/// Sequential, so the operation log is deterministic.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct OperationId(pub u64);

/// Who asked. A human skips approval; an agent does not (once there is
/// anything to approve).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Actor {
    Human,
    Agent { name: String },
}

/// What a new worktree checks out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorktreeMode {
    /// An existing local branch. Git refuses one that another worktree has.
    Existing { branch: String },
    /// A new branch `branch`, created at `base` (any commit-ish).
    New { branch: String, base: String },
    /// A detached HEAD at `commit` (any commit-ish); no branch is created.
    Detached { commit: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandKind {
    OpenRepo {
        path: String,
    },
    RefreshStatus,
    LoadDiff {
        path: String,
        side: DiffSide,
    },
    Stage {
        target: StageTarget,
    },
    Unstage {
        target: StageTarget,
    },
    /// Reverts one unstaged hunk in the working tree. Destructive: the
    /// hunk's changes are lost, and the id must name a current unstaged hunk.
    DiscardHunk {
        path: String,
        hunk: HunkId,
    },
    /// Discards every change of `path` (B1, D93). Destructive: the loss is
    /// permanent, with no snapshot to undo it. Without `include_staged` a
    /// tracked file goes back to its index content and what is staged stays;
    /// with it, the file goes back to HEAD in the index and the worktree, and
    /// an added file is removed. An untracked file is deleted either way. Human
    /// only, and the command is logged (D28).
    DiscardFile {
        path: String,
        include_staged: bool,
    },
    /// Stages every change of the working tree (`git add -A`), as a workflow's
    /// `stage.all` does. Never commits.
    StageAll,
    Commit {
        message: String,
    },
    ListBranches,
    /// Reads the first `HISTORY_PAGE` commits of HEAD, newest first. Never
    /// recorded in the command log (D28).
    LoadHistory,
    /// Reads the next `HISTORY_PAGE` commits of HEAD after `after`, the last
    /// commit the list holds. Never recorded in the command log (D28).
    LoadHistoryPage {
        after: String,
    },
    /// Reads what commit `oid` changed: its header and its files, against
    /// the first parent. Never recorded in the command log (D28). A request
    /// in flight is superseded by the next one.
    LoadCommitDetail {
        oid: String,
    },
    /// Reads the diffs of the files of commit `oid` (H4c), against its first
    /// parent: one patch for the whole commit, cut into the listed files in
    /// their order. Never recorded in the command log (D28). A request in flight
    /// is superseded by the next one.
    LoadCommitStack {
        oid: String,
        files: Vec<CommitFile>,
    },
    /// Reads `REFLOG_PAGE` entries of the reflog of HEAD (B3, D95), newest
    /// first, after the `skip` newest ones. Never recorded in the command log
    /// (D28). An unborn HEAD or a repository without a reflog is an empty list.
    LoadReflog {
        skip: u32,
    },
    /// Reads the file list of the worktree again (N1): `git ls-files` with the
    /// untracked files `.gitignore` does not hide, minus what a sparse checkout
    /// leaves out. The list the last read made keeps answering until the new one
    /// is ready, then `FilesIndexed` follows. Never recorded in the command log
    /// (D28). Quick Open and the Code page ask for it when they open.
    IndexFiles,
    /// Finds the files whose path the `query` matches (N1), best first, over
    /// the file list in memory. A `:line` suffix is parsed, not matched. Never
    /// recorded in the command log (D28). A request in flight is superseded by
    /// the next one.
    SearchFiles {
        query: String,
    },
    /// Lists the entries of folder `dir` (`""` for the root) from the file list
    /// in memory, for the file tree (N2). Never recorded in the command log (D28).
    ListDirectory {
        dir: String,
    },
    /// Searches the text of the files of the worktree (N3) with `git grep`:
    /// tracked files and untracked ones `.gitignore` does not hide, never
    /// binaries. `paths` is a list of pathspecs separated by spaces; a word
    /// starting with `!` excludes. Stops at `GREP_TOTAL_LIMIT` lines. Never
    /// recorded in the command log (D28). A request in flight is superseded by
    /// the next one.
    SearchText {
        query: String,
        options: GrepOptions,
        paths: String,
    },
    /// Reads the file `path` of the worktree from disk for the viewer (N2). Never
    /// recorded in the command log (D28). A request in flight is superseded by
    /// the next one.
    ReadFile {
        path: String,
    },
    /// A local branch, or a remote branch such as `origin/feature` (a local
    /// tracking branch is created for it when none exists).
    Checkout {
        branch: String,
    },
    /// Creates the local branch `name` at `start_point` (HEAD when `None`), and
    /// checks it out when `checkout` is set (`git checkout -b`). Git refuses a
    /// name that exists; its message is the error. Human only.
    CreateBranch {
        name: String,
        start_point: Option<String>,
        checkout: bool,
    },
    /// Detaches HEAD at commit `oid` (`git checkout --detach`), as a reflog entry
    /// is checked out (B3, D95). The id is 7 to 64 hex characters; Git's text is
    /// the error, such as local changes that would be overwritten. Human only.
    CheckoutCommit {
        oid: String,
    },
    /// Deletes the local branch `name`: `git branch -d`, or `-D` when `force`.
    /// Refused while a worktree has the branch checked out. Git refuses an
    /// unmerged branch under `-d`, and its message is the error. Human only.
    DeleteBranch {
        name: String,
        force: bool,
    },
    /// `git fetch --all --prune`: every remote is fetched, and the remote-tracking
    /// branches a remote has deleted are removed. Human only. Runs off the
    /// engine thread; the status and the branches follow.
    Fetch,
    /// `git pull --ff-only` on the current branch, against its upstream. It only
    /// fast-forwards, so it never makes a merge commit or a rebase. Human only.
    Pull,
    /// `git push --force-with-lease` of the current branch alone: to its upstream
    /// when it has one, otherwise to the only remote, which becomes its upstream.
    /// Never pushes tags or other branches. Human only.
    PushWithLease,
    /// Lists the worktrees of the repository, the main checkout first. Never
    /// records anything.
    ListWorktrees,
    /// Creates a linked worktree at `path`, which must not exist yet. Human
    /// only, for now (D31: no agent path for worktrees).
    CreateWorktree {
        path: String,
        mode: WorktreeMode,
    },
    /// Reads what removing the worktree at `path` would lose or refuse. Never
    /// changes anything, so it is not an operation the log records as a change.
    InspectWorktree {
        path: String,
    },
    /// Reads what moving the Working Copy change of `path` to `target` would
    /// refuse (D109). Never changes anything visible and is not recorded in the
    /// command log. Answered by `MovePreviewed`.
    PreviewMoveChanges {
        path: String,
        target: MoveTarget,
    },
    /// Moves the Working Copy change of `path` to `target` (D109). Human only,
    /// and recorded in the command log. A branch target needs `message`, the
    /// commit message; a worktree target ignores it. Answered by `ChangesMoved`.
    MoveChanges {
        path: String,
        target: MoveTarget,
        message: Option<String>,
    },
    /// Removes the linked worktree at `path` with `git worktree remove`; the
    /// folder is deleted, never by hand. Human only (D31). Refused while a hard
    /// blocker applies (main, the window's own worktree, a missing folder). A
    /// dirty, unpushed or locked worktree is refused unless `force` is set: the
    /// human ticked the confirmation that the listed changes go.
    RemoveWorktree {
        path: String,
        force: bool,
    },
    /// Locks the worktree at `path` with `git worktree lock`; an empty `reason`
    /// locks it without one. Human only.
    LockWorktree {
        path: String,
        reason: String,
    },
    /// Unlocks the worktree at `path`. Human only.
    UnlockWorktree {
        path: String,
    },
    /// Lists the admin records `git worktree prune` would remove, without
    /// removing them. Never changes anything.
    ListPrunable,
    /// Removes the admin records named in `names`, the ones the last
    /// `PrunableListed` showed. Refused when the list has changed since.
    /// Human only.
    PruneWorktrees {
        names: Vec<String>,
    },
    /// Lists the local branches merged into the base branch, without deleting
    /// anything. The base is `origin/HEAD`'s branch, else the first of `main`,
    /// `master`, `trunk`, `develop` that exists, else the current branch. Never
    /// records anything in the command log. Remote branches are not listed.
    ListMergedBranches,
    /// Deletes the local branches `branches`, which the last `MergedBranchesListed`
    /// showed: each with `git branch -d`, never `-D`. Refused when the merged list
    /// has changed since. A branch Git refuses is reported, and the others still go.
    /// Human only.
    DeleteMergedBranches {
        branches: Vec<String>,
    },
    /// Lists the stashes, newest first (B2, D94). Answers `StashesListed`. A
    /// read: never recorded in the command log (D28).
    ListStashes,
    /// Stashes the local changes (`git stash push`), with `message` when it is
    /// not blank, and the untracked files too when `include_untracked` is set.
    /// Refused, with Git's text, when there is nothing to stash. Human only.
    CreateStash {
        message: Option<String>,
        include_untracked: bool,
    },
    /// Applies `stash@{index}` without `--index`, which must still be the stash
    /// `oid` the last `StashesListed` showed; otherwise `STASH_LIST_CHANGED`. The
    /// stash stays in the list. A conflict is Git's error. Human only.
    ApplyStash {
        index: u32,
        oid: String,
    },
    /// Applies `stash@{index}` (as `ApplyStash`) and drops it when Git applied it
    /// cleanly. A conflict keeps the stash in the list. Human only.
    PopStash {
        index: u32,
        oid: String,
    },
    /// Drops `stash@{index}`, the stash `oid` the last `StashesListed` showed.
    /// Human only.
    DropStash {
        index: u32,
        oid: String,
    },
    /// Lists the hooks read from config, with their state (design/hooks-transitional.md).
    ListHooks,
    /// Approves the repository hook `name` whose command still hashes to
    /// `command_hash`, as the last `HooksListed` showed it.
    ApproveHook {
        name: String,
        command_hash: String,
    },
    /// Runs the `prepare-commit` chain on `message`. Asynchronous: the hook
    /// events for the run carry the run's own operation id.
    RunPrepareCommit {
        message: String,
    },
    /// Stops the `RunPrepareCommit` operation `run`, if it is still running.
    CancelHook {
        run: OperationId,
    },
    /// Lists the workflow files of the user's directory and of every
    /// `gitmaster.workflowPath`, with what loading each gave. Reads only; never
    /// recorded in the command log (D28).
    ListWorkflows,
    /// Starts the manual workflow `name` with `inputs`, one string per declared
    /// input. Human only. The run goes on with its own events, keyed by its run id.
    RunWorkflow {
        name: String,
        inputs: BTreeMap<String, String>,
    },
    /// The human's answer to the `approve` state run `run` is in. Human only.
    ApproveWorkflowStep {
        run: i64,
        approved: bool,
    },
    /// Stops the workflow run `run`, killing its `exec` step if one is running.
    /// Human only.
    CancelWorkflowRun {
        run: i64,
    },
    /// The person reviewed the version of the script `name` whose text hashes to `hash`
    /// (answering `WorkflowReviewRequested`), and chose whether event runs of this version
    /// apply without asking. Refused when the file changed since `hash` was shown. Human only.
    ReviewWorkflow {
        name: String,
        hash: String,
        auto_approve: bool,
    },
    /// Copies the built-in template `template` into the user's workflows
    /// directory under a file name not taken yet; never into a repository.
    /// Answers `WorkflowCreated`. Human only.
    CreateWorkflowFromTemplate {
        template: String,
    },
    /// Lists the newest `limit` runs of `workflow` in the open repository.
    /// Answers `WorkflowRunsListed`. A read: never recorded.
    ListWorkflowRuns {
        workflow: String,
        limit: u32,
    },
    /// Reads one run: its script, input and every `ctx` call. Answers `WorkflowRunLoaded`.
    /// A read: never recorded.
    LoadWorkflowRun {
        run: i64,
    },
    /// Dry run of `source`, the workflow text the editor holds (not a saved
    /// file): runs it with every `ctx` call answered by the dry host, so nothing is
    /// performed, and answers `WorkflowDryRun` with the calls it would make. `inputs` is an object of input values; a declared input
    /// it does not give is `""`. Writes nothing, and is never recorded.
    DryRunWorkflow {
        source: String,
        inputs: serde_json::Value,
    },
    /// Checks `source`, the workflow text the editor holds, without running it: the
    /// problems `inspect` finds (a syntax error, a missing `execute`, a bad `M.meta`), each
    /// with its line. Answers `WorkflowChecked`, with no problems when the text loads. Nothing
    /// is written and nothing is run, so agents may ask too; never recorded.
    CheckWorkflowScript {
        source: String,
    },
    /// Writes `source` to the `.lua` file at `path`, which must be inside
    /// the user's workflows directories (never a repository). With
    /// `expected_hash`, refuses when the file on disk no longer hashes to it
    /// (changed elsewhere). A new file passes `None`. Answers
    /// `WorkflowFileSaved`. Human only.
    SaveWorkflowFile {
        path: String,
        source: String,
        expected_hash: Option<String>,
    },
    /// Moves the workflow file at `path` to the trash folder of the app's data
    /// directory, when it still hashes to `expected_hash`. Answers
    /// `WorkflowFileDeleted`. Human only.
    DeleteWorkflowFile {
        path: String,
        expected_hash: String,
    },
    /// Comments on the lines `start_line` to `end_line` (1-based, inclusive) of
    /// `path` in the working tree (B4, D96). The file is read from disk and the
    /// anchor is taken from its text now, so a path outside the worktree, a
    /// missing or non-UTF-8 file and a blank body are refused. A snapshot of the
    /// working tree is taken for the comment (D97). Answers
    /// `ReviewCommentsListed`. Human only.
    AddReviewComment {
        path: String,
        start_line: u32,
        end_line: u32,
        body: String,
    },
    /// Lists every review comment, with where its anchor is in the file now.
    /// Answers `ReviewCommentsListed`. A read: never recorded (D28).
    ListReviewComments,
    /// Replaces the body of review comment `id`. Human only.
    EditReviewComment {
        id: i64,
        body: String,
    },
    /// Marks review comment `id` resolved, which is refused when it already is.
    /// Human only.
    ResolveReviewComment {
        id: i64,
    },
    /// Marks review comment `id` open again, which is refused when it is not
    /// resolved. Human only.
    ReopenReviewComment {
        id: i64,
    },
    /// Deletes review comment `id` and the private ref that keeps its snapshot.
    /// Human only.
    DeleteReviewComment {
        id: i64,
    },
    /// Reads the lines review comment `id` covered in its snapshot, for the
    /// Before side of the diff. Answers `ReviewSnapshotText`. A read: never
    /// recorded (D28).
    LoadReviewSnapshotText {
        id: i64,
    },
    /// Reads the lines review comment `id` covers in the working tree now, for
    /// the After side of the diff. Answers `ReviewCurrentText`. A read: never
    /// recorded (D28).
    LoadReviewCurrentText {
        id: i64,
    },
    /// Reads the conflicted file `path` of the operation that stopped (D114):
    /// the three stages merged into zdiff3 text, with the sides named by role.
    /// Answers `ConflictRead`. A read: never recorded (D28).
    ReadConflict {
        path: String,
    },
    /// Writes `text` as the resolution of the conflicted file `path`, refused
    /// while markers remain, then stages it. Human only, and recorded in the
    /// command log. Answers `ConflictResolved`.
    ResolveConflict {
        path: String,
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Command {
    pub actor: Actor,
    pub kind: CommandKind,
}

/// `Requested → Gating → AwaitingApproval → Executing → Done | Failed |
/// Vetoed | Cancelled` (design/architecture.md, rule 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Requested,
    Gating,
    AwaitingApproval,
    Executing,
    Done,
    Failed,
    Vetoed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IllegalTransition {
    pub from: OperationState,
    pub to: OperationState,
}

impl OperationState {
    pub const ALL: [Self; 8] = [
        Self::Requested,
        Self::Gating,
        Self::AwaitingApproval,
        Self::Executing,
        Self::Done,
        Self::Failed,
        Self::Vetoed,
        Self::Cancelled,
    ];

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Vetoed | Self::Cancelled
        )
    }

    pub fn can_move_to(self, to: Self) -> bool {
        use OperationState::{
            AwaitingApproval, Cancelled, Done, Executing, Failed, Gating, Requested, Vetoed,
        };
        matches!(
            (self, to),
            (Requested, Gating | Cancelled)
                | (
                    Gating,
                    AwaitingApproval | Executing | Vetoed | Failed | Cancelled
                )
                | (AwaitingApproval, Executing | Vetoed | Cancelled)
                | (Executing, Done | Failed | Cancelled)
        )
    }

    pub fn move_to(self, to: Self) -> Result<Self, IllegalTransition> {
        if self.can_move_to(to) {
            Ok(to)
        } else {
            Err(IllegalTransition { from: self, to })
        }
    }
}

/// What front-ends subscribe to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    OperationChanged {
        id: OperationId,
        actor: Actor,
        state: OperationState,
        /// Failure reason, when `state` is `failed`.
        detail: Option<String>,
    },
    StatusLoaded {
        id: OperationId,
        status: RepoStatus,
    },
    DiffLoaded {
        id: OperationId,
        side: DiffSide,
        diff: FileDiff,
    },
    Committed {
        id: OperationId,
        oid: String,
    },
    BranchesLoaded {
        id: OperationId,
        branches: Vec<BranchEntry>,
        /// The configured `user.email`, without angle brackets; `None` when unset.
        user_email: Option<String>,
    },
    /// Answers `LoadHistory`.
    HistoryLoaded {
        id: OperationId,
        history: History,
    },
    /// Answers `LoadReflog`: the entries after `skip`, and whether more follow.
    ReflogLoaded {
        id: OperationId,
        skip: u32,
        entries: Vec<ReflogEntry>,
        has_more: bool,
    },
    /// Answers `SearchFiles`.
    FilesFound {
        id: OperationId,
        search: FileSearch,
    },
    /// Answers `ListDirectory`.
    DirectoryListed {
        id: OperationId,
        listing: DirListing,
    },
    /// Answers `SearchText`.
    TextFound {
        id: OperationId,
        result: GrepResult,
    },
    /// Answers `ReadFile`.
    FileRead {
        id: OperationId,
        file: FileView,
    },
    /// The colors of the file `FileRead` carried, sent right after it when the
    /// language has a grammar (N4). `id` is the read's. Spans are in order, do
    /// not overlap, and fall on character boundaries of the text. Not sent when
    /// the file is superseded, too large to parse in time, or in a language
    /// with no grammar.
    FileHighlighted {
        id: OperationId,
        spans: Vec<HighlightSpan>,
    },
    /// The file list was (re)read: `files` paths now. Asked for by `IndexFiles`;
    /// whoever shows a search or a folder listing asks again.
    FilesIndexed {
        id: OperationId,
        files: usize,
    },
    /// Answers `LoadCommitDetail`.
    CommitDetailLoaded {
        id: OperationId,
        detail: CommitDetail,
    },
    /// Answers `LoadCommitStack`: one diff per listed file, in the same order.
    CommitStackLoaded {
        id: OperationId,
        diffs: Vec<FileDiff>,
    },
    /// Answers `ListWorktrees`, and follows each `RefreshStatus` and
    /// `OpenRepo`, so the worktree list is as fresh as the status.
    WorktreesLoaded {
        id: OperationId,
        worktrees: Vec<WorktreeEntry>,
    },
    /// Answers `InspectWorktree`: what removing the worktree would lose or refuse.
    WorktreeInspected {
        id: OperationId,
        inspection: WorktreeInspection,
    },
    /// Answers `PreviewMoveChanges`: the blockers of the move, none when it would work.
    MovePreviewed {
        id: OperationId,
        preview: MovePreview,
    },
    /// Answers `MoveChanges`: what the move did. Follows the status, as after a
    /// change to the worktrees.
    ChangesMoved {
        id: OperationId,
        moved: MoveOutcome,
    },
    /// Answers `ListPrunable`: the admin records a prune would remove.
    PrunableListed {
        id: OperationId,
        candidates: Vec<PruneCandidate>,
    },
    /// Answers `ListMergedBranches`: the base the check used (`None` when there
    /// is none, as on a detached HEAD with no base branch) and the branches.
    MergedBranchesListed {
        id: OperationId,
        base: Option<String>,
        branches: Vec<MergedBranch>,
    },
    /// Answers `ListStashes`, and follows each stash write (after the status), so
    /// the list is as fresh as the status.
    StashesListed {
        id: OperationId,
        stashes: Vec<StashEntry>,
    },
    /// Answers `DeleteMergedBranches` with what was removed and what Git refused,
    /// before the operation's own terminal event.
    MergedBranchesDeleted {
        id: OperationId,
        deleted: Vec<String>,
        failures: Vec<BranchFailure>,
    },
    HooksListed {
        id: OperationId,
        hooks: Vec<HookInfo>,
    },
    /// A chain step is about to run; `step` counts from 1 of `of`.
    HookStarted {
        id: OperationId,
        hook: String,
        step: u32,
        of: u32,
    },
    /// The whole chain answered; `message` is the last hook's output.
    /// `repo_changed` is true when HEAD or the staged diff differs from the
    /// fingerprint taken when the run started: the message is then a
    /// suggestion for the new state, not for the state the user saw.
    HookFinished {
        id: OperationId,
        message: String,
        repo_changed: bool,
    },
    HookFailed {
        id: OperationId,
        hook: String,
        failure: HookFailure,
    },
    HookCancelled {
        id: OperationId,
        hook: String,
    },
    /// Answers `ListWorkflows`. `ignored` holds each directory or file that was not
    /// read, as `<path>: <reason>` (a `.toml` file is one of these).
    WorkflowsListed {
        id: OperationId,
        workflows: Vec<WorkflowInfo>,
        ignored: Vec<String>,
        /// The user's workflows directory, whether or not it exists yet.
        user_dir: Option<String>,
        /// Names of the built-in templates `CreateWorkflowFromTemplate` takes.
        templates: Vec<String>,
    },
    WorkflowRunsListed {
        id: OperationId,
        workflow: String,
        runs: Vec<WorkflowRunSummary>,
    },
    /// Boxed: a run's detail is far larger than any other event.
    WorkflowRunLoaded {
        id: OperationId,
        detail: Box<WorkflowRunDetail>,
    },
    /// Answers `DryRunWorkflow`: the calls the script would make, and why the
    /// walk stopped. Boxed, like `WorkflowRunLoaded`.
    WorkflowDryRun {
        id: OperationId,
        dry_run: Box<WorkflowDryRun>,
    },
    /// Answers `CheckWorkflowScript`: every problem of the text, empty when it loads.
    WorkflowChecked {
        id: OperationId,
        diagnostics: Vec<WorkflowDiagnostic>,
    },
    /// The file at `path` now holds what was saved; `hash` is its new sha256.
    WorkflowFileSaved {
        id: OperationId,
        path: String,
        hash: String,
    },
    /// The file was moved to `trashed_to`.
    WorkflowFileDeleted {
        id: OperationId,
        path: String,
        trashed_to: String,
    },
    /// A template was copied to `path`; the workflow inside is named `name`.
    WorkflowCreated {
        id: OperationId,
        path: String,
        name: String,
    },
    /// `RunWorkflow` found a version of the script the person has not reviewed (D101), so it did
    /// not start. The window shows `source` and asks "Run this version?"; a yes is
    /// `ReviewWorkflow` with this `hash`, then `RunWorkflow` again.
    WorkflowReviewRequested {
        id: OperationId,
        name: String,
        hash: String,
        source: String,
        /// The inputs of the run that did not start, so the window can send it again.
        inputs: BTreeMap<String, String>,
    },
    /// A workflow run moved: its status, and the state it is in when it has one.
    WorkflowRunChanged {
        run: i64,
        workflow: String,
        status: WorkflowRunStatus,
        state: Option<String>,
    },
    /// A `commit.suggest_message` step: the composer shows `message` as a suggestion.
    WorkflowMessageSuggested {
        run: i64,
        message: String,
    },
    /// A `notify` step: a line for the user.
    WorkflowNotice {
        run: i64,
        text: String,
    },
    /// The run waits at the `approve` state `state`; `summary` is what it asks.
    WorkflowApprovalRequested {
        run: i64,
        state: String,
        summary: String,
    },
    /// The run ended. `reason` says why when it failed, was cancelled or was blocked.
    WorkflowRunFinished {
        run: i64,
        end: WorkflowEnd,
        reason: Option<String>,
    },
    /// Answers `ListReviewComments`, and follows each review write, so the list
    /// is as fresh as the write.
    ReviewCommentsListed {
        id: OperationId,
        comments: Vec<ReviewComment>,
    },
    /// Answers `LoadReviewSnapshotText` for comment `comment`: the lines it
    /// covered in the snapshot, empty when there is no snapshot or the file was
    /// not in it.
    ReviewSnapshotText {
        id: OperationId,
        comment: i64,
        path: String,
        lines: Vec<String>,
    },
    /// Answers `LoadReviewCurrentText` for comment `comment`: the lines it covers
    /// in the file now, empty when the comment is outdated or the file is gone.
    ReviewCurrentText {
        id: OperationId,
        comment: i64,
        path: String,
        lines: Vec<String>,
    },
    /// Answers `ReadConflict`.
    ConflictRead {
        id: OperationId,
        read: ConflictFileRead,
    },
    /// Answers `ResolveConflict`: the file is staged with the resolution. Follows
    /// the status, as after any stage.
    ConflictResolved {
        id: OperationId,
        path: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn checkout_command_wire_format_is_tagged() {
        let command = CommandKind::Checkout {
            branch: "origin/feature".into(),
        };
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["type"], "checkout");
        assert_eq!(json["branch"], "origin/feature");
        let list = serde_json::to_value(CommandKind::ListBranches).unwrap();
        assert_eq!(list["type"], "list_branches");
    }

    #[test]
    fn branch_commands_wire_format_is_tagged() {
        let create = serde_json::to_value(CommandKind::CreateBranch {
            name: "topic".into(),
            start_point: None,
            checkout: true,
        })
        .unwrap();
        assert_eq!(create["type"], "create_branch");
        assert_eq!(create["checkout"], true);
        assert!(create["start_point"].is_null());
        let delete = serde_json::to_value(CommandKind::DeleteBranch {
            name: "topic".into(),
            force: false,
        })
        .unwrap();
        assert_eq!(delete["type"], "delete_branch");
        assert_eq!(delete["force"], false);
    }

    #[test]
    fn merged_branch_commands_wire_format_is_tagged() {
        let list = serde_json::to_value(CommandKind::ListMergedBranches).unwrap();
        assert_eq!(list["type"], "list_merged_branches");
        let delete = serde_json::to_value(CommandKind::DeleteMergedBranches {
            branches: vec!["topic".into()],
        })
        .unwrap();
        assert_eq!(delete["type"], "delete_merged_branches");
        assert_eq!(delete["branches"][0], "topic");
    }

    #[test]
    fn discard_file_wire_format_is_tagged() {
        let discard = serde_json::to_value(CommandKind::DiscardFile {
            path: "src/main.rs".into(),
            include_staged: true,
        })
        .unwrap();
        assert_eq!(discard["type"], "discard_file");
        assert_eq!(discard["path"], "src/main.rs");
        assert_eq!(discard["include_staged"], true);
        let back: CommandKind = serde_json::from_value(discard).unwrap();
        assert_eq!(
            back,
            CommandKind::DiscardFile {
                path: "src/main.rs".into(),
                include_staged: true,
            }
        );
    }

    #[test]
    fn stash_commands_wire_format_is_tagged() {
        let create = serde_json::to_value(CommandKind::CreateStash {
            message: Some("wip".into()),
            include_untracked: true,
        })
        .unwrap();
        assert_eq!(create["type"], "create_stash");
        assert_eq!(create["include_untracked"], true);
        let pop = serde_json::to_value(CommandKind::PopStash {
            index: 2,
            oid: "a".repeat(40),
        })
        .unwrap();
        assert_eq!(pop["type"], "pop_stash");
        assert_eq!(pop["index"], 2);
        let back: CommandKind = serde_json::from_value(pop).unwrap();
        assert_eq!(
            back,
            CommandKind::PopStash {
                index: 2,
                oid: "a".repeat(40),
            }
        );
        let list = serde_json::to_value(CommandKind::ListStashes).unwrap();
        assert_eq!(list["type"], "list_stashes");
    }

    #[test]
    fn review_commands_wire_format_is_tagged() {
        let add = serde_json::to_value(CommandKind::AddReviewComment {
            path: "src/a.rs".into(),
            start_line: 3,
            end_line: 4,
            body: "rename".into(),
        })
        .unwrap();
        assert_eq!(add["type"], "add_review_comment");
        assert_eq!(add["end_line"], 4);
        let resolve = serde_json::to_value(CommandKind::ResolveReviewComment { id: 7 }).unwrap();
        assert_eq!(resolve["type"], "resolve_review_comment");
        assert_eq!(resolve["id"], 7);
        let list = serde_json::to_value(CommandKind::ListReviewComments).unwrap();
        assert_eq!(list["type"], "list_review_comments");
    }

    #[test]
    fn reflog_commands_wire_format_is_tagged() {
        let load = serde_json::to_value(CommandKind::LoadReflog { skip: 200 }).unwrap();
        assert_eq!(load["type"], "load_reflog");
        assert_eq!(load["skip"], 200);
        let checkout = serde_json::to_value(CommandKind::CheckoutCommit {
            oid: "a".repeat(40),
        })
        .unwrap();
        assert_eq!(checkout["type"], "checkout_commit");
        let back: CommandKind = serde_json::from_value(checkout).unwrap();
        assert_eq!(
            back,
            CommandKind::CheckoutCommit {
                oid: "a".repeat(40),
            }
        );
    }

    #[test]
    fn remote_commands_wire_format_is_tagged() {
        let fetch = serde_json::to_value(CommandKind::Fetch).unwrap();
        assert_eq!(fetch["type"], "fetch");
        let pull = serde_json::to_value(CommandKind::Pull).unwrap();
        assert_eq!(pull["type"], "pull");
        let push = serde_json::to_value(CommandKind::PushWithLease).unwrap();
        assert_eq!(push["type"], "push_with_lease");
    }

    #[test]
    fn branches_loaded_wire_format_is_tagged() {
        let event = Event::BranchesLoaded {
            id: OperationId(2),
            branches: vec![BranchEntry {
                name: "main".into(),
                is_remote: false,
                is_current: true,
                upstream: Some("origin/main".into()),
                author_email: Some("ana@example.com".into()),
            }],
            user_email: Some("me@example.com".into()),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "branches_loaded");
        assert_eq!(json["branches"][0]["is_current"], true);
        assert_eq!(json["branches"][0]["upstream"], "origin/main");
        assert_eq!(json["branches"][0]["author_email"], "ana@example.com");
        assert_eq!(json["user_email"], "me@example.com");
    }

    fn any_state() -> impl Strategy<Value = OperationState> {
        proptest::sample::select(OperationState::ALL.to_vec())
    }

    proptest! {
        #[test]
        fn terminal_states_never_move(from in any_state(), to in any_state()) {
            if from.is_terminal() {
                prop_assert!(!from.can_move_to(to));
            }
        }

        #[test]
        fn nothing_moves_back_to_requested(from in any_state()) {
            prop_assert!(!from.can_move_to(OperationState::Requested));
        }
    }

    #[test]
    fn approval_cannot_be_skipped_into_done() {
        assert!(!OperationState::Gating.can_move_to(OperationState::Done));
        assert!(!OperationState::AwaitingApproval.can_move_to(OperationState::Done));
    }

    #[test]
    fn event_wire_format_is_tagged() {
        let event = Event::OperationChanged {
            id: OperationId(1),
            actor: Actor::Agent {
                name: "agent".into(),
            },
            state: OperationState::AwaitingApproval,
            detail: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "operation_changed");
        assert_eq!(json["actor"]["type"], "agent");
        assert_eq!(json["state"], "awaiting_approval");
    }
}

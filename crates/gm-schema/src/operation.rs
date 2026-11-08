use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::conflict::{ConflictFileRead, ConflictSite, HunkOrigin};
use crate::diff::{DiffSide, FileDiff, HunkId, StageTarget};
use crate::files::{DirListing, FileSearch, FileView};
use crate::grep::{GrepOptions, GrepResult};
use crate::history::{CommitDetail, CommitFile, History, HistoryScope};
use crate::move_changes::{MoveOutcome, MovePreview, MoveTarget};
use crate::reflog::ReflogEntry;
use crate::restack::{RestackPlan, RestackRecord, RestackResolution};
use crate::review::{ReviewAuthorFilter, ReviewChange, ReviewComment, ReviewKind, ReviewSeverity};
use crate::stack::StackSnapshot;
use crate::stash::StashEntry;
use crate::status::{
    BranchEntry, BranchFailure, MergedBranch, PruneCandidate, RepoStatus, WorktreeEntry,
    WorktreeInspection,
};
use crate::submodule::SubmoduleDetail;
use crate::syntax::HighlightSpan;
use crate::threads::{
    AttachKind, Context, EditState, Label, LabelDetail, LinkKind, Memory, MemoryBasis, MemoryKind,
    MemorySourceInput, MemoryStatus, ProvenanceResult, SearchFacets, SearchGroup, SearchOrder,
    ThreadDetail, ThreadEdit, ThreadFilter, ThreadHit, ThreadLink, ThreadRelKind, ThreadRelation,
    ThreadSummary,
};
use crate::workflow::{
    WorkflowDiagnostic, WorkflowDryRun, WorkflowEnd, WorkflowInfo, WorkflowRunDetail,
    WorkflowRunStatus, WorkflowRunSummary,
};
use crate::workspace::{MemberPlan, MemberRequest, RemovalBlocker, WorkspaceProblem, WorkspaceRow};

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
    /// Records the combination of the workspace's members (D180(3)): stages the gitlink of
    /// each checked-out member whose commit the workspace does not record, and commits those
    /// gitlinks alone with a message that lists every member. Refused when no member has a
    /// new commit, when a member is detached, and when there is no member. Human only, and
    /// recorded in the command log (D28).
    RecordCombination,
    ListBranches,
    /// Lists the branches of the repository at `repo` without opening it as the window's
    /// repository (D195). Answers `RepoBranchesLoaded`. Never recorded in the command log (D28).
    ListRepoBranches {
        repo: String,
    },
    /// Reads the first `HISTORY_PAGE` commits of `scope`, newest first, and
    /// starts the walk the next pages continue. Never recorded in the command
    /// log (D28).
    LoadHistory {
        #[serde(default)]
        scope: HistoryScope,
    },
    /// Reads the next `HISTORY_PAGE` commits of the walk after `after`, the last
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
    /// Reads the state of the submodule at `path` (D175): its branch or
    /// detached HEAD, the commit the repository records, and the commits it
    /// gained since. `path` is relative to the root and must be a submodule
    /// the index records; an absolute path or one with `..` is refused. Never
    /// recorded in the command log (D28).
    LoadSubmodule {
        path: String,
    },
    /// Lists the submodules checked out in the worktree (D181): the workspace's
    /// members, by path, in index order. Empty when none is checked out, which
    /// is when the repository is not a workspace. Never recorded in the command
    /// log (D28).
    ListMembers,
    /// Lists the workspaces under the root (D183): the folders `<root>/<name>` whose repository
    /// marks them as workspaces. `root` is the flag's value, `None` for the configured or the
    /// default root (D174 (4)). Answers `WorkspacesListed`. Never recorded in the command log (D28).
    ListWorkspaces {
        root: Option<String>,
    },
    /// Checks the workspace `name` with `members` before anything is written (D178):
    /// the branch of each member and any problem. `base` is the `--base` rev of the members'
    /// new branches; `root` as for `ListWorkspaces`. Answers `WorkspacePlanned`. Writes nothing,
    /// so it is not recorded in the command log (D28).
    PlanWorkspace {
        name: String,
        members: Vec<MemberRequest>,
        base: Option<String>,
        root: Option<String>,
    },
    /// Creates the workspace that `PlanWorkspace` plans: the folder, its repository, a member
    /// per repository, and each member's commit. It plans again first and refuses with the
    /// problems when there are any, so nothing is created. Human only, and recorded in the
    /// command log (D28). Answers `WorkspaceCreated`.
    CreateWorkspace {
        name: String,
        members: Vec<MemberRequest>,
        base: Option<String>,
        root: Option<String>,
    },
    /// Reads what removing the workspace at `path` would refuse or lose (D183). The window's own
    /// workspace is a blocker, unless `closing` says the window is going away (D185). Answers
    /// `WorkspaceRemovalInspected`. Never recorded in the command log (D28).
    InspectWorkspaceRemoval {
        path: String,
        #[serde(default)]
        closing: bool,
    },
    /// Removes the workspace at `path`: each member's worktree from its repository, then the
    /// folder. It inspects again first and refuses a blocker that cannot be overridden, and the
    /// overridable ones unless `force`. The branches stay. With `closing` the window's own
    /// workspace is not a blocker, since the window closes with it (D185). Human only, and
    /// recorded in the command log (D28). Answers `WorkspaceRemoved`.
    RemoveWorkspace {
        path: String,
        force: bool,
        #[serde(default)]
        closing: bool,
    },
    /// Checks out the branch the workspace's `.gitmodules` names for the member at `path`,
    /// relative to the open repository, which is the workspace (D183). Refused when the member
    /// has no branch or when another worktree holds it. Human only, and recorded in the command
    /// log (D28). Answers with the status, as a checkout does.
    CheckOutMemberBranch {
        path: String,
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
    /// Reads every branch stack (D68's `stack.tree`): the recorded parents, and
    /// the state each branch derives from its base. Answers `StacksLoaded`.
    /// Never recorded in the command log (D28).
    LoadStacks,
    /// Plans a restack of `branch` and the branches under it (D14, D142): each
    /// branch that is stale, or under one that moves, is replayed commit by commit
    /// onto its parent's planned tip, without moving any ref. Answers
    /// `RestackPlanned`. Never recorded in the command log (D28).
    PlanRestack {
        branch: String,
        /// Branches left out (Skip branch and children): they and the ones under
        /// them are not replayed.
        #[serde(default)]
        skip: Vec<String>,
    },
    /// Runs the restack `PlanRestack { branch }` answers with `plan_id`: moves
    /// every branch of the plan that is not blocked to its new tip in one ref
    /// transaction, then records the new bases (D142). Refused when the plan
    /// changed since, and when it cannot apply. Answers `RestackFinished`. Human
    /// only.
    ApplyRestack {
        branch: String,
        plan_id: String,
        /// The branches the plan was made without.
        #[serde(default)]
        skip: Vec<String>,
    },
    /// Puts back every branch and base of the last restack, in one ref
    /// transaction. Refused when a branch moved since. Answers `RestackFinished`
    /// with no record. Human only.
    UndoRestack,
    /// Opens the resolution worktree on the conflict of `conflicted`, a branch the
    /// plan `branch` (its `from`) blocks on a conflict (D154): the worktree is checked
    /// out at that conflict's `onto` with its commit replayed, so the resolver works
    /// there. Refused when the plan changed since `plan_id` was made, or when
    /// `conflicted` has no conflict in it. Starting one closes the one open before.
    /// Answers `RestackResolutionStarted`. Human only, and recorded in the command log.
    StartRestackResolution {
        branch: String,
        plan_id: String,
        /// The branches the plan was made without.
        #[serde(default)]
        skip: Vec<String>,
        conflicted: String,
    },
    /// Builds the resolved commit of the open restack conflict from the resolution
    /// worktree (D154), records it as the resolution of that (onto, commit), closes the
    /// worktree and plans the restack again with the `from` and `skip` it was started
    /// with. Refused while a path is unmerged. Answers `RestackResolutionFinished` and
    /// `RestackPlanned`. Human only, and recorded in the command log.
    FinishRestackResolution,
    /// Closes the resolution worktree and forgets the open restack conflict; a
    /// resolution already finished stays. Succeeds when nothing is open. Answers
    /// `RestackResolutionCancelled`. Human only, and recorded in the command log.
    CancelRestackResolution,
    /// Finishes a restack that a crash or a kill interrupted, or drops its record
    /// when no ref had moved, and answers `RestackFinished` with the record undo
    /// would use (`None` when there is none). Sent when a repository opens. Human
    /// only.
    ResumeRestack,
    /// Creates the local branch `name` at the tip of the local branch `parent`
    /// and records `parent` as its stack parent, with that tip as its base
    /// (D138). Checks it out when `checkout` is set. Human only.
    CreateChildBranch {
        name: String,
        parent: String,
        checkout: bool,
    },
    /// Records the local branch `parent` as the stack parent of `branch`, with
    /// their merge base as the base (D138). Refused for the branch itself, a
    /// branch under it, and a branch that shares no history with it. Moves no
    /// commit. Human only.
    SetStackParent {
        branch: String,
        parent: String,
    },
    /// Removes the stack parent and base of `branch`. Its children keep theirs.
    /// Human only.
    ClearStackParent {
        branch: String,
    },
    /// Records the parent of the old parent as the stack parent of `branch`,
    /// keeping its base, when the old parent is gone or merged. With the old
    /// parent's own parent unknown, the default branch is taken (D138). Human only.
    ReparentToGrandparent {
        branch: String,
    },
    /// Moves `branch` and the branches under it onto `parent` as the plan `plan_id` of
    /// `reparent.plan` says, and records `parent` as its stack parent (`WF4a`). Written by a
    /// workflow run only: the engine refuses it from any other source, and the log records it under
    /// the run's operation (D81).
    ApplyReparent {
        branch: String,
        parent: String,
        plan_id: String,
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
    /// Lists the `gitmaster.prepare-commit.*` sections of the user's and the repository's Git
    /// config, each as `<scope> gitmaster.prepare-commit.<name>` (`WF3e`, D160). Never records
    /// anything.
    ListHookSections,
    /// Converts the user's `prepare-commit` sections into a `transform.commit_message` workflow in
    /// the user's workflows directory (`WF3e`, D160). Writes a file, never Git config. Human only.
    ConvertHooksToWorkflows,
    /// Cancels the operation `op` while it waits for its workflow gates (`WF3c`, D157): the gate
    /// run is stopped and the operation ends `Cancelled`, with nothing written. Refused for any
    /// other operation. Human only.
    CancelOperation {
        op: OperationId,
    },
    /// Runs every reviewed `transform.<field>` workflow, in file-name order, for `field`
    /// (`commit_message` or `branch_name`), whose value so far is `current` (`WF3d`, D155). Answers
    /// with `TransformFinished`. Human only.
    RunTransform {
        field: String,
        current: String,
    },
    /// The empty message box of the commit composer got focus while something is staged (`WF3d`,
    /// D155(1)): fires `composer.opened`, then runs `transform.commit_message` with nothing typed
    /// yet. Answers with `TransformFinished`. Human only.
    ComposerOpened,
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
    /// Takes back the review of the version whose text hashes to `hash`, and with it the grant
    /// (`WF3b`, D101). Answers `WorkflowsListed`. Human only.
    RevokeWorkflowReview {
        hash: String,
    },
    /// Drops the pending event request of the workflow `workflow`: the notice goes and the run
    /// does not start (`WF3b`, D155(3)). Answers `WorkflowRequestDropped`. Human only.
    DismissWorkflowRequest {
        workflow: String,
    },
    /// Copies the built-in template `template` into the user's workflows
    /// directory under a file name not taken yet; never into a repository.
    /// Answers `WorkflowCreated`. Human only.
    CreateWorkflowFromTemplate {
        template: String,
    },
    /// Turns the workflow `name` on or off in the open repository (`WF4b`, D164(1)). The name
    /// of a user workflow is switched off with `disabled`; a built-in one is switched on with
    /// `enabled_builtin`. Writes `.git/gitmaster/workflows.json` and no Git state, so it is not
    /// logged. Answers `WorkflowsListed`. Human only.
    SetWorkflowEnabled {
        name: String,
        enabled: bool,
    },
    /// Copies the built-in workflow `name` into the user's workflows directory as
    /// `<name>-custom.lua` (then `-2`, ...), unreviewed (`WF4b`, D164(4)). Answers
    /// `WorkflowCopied`. Human only.
    CopyBuiltinWorkflow {
        name: String,
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
    /// Comments on `text`, lines joined with `\n`, found in `path` in the working
    /// tree (D141). For a surface that does not show the working tree (the staged
    /// side, a commit, a stash): the run of whole lines must appear exactly once
    /// in the file now, otherwise the comment is refused and nothing is written.
    /// Answers `ReviewCommentsListed`. Human only.
    AddReviewCommentByText {
        path: String,
        text: String,
        body: String,
    },
    /// Lists the review comments, with where each anchor is in the file now. `author`
    /// picks the threads by who started them (D200): by default the person's own, and the
    /// agent's that hold a reply from the person. Answers `ReviewCommentsListed`. A read:
    /// never recorded (D28).
    ListReviewComments {
        #[serde(default)]
        author: ReviewAuthorFilter,
    },
    /// Starts a review thread on `anchor_text`, lines joined with `\n`, which must be
    /// in `path` in the working tree exactly once; otherwise it is refused and nothing is
    /// written (D200). Only an agent starts one, and its author is the agent. A flag takes
    /// a `severity` and no other kind does. Refused when the repository already has 20 open
    /// agent threads or the file has 5. A snapshot is taken as for a person's comment
    /// (D140). Answers `ReviewCommentsListed`.
    CreateAgentReviewComment {
        path: String,
        anchor_text: String,
        kind: ReviewKind,
        body: String,
        #[serde(default)]
        severity: Option<ReviewSeverity>,
    },
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
    /// Marks review comment `id` handled by an agent (D135), and adds `summary`, its
    /// account of what it did, as a reply by the agent. Refused for a resolved comment
    /// and for a blank summary. Writes the app's own store, then takes the "after"
    /// snapshot of the working tree and keeps it under a private ref (D140); a
    /// snapshot that fails leaves the comment handled without one. Answers
    /// `ReviewCommentsListed`.
    HandleReviewComment {
        id: i64,
        summary: String,
    },
    /// Adds `body` as a reply to review comment `id`, which may be a root or a reply
    /// (the reply attaches to the root). Refused for a resolved comment and for a blank
    /// body. An agent's reply leaves the status as it is; a person's reply reopens a
    /// handled comment, as `ReopenReviewComment` does (D140). Answers
    /// `ReviewCommentsListed`.
    ReplyToReviewComment {
        id: i64,
        body: String,
    },
    /// Marks review comment `id` open again, which is refused when it is already
    /// open. The "after" snapshot is dropped (D140); the replies stay.
    /// Human only.
    ReopenReviewComment {
        id: i64,
    },
    /// Deletes review comment `id` and the private refs that keep its
    /// snapshots. Human only.
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
    /// Reads the anchored lines of review comment `id` in its "before" and
    /// "after" snapshots (D140). Answers `ReviewChange`. A read: never recorded
    /// (D28).
    LoadReviewChange {
        id: i64,
    },
    /// Reads the conflicted file `path` of the operation that stopped (D114):
    /// the three stages merged into zdiff3 text, with the sides named by role.
    /// Answers `ConflictRead`. A read: never recorded (D28).
    ReadConflict {
        path: String,
        /// The worktree the file is read from; `restack` reads the restack conflict
        /// being resolved, and the sides are named as a rebase (D154).
        #[serde(default)]
        site: ConflictSite,
    },
    /// Writes `text` as the resolution of the conflicted file `path`, refused
    /// while markers remain, then stages it. `origins` holds who resolved each
    /// hunk, in order; an older payload without it has none. Human only, and
    /// recorded in the command log. Answers `ConflictResolved`.
    ResolveConflict {
        path: String,
        text: String,
        #[serde(default)]
        origins: Vec<HunkOrigin>,
        /// The worktree the file is in (D154): `restack` is the resolution worktree.
        #[serde(default)]
        site: ConflictSite,
    },
    /// Forgets the resolution Git recorded for the conflicted file `path` and
    /// restores its conflict markers in the working tree. Refused when nothing is
    /// recorded or the file is not conflicted. Human only, and recorded in the
    /// command log. Answers `RecordedResolutionForgotten`.
    ForgetRecordedResolution {
        path: String,
        /// The worktree the file is in (D154): `restack` is the resolution worktree.
        #[serde(default)]
        site: ConflictSite,
    },
    /// Lists thread summaries, newest first, with `total` threads in the store.
    /// `before_at` and `before_uid` name the last row of the previous page, and
    /// are both set or both unset. Answers `ThreadsListed`. A read (D08).
    ListThreads {
        filter: ThreadFilter,
        before_at: Option<i64>,
        before_uid: Option<String>,
        limit: u32,
    },
    /// Searches entry text and edit paths; with an empty `query` it lists the threads
    /// the filter matches, newest first (D147). `limit` is the page size; `cursor` is the
    /// `next_cursor` of the previous page. `group` and `order` shape the hits, `context`
    /// (0 to 3) adds the entries around each hit, `facets` asks for the common values of
    /// the matching threads. Answers `ThreadSearchResults`. A read (D08).
    SearchThreads {
        query: String,
        filter: ThreadFilter,
        limit: u32,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default)]
        group: SearchGroup,
        #[serde(default)]
        order: SearchOrder,
        #[serde(default)]
        context: u8,
        #[serde(default)]
        facets: bool,
    },
    /// Reads the entries of thread `uid` from `from_seq`, with their edits and links.
    /// Answers `ThreadRead`. A read (D08).
    ReadThread {
        uid: String,
        from_seq: u32,
        limit: u32,
    },
    /// Explains the lines `from_line` to `to_line` (1-based, inclusive) of `path`: the commits
    /// that last changed them (`git blame`), their threads and the entries around each
    /// matching edit (D144). Bounded: 10 commits, 5 threads per commit, and a range of at most
    /// 1000 lines. Answers `ThreadProvenance`. A read (D08), never in the command log (D28).
    ReadProvenance {
        path: String,
        from_line: u32,
        to_line: u32,
    },
    /// Reads edit `id` with its state in the working tree and at `HEAD`. Answers
    /// `ThreadEditRead`. A read (D08).
    ReadThreadEdit {
        id: i64,
    },
    /// Renders thread `uid` as the Markdown `ExportThread` writes, redacted with the
    /// current patterns, and returns it. Answers `ThreadMarkdown`. A read (D08).
    ReadThreadMarkdown {
        uid: String,
    },
    /// Writes thread `uid` as Markdown to the absolute `path`, outside the worktree,
    /// redacted again with the current patterns. Answers `ThreadExported`. Human only.
    ExportThread {
        uid: String,
        path: String,
    },
    /// Edits the metadata of thread `uid`; never its entries. Its labels are edited
    /// by the label commands. Human only.
    UpdateThread {
        uid: String,
        title: Option<String>,
        note: Option<String>,
        starred: Option<bool>,
    },
    /// Adds a link from thread `uid` to a commit, branch or path. Human only.
    LinkThread {
        uid: String,
        link: ThreadLink,
    },
    /// Removes a link of thread `uid`. Human only.
    UnlinkThread {
        uid: String,
        kind: LinkKind,
        reference: String,
    },
    /// Lists the labels whose name contains `query` (empty for all), by name, with how many rows
    /// each has attached (D145). Answers `LabelsListed`. A read: never recorded (D28).
    ListLabels {
        query: String,
        limit: u32,
    },
    /// Reads label `id` and what is attached to it. Answers `LabelRead`. A read: never recorded
    /// (D28).
    ReadLabel {
        id: i64,
    },
    /// Creates the label `name`, or returns the label of that name when there is one (the name
    /// is unique and the other fields are then not changed). `color` is `#rrggbb` or none. Level
    /// record (D28, D146): writes only the app's own store, needs no approval, and is logged
    /// with its actor. Answers `LabelsChanged`.
    CreateLabel {
        name: String,
        description: String,
        color: Option<String>,
    },
    /// Attaches the label `label` (by name, created with the actor's origin when missing) to
    /// `kind` `ref`: a thread uid, a repository-relative path, a commit id (a snapshot is taken
    /// from Git now) or the id of another label. Attaching a pair already attached returns it
    /// unchanged. Level record (D146). Answers `LabelsChanged`.
    AttachLabel {
        label: String,
        kind: AttachKind,
        #[serde(rename = "ref")]
        reference: String,
    },
    /// Links thread `from` to thread `to` with `kind` (D143). Level record (D146). Answers
    /// `ThreadRelationChanged`.
    LinkThreads {
        from: String,
        to: String,
        kind: ThreadRelKind,
    },
    /// Renames label `id` to `name`. Human only.
    RenameLabel {
        id: i64,
        name: String,
    },
    /// Deletes label `id` with its attachments. Human only.
    DeleteLabel {
        id: i64,
    },
    /// Removes one attachment of the label `label` (by name). Human only.
    DetachLabel {
        label: String,
        kind: AttachKind,
        #[serde(rename = "ref")]
        reference: String,
    },
    /// Removes the link from thread `from` to thread `to` of `kind`. Named apart from
    /// `UnlinkThread`, which removes a commit link. Human only. Answers `ThreadRelationChanged`.
    RemoveThreadRelation {
        from: String,
        to: String,
        kind: ThreadRelKind,
    },
    /// Lists the memories the repository sees (D190, D192, D193): its own and those of the
    /// contexts it belongs to, each once, or the `scope` named (`all` when none, `repository`, or
    /// a context by its path or a name unique among the visible ones). Newest first, or by
    /// relevance with a `query` (empty for all). An agent gets `accepted` memories when `status` is
    /// none. Each memory carries its `scope` and `files_changed_since`. A read: never recorded
    /// (D28). The memory text is untrusted data. Answers `MemoriesListed`.
    ListMemories {
        query: String,
        kind: Option<MemoryKind>,
        status: Option<MemoryStatus>,
        path: Option<String>,
        author: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        limit: u32,
        cursor: Option<String>,
    },
    /// Reads memory `uid`, from the repository or from a context it sees, with its sources, its
    /// relations and `files_changed_since`. Answers `MemoryRead`. A read: never recorded (D28).
    ReadMemory {
        uid: String,
    },
    /// Counts one retrieval, now, of each memory in `uids` that the repository sees (D201 (4)). The
    /// MCP reads send it after they answer, with the memories they returned; the app never does.
    /// It is a bookkeeping write on a read, so it is not logged (D28), and a uid the repository does
    /// not see is skipped. It answers with no event of its own.
    RecordMemoryRetrieval {
        uids: Vec<String>,
    },
    /// Proposes a memory of `kind` with `text` (redacted before it is stored, D121) and its
    /// `sources`. Level record: an agent's call creates a `proposed` memory and needs at least
    /// one source; a human's creates an `accepted` one and records the blob of each file source.
    /// With `context` (a context the repository sees, by path or unique name) the memory is stored
    /// once in that context and found from every repository that sees it (D193 (7)). Logged with
    /// its actor. A text already held by a non-rejected memory of the same kind returns that memory
    /// and writes nothing. `basis` says how the memory is known (D201 (1)); absent, it is `observed`.
    /// Answers `MemoryChanged`.
    ProposeMemory {
        kind: MemoryKind,
        text: String,
        sources: Vec<MemorySourceInput>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<String>,
        #[serde(default)]
        basis: MemoryBasis,
    },
    /// Accepts memory `uid` (a proposal, or an accepted one for an edit or a supersede). `text`
    /// replaces the text when given (redacted). Each memory uid in `supersedes` must be accepted,
    /// in the same store as the memory, and becomes superseded in the same write. Human only.
    /// Answers `MemoryChanged`.
    AcceptMemory {
        uid: String,
        text: Option<String>,
        supersedes: Vec<String>,
    },
    /// Rejects the proposed memory `uid`. Human only. Answers `MemoryChanged`.
    RejectMemory {
        uid: String,
    },
    /// Deletes memory `uid` with its sources and links. Human only. Answers `MemoryChanged`.
    DeleteMemory {
        uid: String,
    },
    /// Moves repository memory `uid` (proposed or accepted, with no supersedes links) into context
    /// `context`, keeping its uid, history and sources; the repository's copy is deleted after the
    /// context holds it (D193 (8)). Human only. Answers `MemoryChanged`.
    MoveMemoryToContext {
        uid: String,
        context: i64,
    },
    /// Attaches accepted context memory `uid` to context `context`, which the repository sees too.
    /// Human only. Answers `MemoryChanged`.
    AttachMemory {
        uid: String,
        context: i64,
    },
    /// Detaches context memory `uid` from context `context`. The last context of a memory cannot
    /// be detached. Human only. Answers `MemoryChanged`.
    DetachMemory {
        uid: String,
        context: i64,
    },
    /// Lists the contexts (design/threads.md, "Contexts"; D191): every context, or with `repo`
    /// those the open repository sees, each with its path. A clone whose identity clashes with
    /// another clone lists none and names the other one (D193 (4)). With no `contexts.db` the
    /// list is empty and no file is made. A read: never recorded (D28). Answers `ContextsListed`.
    ListContexts {
        repo: bool,
    },
    /// Previews `DeleteContext` for context `id` without writing: the sub-contexts it has and the
    /// memories it holds alone, which `DeleteContext` with `delete_memories` true would delete. The
    /// list is capped at 50 with the full count in `total`. A read: never recorded (D28). Answers
    /// `ContextDeletePreview`.
    PreviewDeleteContext {
        id: i64,
    },
    /// Creates context `name` under `parent` (the root when none). Human only. Answers
    /// `ContextsChanged`.
    CreateContext {
        name: String,
        parent: Option<i64>,
    },
    /// Renames context `id` and/or moves it under `parent` (`Some(None)` moves it to the root).
    /// Refuses a cycle and a sibling with the same name. Human only. Answers `ContextsChanged`.
    UpdateContext {
        id: i64,
        name: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_option"
        )]
        parent: Option<Option<i64>>,
    },
    /// Deletes context `id`. Refused while it has sub-contexts, and while memories belong to it
    /// alone unless `delete_memories` is true, which deletes them too. Human only. Answers
    /// `ContextsChanged` with the number of memories deleted.
    DeleteContext {
        id: i64,
        delete_memories: bool,
    },
    /// Adds the repository that holds `path` to context `context`, writing its `repo-id` on the
    /// first join (D191 (4) (b)). A path that is not a repository is refused. Human only. Answers
    /// `ContextsChanged`.
    AddRepoToContext {
        context: i64,
        path: String,
    },
    /// Removes repository `repo_id` from context `context`; its memories stay. Human only. Answers
    /// `ContextsChanged`.
    RemoveRepoFromContext {
        context: i64,
        repo_id: String,
    },
    /// Gives the open repository a new id, outside every context (D193 (4)), so a copied clone
    /// can be told apart. Human only. Answers `ContextsChanged` with no context.
    RenewRepoId,
    /// Deletes thread `uid` with its entries, edits and links, securely. Human only.
    DeleteThread {
        uid: String,
    },
    /// Deletes entry `id` and its edits, securely. Human only.
    DeleteThreadEntry {
        id: i64,
    },
    /// Applies the redaction patterns to every stored text and title, then answers
    /// `ThreadsScrubbed`. Human only.
    ScrubThreads,
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
    /// Answers `LoadStacks`, and follows each stack write after the branch list.
    StacksLoaded {
        id: OperationId,
        stacks: StackSnapshot,
    },
    /// Answers `ListRepoBranches`: the branches of `repo`, as `BranchesLoaded` lists them (D195).
    RepoBranchesLoaded {
        id: OperationId,
        repo: String,
        branches: Vec<BranchEntry>,
    },
    /// Answers `StartRestackResolution`: the worktree is open, with the conflict.
    RestackResolutionStarted {
        id: OperationId,
        resolution: RestackResolution,
    },
    /// Answers `FinishRestackResolution`: `resolved` is the commit the conflict became.
    RestackResolutionFinished {
        id: OperationId,
        resolved: String,
    },
    /// Answers `CancelRestackResolution`.
    RestackResolutionCancelled {
        id: OperationId,
    },
    /// Answers `PlanRestack`.
    RestackPlanned {
        id: OperationId,
        plan: RestackPlan,
    },
    /// Answers `ApplyRestack` (the record that undo will use) and `UndoRestack`
    /// (`None`); the branches, the status and the stacks follow.
    RestackFinished {
        id: OperationId,
        record: Option<RestackRecord>,
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
    /// Answers `LoadSubmodule`.
    SubmoduleLoaded {
        id: OperationId,
        submodule: SubmoduleDetail,
    },
    /// Answers `ListMembers`: the paths of the checked-out submodules.
    MembersListed {
        id: OperationId,
        members: Vec<String>,
    },
    /// Answers `ListWorkspaces`: the workspaces under `root`, the folder that was listed.
    WorkspacesListed {
        id: OperationId,
        workspaces: Vec<WorkspaceRow>,
        root: String,
    },
    /// Answers `PlanWorkspace`: each member's branch plan and the problems that refuse the
    /// workspace. `path` is the folder the workspace would have.
    WorkspacePlanned {
        id: OperationId,
        path: String,
        members: Vec<MemberPlan>,
        problems: Vec<WorkspaceProblem>,
    },
    /// Answers `CreateWorkspace`: the workspace folder exists, with its members.
    WorkspaceCreated {
        id: OperationId,
        path: String,
    },
    /// Answers `InspectWorkspaceRemoval`: what would stop the removal; none when it would work.
    WorkspaceRemovalInspected {
        id: OperationId,
        path: String,
        blockers: Vec<RemovalBlocker>,
    },
    /// Answers `RemoveWorkspace`: the workspace folder is gone; the branches are kept.
    WorkspaceRemoved {
        id: OperationId,
        path: String,
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
    /// Answers `ListHookSections`: each section as `<scope> gitmaster.prepare-commit.<name>`.
    HookSectionsListed {
        id: OperationId,
        sections: Vec<String>,
    },
    /// Answers `ConvertHooksToWorkflows`. `written` holds the workflow files created; `skipped`
    /// holds each section left out with its reason; `sections` holds every section the person can
    /// now remove, with the scope the `git config --remove-section` command needs.
    HooksConverted {
        id: OperationId,
        written: Vec<String>,
        skipped: Vec<(String, String)>,
        sections: Vec<String>,
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
    /// A built-in workflow was copied to `path` for editing (`WF4b`); the copy is named `name`. The
    /// page selects it; the window does not reveal the folder.
    WorkflowCopied {
        id: OperationId,
        path: String,
        name: String,
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
    /// An event (or an observer of an operation) started for a version of the script `workflow`
    /// the person has not reviewed (`WF3b`, D155(3)). The run waits for the review: the window shows
    /// a notice with Review and Dismiss. A later request for the same workflow replaces it.
    /// `trigger` is the event's `on`, such as `commit.created`.
    WorkflowWantsReview {
        workflow: String,
        hash: String,
        trigger: String,
    },
    /// The pending request of the workflow `workflow` ended: it was reviewed and ran, dismissed, or
    /// its file changed (`WF3b`).
    WorkflowRequestDropped {
        workflow: String,
    },
    /// A `before.<op>` gate of the operation `id` is not reviewed, so the operation is vetoed
    /// (`WF3b`, D157(2)). The window offers Review for this version beside the veto. Sent just
    /// before the veto.
    WorkflowGateUnreviewed {
        id: OperationId,
        workflow: String,
        hash: String,
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
    /// The transform operation `id` ended (`WF3d`). `value` is the field's value when a transform
    /// produced one. `changed` is true when HEAD or the staged diff moved since the command started
    /// (`commit_message` only). A cancelled or superseded operation answers with no value.
    TransformFinished {
        id: OperationId,
        field: String,
        value: Option<String>,
        changed: bool,
    },
    /// A `before.<operation>` gate started for the operation `id`, which waits for it (`WF3c`,
    /// D157). `workflow` is the gate's name.
    GateStarted {
        id: OperationId,
        workflow: String,
    },
    /// The run waits at the `approve` state `state`; `summary` is what it asks. For the approval of
    /// `ctx.restack.apply` and `ctx.reparent.apply`, `plan` is the plan the call would move, so the
    /// window shows what is approved (`WF4c`).
    WorkflowApprovalRequested {
        run: i64,
        state: String,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<RestackPlan>,
    },
    /// The run's `ctx.restack.apply` ended `conflict` (`WF4c`): the run stopped on a restack
    /// conflict and waits for **Continue restack**. `plan` is the restack after the apply, with the
    /// blocked branch.
    WorkflowRestackConflict {
        run: i64,
        workflow: String,
        plan: RestackPlan,
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
    /// Answers `LoadReviewChange` for comment `comment`: the anchored lines in
    /// both snapshots, `None` when either snapshot is missing or the file is not
    /// UTF-8 text.
    ReviewChange {
        id: OperationId,
        comment: i64,
        path: String,
        change: Option<ReviewChange>,
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
    /// A workflow suggested the text of conflict hunk `index` (from 0) of the file `path`, for the
    /// read `read` (`WF3d`, D168). The suggestion is never applied by itself: the person chooses it.
    /// `workflow` is the last workflow whose text was kept. Sent only for a non-empty answer, and
    /// never for a read that a newer read, a write of the file or a cancel has superseded.
    HunkSuggested {
        read: OperationId,
        path: String,
        index: usize,
        text: String,
        workflow: String,
    },
    /// Answers `ForgetRecordedResolution`: the conflict markers are back in the
    /// working tree file.
    RecordedResolutionForgotten {
        id: OperationId,
        path: String,
    },
    /// Answers `ThreadsListed`. `total` counts every thread in the store, whatever the filter.
    ThreadsListed {
        id: OperationId,
        threads: Vec<ThreadSummary>,
        total: u32,
    },
    /// Answers `SearchThreads`, best match first. `has_more` and `next_cursor` page the
    /// hits (D147). `facets` is set when asked; `diagnosis` is set when a search with
    /// filters finds nothing: each filter that, removed alone, would bring hits back, with
    /// its count, most first.
    ThreadSearchResults {
        id: OperationId,
        hits: Vec<ThreadHit>,
        #[serde(default)]
        has_more: bool,
        #[serde(default)]
        next_cursor: Option<String>,
        #[serde(default)]
        facets: Option<SearchFacets>,
        #[serde(default)]
        diagnosis: Option<Vec<(String, u32)>>,
    },
    /// Answers `ReadThread`. Boxed: a thread's detail is the largest payload of the enum.
    ThreadRead {
        id: OperationId,
        detail: Box<ThreadDetail>,
    },
    /// Answers `ReadProvenance`: the commits, threads and evidence behind the lines.
    ThreadProvenance {
        id: OperationId,
        provenance: ProvenanceResult,
    },
    /// Answers `ReadThreadEdit`: the edit and where its file is now.
    ThreadEditRead {
        id: OperationId,
        edit: ThreadEdit,
        state: EditState,
    },
    /// Answers `ReadThreadMarkdown`: the same text `ExportThread` writes.
    ThreadMarkdown {
        id: OperationId,
        uid: String,
        markdown: String,
    },
    /// Answers `ExportThread` with the file written.
    ThreadExported {
        id: OperationId,
        path: String,
    },
    /// Answers `ScrubThreads`: how many texts, titles and notes changed.
    ThreadsScrubbed {
        id: OperationId,
        changed: u32,
    },
    /// The thread database changed, by a write of this engine or by new data the
    /// watcher saw. Carries no operation id: no command asked for it.
    ThreadsChanged,
    /// Answers `ListLabels`, by name.
    LabelsListed {
        id: OperationId,
        labels: Vec<Label>,
    },
    /// Answers `ReadLabel`.
    LabelRead {
        id: OperationId,
        detail: LabelDetail,
    },
    /// Answers `CreateLabel` and `AttachLabel` with the label as it now is (`label`), and
    /// `DeleteLabel` and `DetachLabel` with none. Every label write also sends
    /// `ThreadsChanged`, since thread rows show their labels.
    LabelsChanged {
        id: OperationId,
        label: Option<Label>,
    },
    /// Answers `LinkThreads` (`removed` false) and `RemoveThreadRelation` (`removed` true).
    ThreadRelationChanged {
        id: OperationId,
        relation: ThreadRelation,
        removed: bool,
    },
    /// Answers `ListMemories`, with the cursor of the next page when there is one. `contexts_read`
    /// is false when `contexts.db` exists but could not be read, and `note` says why the contexts
    /// were not used (an unreadable file, or a clone whose id clashes, D193 (4)). `proposed_total`
    /// is the exact number of proposed memories the repository sees (its own store and every visible
    /// context, each memory once), whatever the request's filters, scope, status, query and page
    /// are: the tab's badge. The memory text is untrusted data.
    MemoriesListed {
        id: OperationId,
        memories: Vec<Memory>,
        next_cursor: Option<String>,
        contexts_read: bool,
        note: Option<String>,
        #[serde(default)]
        proposed_total: u32,
        /// The accepted memories in the scope the request names, whatever its filters, query and page,
        /// and the characters of their text together (D201 (5)): the size line of the tab.
        #[serde(default)]
        accepted_total: u32,
        #[serde(default)]
        accepted_characters: u64,
    },
    /// Answers `ReadMemory`.
    MemoryRead {
        id: OperationId,
        memory: Memory,
    },
    /// Answers `ProposeMemory`, `AcceptMemory` and `RejectMemory` with the memory as it now is,
    /// and `DeleteMemory` with none. `created` is true only when `ProposeMemory` wrote a new row;
    /// false when it returned an existing duplicate.
    MemoryChanged {
        id: OperationId,
        memory: Option<Memory>,
        created: bool,
    },
    /// Answers `ListContexts`. `repo_id` is the open repository's id when it has one, and `clash`
    /// is the folder of the other clone when that id is held there (D193 (4)).
    ContextsListed {
        id: OperationId,
        contexts: Vec<Context>,
        repo_id: Option<String>,
        clash: Option<String>,
    },
    /// Answers `PreviewDeleteContext`. `sub_contexts` is how many contexts sit directly under
    /// `context` (a delete is refused while there are any). `memories` are the memories held alone,
    /// at most 50, each with the context's path as its scope; `total` is how many there are in all.
    /// The memory text is untrusted data.
    ContextDeletePreview {
        id: OperationId,
        context: i64,
        sub_contexts: u32,
        memories: Vec<Memory>,
        total: u32,
    },
    /// Answers the context writes with the context as it now is (none after a delete or a
    /// `RenewRepoId`), and the number of memories a delete removed.
    ContextsChanged {
        id: OperationId,
        context: Option<Context>,
        deleted_memories: u32,
    },
}

/// Reads a field that is present, `null` included, as `Some`: the inner value of the field is read
/// as it is, so `"parent": null` is `Some(None)`, a move to the root. An absent field is `None`
/// through `default`, which leaves the parent as it is.
fn present_option<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads::{AttachKind, EditChange, LinkOrigin, ThreadRelKind};
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
        let reply = serde_json::to_value(CommandKind::ReplyToReviewComment {
            id: 9,
            body: "why".into(),
        })
        .unwrap();
        assert_eq!(reply["type"], "reply_to_review_comment");
        assert_eq!(reply["id"], 9);
        assert_eq!(reply["body"], "why");
        let back: CommandKind = serde_json::from_value(reply).unwrap();
        assert_eq!(
            back,
            CommandKind::ReplyToReviewComment {
                id: 9,
                body: "why".into()
            }
        );
        let list = serde_json::to_value(CommandKind::ListReviewComments {
            author: ReviewAuthorFilter::Any,
        })
        .unwrap();
        assert_eq!(list["type"], "list_review_comments");
        assert_eq!(list["author"], "any");
        let bare: CommandKind =
            serde_json::from_value(serde_json::json!({ "type": "list_review_comments" })).unwrap();
        assert_eq!(
            bare,
            CommandKind::ListReviewComments {
                author: ReviewAuthorFilter::Human
            }
        );
        let flag = serde_json::to_value(CommandKind::CreateAgentReviewComment {
            path: "a.rs".to_owned(),
            anchor_text: "x".to_owned(),
            kind: ReviewKind::Flag,
            body: "guess".to_owned(),
            severity: Some(ReviewSeverity::Low),
        })
        .unwrap();
        assert_eq!(flag["type"], "create_agent_review_comment");
        assert_eq!(flag["kind"], "flag");
        assert_eq!(flag["severity"], "low");
        let change = serde_json::to_value(CommandKind::LoadReviewChange { id: 3 }).unwrap();
        assert_eq!(change["type"], "load_review_change");
        assert_eq!(change["id"], 3);
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

    #[test]
    fn repo_branches_wire_format_is_tagged() {
        let list = serde_json::to_value(CommandKind::ListRepoBranches {
            repo: "/home/me/src/billing".into(),
        })
        .unwrap();
        assert_eq!(list["type"], "list_repo_branches");
        assert_eq!(list["repo"], "/home/me/src/billing");
        let event = Event::RepoBranchesLoaded {
            id: OperationId(3),
            repo: "/home/me/src/billing".into(),
            branches: vec![BranchEntry {
                name: "origin/feat".into(),
                is_remote: true,
                is_current: false,
                upstream: None,
                author_email: None,
            }],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "repo_branches_loaded");
        assert_eq!(json["repo"], "/home/me/src/billing");
        assert_eq!(json["branches"][0]["is_remote"], true);
    }

    fn thread_summary(uid: &str) -> ThreadSummary {
        ThreadSummary {
            uid: uid.into(),
            harness: "claude-code".into(),
            harness_version: None,
            session_id: "s-1".into(),
            worktree: "/r".into(),
            branch: Some("main".into()),
            model: None,
            title: "retry with backoff".into(),
            note: String::new(),
            starred: false,
            tags: vec!["flaky".into()],
            started_at: 1,
            last_event_at: 2,
            ended_at: None,
            entry_count: 3,
            edit_count: 1,
            redaction_count: 0,
            workspace: None,
        }
    }

    #[test]
    fn thread_commands_wire_format_is_tagged() {
        let list = serde_json::to_value(CommandKind::ListThreads {
            filter: ThreadFilter::default(),
            before_at: Some(5),
            before_uid: Some("ab".into()),
            limit: 20,
        })
        .unwrap();
        assert_eq!(list["type"], "list_threads");
        assert_eq!(list["before_at"], 5);
        let back: CommandKind = serde_json::from_value(list).unwrap();
        assert_eq!(
            back,
            CommandKind::ListThreads {
                filter: ThreadFilter::default(),
                before_at: Some(5),
                before_uid: Some("ab".into()),
                limit: 20,
            }
        );
        let link = serde_json::to_value(CommandKind::LinkThread {
            uid: "0123456789abcdef".into(),
            link: ThreadLink {
                member: None,
                kind: LinkKind::Commit,
                reference: "a".repeat(40),
                origin: LinkOrigin::Human,
                resolved: None,
                state: None,
            },
        })
        .unwrap();
        assert_eq!(link["type"], "link_thread");
        assert_eq!(link["link"]["kind"], "commit");
        assert_eq!(link["link"]["origin"], "human");
        let unlink = serde_json::to_value(CommandKind::UnlinkThread {
            uid: "u".into(),
            kind: LinkKind::File,
            reference: "src/a.rs".into(),
        })
        .unwrap();
        assert_eq!(unlink["type"], "unlink_thread");
        assert_eq!(unlink["kind"], "file");
        let scrub = serde_json::to_value(CommandKind::ScrubThreads).unwrap();
        assert_eq!(scrub["type"], "scrub_threads");
        let markdown = serde_json::to_value(CommandKind::ReadThreadMarkdown {
            uid: "0123456789abcdef".into(),
        })
        .unwrap();
        assert_eq!(markdown["type"], "read_thread_markdown");
        assert_eq!(markdown["uid"], "0123456789abcdef");
        let back: CommandKind = serde_json::from_value(markdown).unwrap();
        assert!(matches!(back, CommandKind::ReadThreadMarkdown { .. }));
    }

    #[test]
    fn label_commands_wire_format_is_tagged() {
        let attach = serde_json::to_value(CommandKind::AttachLabel {
            label: "flaky".into(),
            kind: AttachKind::Commit,
            reference: "a".repeat(40),
        })
        .unwrap();
        assert_eq!(attach["type"], "attach_label");
        assert_eq!(attach["kind"], "commit");
        assert_eq!(attach["ref"], "a".repeat(40));
        let back: CommandKind = serde_json::from_value(attach).unwrap();
        assert!(matches!(
            back,
            CommandKind::AttachLabel {
                kind: AttachKind::Commit,
                ..
            }
        ));
        let link = serde_json::to_value(CommandKind::LinkThreads {
            from: "a".into(),
            to: "b".into(),
            kind: ThreadRelKind::Continues,
        })
        .unwrap();
        assert_eq!(link["type"], "link_threads");
        assert_eq!(link["kind"], "continues");
        let list = serde_json::to_value(CommandKind::ListLabels {
            query: String::new(),
            limit: 20,
        })
        .unwrap();
        assert_eq!(list["type"], "list_labels");
        let detach = serde_json::to_value(CommandKind::DetachLabel {
            label: "flaky".into(),
            kind: AttachKind::Thread,
            reference: "u".into(),
        })
        .unwrap();
        assert_eq!(detach["type"], "detach_label");
        assert_eq!(detach["ref"], "u");
    }

    #[test]
    fn memories_listed_carries_the_exact_proposed_total_and_an_old_payload_reads_zero() {
        let listed = Event::MemoriesListed {
            id: OperationId(7),
            memories: Vec::new(),
            next_cursor: None,
            contexts_read: true,
            note: None,
            proposed_total: 4,
            accepted_total: 3,
            accepted_characters: 3_100,
        };
        let json = serde_json::to_value(&listed).unwrap();
        assert_eq!(json["type"], "memories_listed");
        assert_eq!(json["proposed_total"], 4);
        assert_eq!(json["accepted_total"], 3);
        assert_eq!(json["accepted_characters"], 3_100);
        let back: Event = serde_json::from_value(json).unwrap();
        assert_eq!(back, listed);

        // A payload written before the field existed still parses, with no proposals counted.
        let old = serde_json::json!({
            "type": "memories_listed",
            "id": 7,
            "memories": [],
            "next_cursor": null,
            "contexts_read": true,
            "note": null,
        });
        let back: Event = serde_json::from_value(old).unwrap();
        assert!(matches!(
            back,
            Event::MemoriesListed {
                proposed_total: 0,
                ..
            }
        ));
    }

    #[test]
    fn context_delete_preview_round_trips_with_its_total() {
        let preview = Event::ContextDeletePreview {
            id: OperationId(9),
            context: 3,
            sub_contexts: 1,
            memories: Vec::new(),
            total: 61,
        };
        let json = serde_json::to_value(&preview).unwrap();
        assert_eq!(json["type"], "context_delete_preview");
        assert_eq!(json["context"], 3);
        assert_eq!(json["sub_contexts"], 1);
        assert_eq!(json["total"], 61);
        let back: Event = serde_json::from_value(json).unwrap();
        assert_eq!(back, preview);
    }

    #[test]
    fn memory_commands_wire_format_is_tagged() {
        let propose = serde_json::to_value(CommandKind::ProposeMemory {
            kind: MemoryKind::Rule,
            text: "Run the checks".into(),
            sources: vec![MemorySourceInput {
                kind: crate::threads::MemorySourceKind::File,
                reference: "src/a.rs".into(),
                entry_from: None,
                entry_to: None,
            }],
            context: None,
            basis: MemoryBasis::Tested,
        })
        .unwrap();
        assert_eq!(propose["type"], "propose_memory");
        assert!(
            propose.get("context").is_none(),
            "an absent context is not written"
        );
        assert_eq!(propose["kind"], "rule");
        assert_eq!(propose["basis"], "tested");
        assert_eq!(propose["sources"][0]["kind"], "file");
        assert_eq!(propose["sources"][0]["ref"], "src/a.rs");
        let back: CommandKind = serde_json::from_value(propose.clone()).unwrap();
        assert!(matches!(
            back,
            CommandKind::ProposeMemory {
                basis: MemoryBasis::Tested,
                ..
            }
        ));
        let mut old = propose;
        old.as_object_mut().unwrap().remove("basis");
        let old: CommandKind = serde_json::from_value(old).unwrap();
        assert!(matches!(
            old,
            CommandKind::ProposeMemory {
                basis: MemoryBasis::Observed,
                ..
            }
        ));
        let list = serde_json::to_value(CommandKind::ListMemories {
            query: String::new(),
            kind: None,
            status: Some(MemoryStatus::Proposed),
            path: None,
            author: None,
            scope: Some("Billing".into()),
            limit: 20,
            cursor: None,
        })
        .unwrap();
        assert_eq!(list["type"], "list_memories");
        assert_eq!(list["scope"], "Billing");
        let moved = serde_json::to_value(CommandKind::MoveMemoryToContext {
            uid: "0123456789abcdef".into(),
            context: 4,
        })
        .unwrap();
        assert_eq!(moved["type"], "move_memory_to_context");
        assert_eq!(moved["context"], 4);
        let back: CommandKind = serde_json::from_value(moved).unwrap();
        assert!(matches!(
            back,
            CommandKind::MoveMemoryToContext { context: 4, .. }
        ));
        assert_eq!(list["status"], "proposed");
        let accept = serde_json::to_value(CommandKind::AcceptMemory {
            uid: "0123456789abcdef".into(),
            text: None,
            supersedes: vec!["fedcba9876543210".into()],
        })
        .unwrap();
        assert_eq!(accept["type"], "accept_memory");
        assert_eq!(accept["uid"], "0123456789abcdef");
        assert_eq!(accept["supersedes"][0], "fedcba9876543210");
        let back: CommandKind = serde_json::from_value(accept).unwrap();
        assert!(matches!(back, CommandKind::AcceptMemory { .. }));
        let read = serde_json::to_value(CommandKind::ReadMemory {
            uid: "0123456789abcdef".into(),
        })
        .unwrap();
        assert_eq!(
            read,
            serde_json::json!({ "type": "read_memory", "uid": "0123456789abcdef" })
        );
    }

    #[test]
    fn context_commands_wire_format_is_tagged_and_a_null_parent_is_the_root() {
        let list = serde_json::to_value(CommandKind::ListContexts { repo: true }).unwrap();
        assert_eq!(
            list,
            serde_json::json!({ "type": "list_contexts", "repo": true })
        );
        let preview = serde_json::to_value(CommandKind::PreviewDeleteContext { id: 4 }).unwrap();
        assert_eq!(
            preview,
            serde_json::json!({ "type": "preview_delete_context", "id": 4 })
        );
        let back: CommandKind = serde_json::from_value(preview).unwrap();
        assert_eq!(back, CommandKind::PreviewDeleteContext { id: 4 });
        let create = serde_json::to_value(CommandKind::CreateContext {
            name: "Billing".into(),
            parent: Some(3),
        })
        .unwrap();
        assert_eq!(create["type"], "create_context");
        assert_eq!(create["parent"], 3);
        assert_eq!(
            serde_json::to_value(CommandKind::RenewRepoId).unwrap(),
            serde_json::json!({ "type": "renew_repo_id" })
        );

        // An absent parent leaves it as it is; a null one moves the context to the root.
        let absent: CommandKind = serde_json::from_value(
            serde_json::json!({ "type": "update_context", "id": 2, "name": "X" }),
        )
        .unwrap();
        assert!(matches!(
            absent,
            CommandKind::UpdateContext { parent: None, .. }
        ));
        let root: CommandKind = serde_json::from_value(
            serde_json::json!({ "type": "update_context", "id": 2, "parent": null }),
        )
        .unwrap();
        assert!(matches!(
            root,
            CommandKind::UpdateContext {
                parent: Some(None),
                ..
            }
        ));
        let under: CommandKind = serde_json::from_value(
            serde_json::json!({ "type": "update_context", "id": 2, "parent": 1 }),
        )
        .unwrap();
        assert!(matches!(
            under,
            CommandKind::UpdateContext {
                parent: Some(Some(1)),
                ..
            }
        ));
        let kept = serde_json::to_value(CommandKind::UpdateContext {
            id: 2,
            name: None,
            parent: None,
        })
        .unwrap();
        assert!(kept.get("parent").is_none(), "{kept}");
    }

    #[test]
    fn thread_events_round_trip_through_json() {
        let listed = Event::ThreadsListed {
            id: OperationId(4),
            threads: vec![thread_summary("0123456789abcdef")],
            total: 7,
        };
        let json = serde_json::to_value(&listed).unwrap();
        assert_eq!(json["type"], "threads_listed");
        assert_eq!(json["total"], 7);
        let back: Event = serde_json::from_value(json).unwrap();
        assert_eq!(back, listed);

        let edit = Event::ThreadEditRead {
            id: OperationId(5),
            edit: ThreadEdit {
                member: None,
                id: 9,
                entry_id: 3,
                path: "src/a.rs".into(),
                old_path: None,
                change: EditChange::Modify,
                patch: "@@ -1 +1 @@\n-a\n+b\n".into(),
                added: 1,
                removed: 1,
                pre_oid: None,
                post_oid: Some("b".repeat(40)),
                from_input: false,
                at: 1,
            },
            state: EditState::InWorkingTree,
        };
        let json = serde_json::to_value(&edit).unwrap();
        assert_eq!(json["type"], "thread_edit_read");
        assert_eq!(json["state"], "in_working_tree");
        let back: Event = serde_json::from_value(json).unwrap();
        assert_eq!(back, edit);

        let markdown = Event::ThreadMarkdown {
            id: OperationId(6),
            uid: "0123456789abcdef".into(),
            markdown: "# Title\n".into(),
        };
        let json = serde_json::to_value(&markdown).unwrap();
        assert_eq!(json["type"], "thread_markdown");
        assert_eq!(json["markdown"], "# Title\n");
        let back: Event = serde_json::from_value(json).unwrap();
        assert_eq!(back, markdown);

        let changed = serde_json::to_value(Event::ThreadsChanged).unwrap();
        assert_eq!(changed, serde_json::json!({ "type": "threads_changed" }));
        let back: Event = serde_json::from_value(changed).unwrap();
        assert_eq!(back, Event::ThreadsChanged);
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

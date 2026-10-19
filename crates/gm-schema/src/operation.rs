use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::diff::{DiffSide, FileDiff, HunkId, StageTarget};
use crate::history::{CommitDetail, CommitFile, History};
use crate::hook::{HookFailure, HookInfo};
use crate::status::{
    BranchEntry, BranchFailure, MergedBranch, PruneCandidate, RepoStatus, WorktreeEntry,
    WorktreeInspection,
};
use crate::workflow::{WorkflowEnd, WorkflowInfo, WorkflowRunStatus};

/// The refusal of `DeleteMergedBranches` when the merged list no longer matches
/// the one the human confirmed. The UI matches it to list the branches again.
pub const MERGED_LIST_CHANGED: &str =
    "the merged branches changed since they were listed; list them again";

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
    /// Lists the hooks read from config, with their state (docs/03).
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
    /// Copies the built-in template `template` into the user's workflows
    /// directory under a file name not taken yet; never into a repository.
    /// Answers `WorkflowCreated`. Human only.
    CreateWorkflowFromTemplate {
        template: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Command {
    pub actor: Actor,
    pub kind: CommandKind,
}

/// `Requested → Gating → AwaitingApproval → Executing → Done | Failed |
/// Vetoed | Cancelled` (docs/06, rule 2).
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
    /// Answers `ListWorkflows`. `ignored` holds each directory that was not
    /// read, as `<path>: <reason>`.
    WorkflowsListed {
        id: OperationId,
        workflows: Vec<WorkflowInfo>,
        ignored: Vec<String>,
        /// The user's workflows directory, whether or not it exists yet.
        user_dir: Option<String>,
        /// Names of the built-in templates `CreateWorkflowFromTemplate` takes.
        templates: Vec<String>,
    },
    /// A template was copied to `path`; the workflow inside is named `name`.
    WorkflowCreated {
        id: OperationId,
        path: String,
        name: String,
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

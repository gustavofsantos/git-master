//! Types shared by the UI, the MCP server and (later) hooks: commands going in,
//! events coming out, and the repository status they carry.

mod diff;
mod history;
mod hook;
mod operation;
mod status;
mod workflow;

pub use diff::{DiffLine, DiffSide, FileDiff, Hunk, HunkId, LineKind, StageTarget};
pub use history::{
    CommitDetail, CommitFile, CommitSummary, DETAIL_FILE_LIMIT, HISTORY_PAGE, History, RefKind,
    RefLabel,
};
pub use hook::{
    CONTRACT_VERSION, HookFailure, HookInfo, HookInput, HookOutput, HookPayload, HookRepo,
    HookResult, HookScope, HookState, PREPARE_COMMIT,
};
pub use operation::{
    Actor, Command, CommandKind, Event, IllegalTransition, MERGED_LIST_CHANGED, OperationId,
    OperationState, WorktreeMode,
};
pub use status::{
    Blocker, BranchEntry, BranchFailure, BranchInfo, Change, MergedBranch, PruneCandidate,
    RepoStatus, StatusEntry, WorktreeEntry, WorktreeInspection,
};
pub use workflow::{
    WorkflowDryRun, WorkflowDryStep, WorkflowEnd, WorkflowInfo, WorkflowInput, WorkflowRunDetail,
    WorkflowRunStatus, WorkflowRunSummary, WorkflowStepRecord,
};

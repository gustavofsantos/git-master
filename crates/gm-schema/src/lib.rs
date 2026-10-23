//! Types shared by the UI, the MCP server and (later) hooks: commands going in,
//! events coming out, and the repository status they carry.

mod diff;
mod files;
mod grep;
mod history;
mod hook;
mod move_changes;
mod operation;
mod reflog;
mod review;
mod stash;
mod status;
mod workflow;

pub use diff::{DiffLine, DiffSide, FileDiff, Hunk, HunkId, LineKind, StageTarget};
pub use files::{
    DirEntry, DirListing, FILE_HIT_LIMIT, FILE_VIEW_LIMIT, FileBody, FileHit, FileSearch, FileView,
};
pub use grep::{
    GREP_FILE_LIMIT, GREP_LINE_LIMIT, GREP_TOTAL_LIMIT, GrepFile, GrepLine, GrepOptions,
    GrepResult, GrepScope,
};
pub use history::{
    CommitDetail, CommitFile, CommitSummary, DETAIL_FILE_LIMIT, HISTORY_PAGE, History, RefKind,
    RefLabel,
};
pub use hook::{
    CONTRACT_VERSION, HookFailure, HookInfo, HookInput, HookOutput, HookPayload, HookRepo,
    HookResult, HookScope, HookState, PREPARE_COMMIT,
};
pub use move_changes::{MoveBlocker, MoveOutcome, MovePreview, MoveTarget};
pub use operation::{
    Actor, Command, CommandKind, Event, IllegalTransition, MERGED_LIST_CHANGED, OperationId,
    OperationState, STASH_LIST_CHANGED, WorktreeMode,
};
pub use reflog::{REFLOG_PAGE, ReflogEntry};
pub use review::{ReviewAnchor, ReviewComment, ReviewLocation, ReviewStatus};
pub use stash::StashEntry;
pub use status::{
    Blocker, BranchEntry, BranchFailure, BranchInfo, Change, MergedBranch, PruneCandidate,
    RepoStatus, StatusEntry, WorktreeEntry, WorktreeInspection,
};
pub use workflow::{
    WorkflowCallRecord, WorkflowDiagnostic, WorkflowDryCall, WorkflowDryRun, WorkflowEnd,
    WorkflowInfo, WorkflowInput, WorkflowRunDetail, WorkflowRunStatus, WorkflowRunSummary,
};

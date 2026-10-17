//! Types shared by the UI, the MCP server and (later) hooks: commands going in,
//! events coming out, and the repository status they carry.

mod diff;
mod hook;
mod operation;
mod status;

pub use diff::{DiffLine, DiffSide, FileDiff, Hunk, HunkId, LineKind, StageTarget};
pub use hook::{
    CONTRACT_VERSION, HookFailure, HookInfo, HookInput, HookOutput, HookPayload, HookRepo,
    HookResult, HookScope, HookState, PREPARE_COMMIT,
};
pub use operation::{
    Actor, Command, CommandKind, Event, IllegalTransition, OperationId, OperationState,
};
pub use status::{BranchEntry, BranchInfo, Change, RepoStatus, StatusEntry, WorktreeEntry};

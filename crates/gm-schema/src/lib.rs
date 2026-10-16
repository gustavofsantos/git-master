//! Types shared by the UI, the MCP server and (later) hooks: commands going in,
//! events coming out, and the repository status they carry.

mod diff;
mod operation;
mod status;

pub use diff::{DiffLine, DiffSide, FileDiff, Hunk, HunkId, LineKind, StageTarget};
pub use operation::{
    Actor, Command, CommandKind, Event, IllegalTransition, OperationId, OperationState,
};
pub use status::{BranchEntry, BranchInfo, Change, RepoStatus, StatusEntry};

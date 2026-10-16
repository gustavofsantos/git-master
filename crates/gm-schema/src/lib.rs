//! Types shared by the UI, the MCP server and (later) hooks: commands going in,
//! events coming out, and the repository status they carry.

mod operation;
mod status;

pub use operation::{
    Actor, Command, CommandKind, Event, IllegalTransition, OperationId, OperationState,
};
pub use status::{BranchInfo, Change, RepoStatus, StatusEntry};

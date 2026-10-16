//! The engine: commands in, events out, no UI and no real repository needed.
//! Git is reached through `GitPort`; `git::SubprocessGit` is the first adapter.

pub mod diff;
mod engine;
pub mod fake;
pub mod git;
mod port;
mod runner;
pub mod stage_math;

pub use engine::{Engine, LogEntry};
pub use port::{GitError, GitPort};
pub use runner::{EngineHandle, spawn};

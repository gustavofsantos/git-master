//! The engine: commands in, events out, no UI and no real repository needed.
//! Git is reached through `GitPort`; `git::SubprocessGit` is the first adapter.

pub mod command_log;
pub mod diff;
mod engine;
pub mod fake;
pub mod git;
pub mod hook;
mod port;
mod runner;
pub mod stage_math;
pub mod watch;

pub use engine::{Engine, LogEntry};
pub use port::{GitError, GitPort};
pub use runner::{EngineHandle, spawn, spawn_with};

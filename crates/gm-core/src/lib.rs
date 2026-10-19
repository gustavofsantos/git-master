//! The engine: commands in, events out, no UI and no real repository needed.
//! Git is reached through `GitPort`; `git::SubprocessGit` is the first adapter.

pub mod branch;
pub mod command_log;
pub mod commit_detail;
pub mod commit_diff;
pub mod diff;
mod engine;
pub mod fake;
pub mod git;
mod history;
pub mod hook;
mod port;
pub mod remote;
mod runner;
pub mod stage_math;
pub mod watch;
pub mod worktree;

pub use engine::{Engine, LogEntry};
pub use history::{HistoryJob, HistoryOutcome, HistoryPage, HistoryReport, HistoryStream};
pub use port::{DetailRead, GitError, GitPort, RemoteRun};
pub use runner::{EngineHandle, spawn, spawn_with};

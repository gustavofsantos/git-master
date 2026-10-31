//! The engine: commands in, events out, no UI and no real repository needed.
//! Git is reached through `GitPort`; `git::SubprocessGit` is the first adapter.

pub mod branch;
pub mod command_log;
pub mod commit_detail;
pub mod commit_diff;
pub mod conflict;
mod conflict_sides;
pub mod diff;
mod engine;
pub mod fake;
pub mod file_index;
mod files;
pub mod git;
pub mod grep;
mod history;
pub mod hook;
pub mod move_changes;
mod move_exec;
mod move_preview;
pub mod path_match;
mod port;
mod reflog;
pub mod remote;
pub mod review;
mod review_session;
pub mod review_store;
mod runner;
pub mod script;
mod script_session;
pub mod stack;
mod stack_session;
pub mod stage_math;
mod stash;
pub mod syntax;
pub mod threads;
mod threads_session;
pub mod watch;
mod workflow_session;
pub mod worktree;

pub use engine::{Engine, LogEntry};
pub use files::{
    HighlightJob, HighlightReport, IndexJob, IndexReport, ReadJob, ReadReport, SearchJob,
    SearchReport, TextJob, TextReport,
};
pub use history::{HistoryJob, HistoryOutcome, HistoryPage, HistoryReport, HistoryStream};
pub use port::{
    DetailRead, DiscardOutcome, FileListRead, FileMerge, GitError, GitPort, GrepRead, RemoteRun,
    TreeMerge,
};
pub use runner::{EngineHandle, spawn, spawn_with};
pub use script_session::{EffectRequest, ScriptLink};

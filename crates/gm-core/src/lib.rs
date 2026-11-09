//! The engine: commands in, events out, no UI and no real repository needed.
//! Git is reached through `GitPort`; `git::SubprocessGit` is the first adapter.

pub mod blame;
pub mod branch;
pub mod change;
pub mod combination;
pub mod command_log;
pub mod commit_detail;
pub mod commit_diff;
pub mod conflict;
mod conflict_sides;
pub mod contexts;
mod contexts_session;
pub mod diff;
mod engine;
pub mod fake;
pub mod file_index;
mod files;
mod gate_check;
pub mod gates;
pub mod git;
pub mod glob;
pub mod grep;
mod history;
mod memory_scope;
pub mod move_changes;
mod move_exec;
mod move_preview;
pub mod path_match;
pub mod paths;
mod port;
mod proposals;
mod reflog;
pub mod remote;
pub mod restack;
pub mod restack_resolutions;
mod restack_session;
pub mod review;
mod review_session;
pub mod review_store;
pub mod reviewers;
mod runner;
pub mod script;
mod script_session;
mod script_stacks;
mod script_verdicts;
pub mod settings;
pub mod stack;
mod stack_session;
pub mod stage_math;
mod stash;
mod submodule;
mod submodule_files;
pub mod syntax;
pub mod threads;
mod threads_session;
pub mod verdicts;
pub mod watch;
mod workflow_session;
pub mod workspace;
pub mod workspace_cli;
pub mod workspace_comments;
pub mod workspace_defs;
mod workspace_pointer;
pub mod worktree;
pub mod worktree_cli;
pub mod worktree_include;

pub use engine::{Engine, GateInputs, LogEntry};
pub use files::{
    HighlightJob, HighlightReport, IndexJob, IndexReport, ReadJob, ReadReport, SearchJob,
    SearchReport, TextJob, TextReport,
};
pub use history::{HistoryJob, HistoryOutcome, HistoryPage, HistoryReport, HistoryStream};
pub use port::{
    BlameRead, CommitSnapshot, DetailRead, DiscardOutcome, FileListRead, FileMerge, GitError,
    GitPort, GrepRead, HistoryRevs, RefMove, RemoteRun, ReviewSide, TreeMerge,
};
pub use runner::{EngineHandle, spawn, spawn_app, spawn_with};
pub use script_session::{EffectRequest, ScriptJob, ScriptLink, ScriptReport};
pub use threads_session::{ProvenanceJob, ProvenanceReport};

//! Threads: capture of agent sessions into `.git/gitmaster/threads.db` (design/threads.md).
//! Harness-specific shapes stop at `harness`; everything else works on canonical events.

pub mod cli;
pub mod config;
pub mod harness;
pub mod pipeline;
pub mod provenance;
pub mod query;
pub mod redact;
pub mod store;
pub mod time;
pub mod when;

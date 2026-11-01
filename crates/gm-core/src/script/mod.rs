//! The workflow script engine (design/workflow-scripts.md, D98 to D104): a Lua script runs in a
//! sandbox, and `ctx` is its only authority. The VM sits behind the `ScriptEngine` port; the host
//! side (`run`) journals every call, applies the approval rules and runs the effects.

pub mod api_docs;
pub mod args;
pub mod catalog;
pub mod cli;
pub mod dry;
pub mod exec;
pub mod fake;
pub mod input;
pub mod inspect;
pub mod journal;
pub mod loader;
pub mod luau;
pub mod meta;
pub mod policy;
pub mod port;
pub mod run;
pub mod surface;
pub mod templates;
pub mod triggers;
pub mod trust;

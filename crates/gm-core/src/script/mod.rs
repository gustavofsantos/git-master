//! The workflow script engine (design/workflow-scripts.md, D98 to D104): a Lua script runs in a
//! sandbox, and `ctx` is its only authority. The VM sits behind the `ScriptEngine` port; the host
//! side (`run`) journals every call, applies the approval rules and runs the effects. Built next
//! to the TOML engine in `workflow`, which WS2 removes.

pub mod fake;
pub mod journal;
pub mod luau;
pub mod port;
pub mod surface;

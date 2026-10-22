//! The script engine port (design/workflow-scripts.md, D98, D99, D104). A VM adapter implements
//! `ScriptEngine`; the host implements `CtxHost`. The script reaches the host only through
//! `CtxHost::call`: that is the one effect boundary, so the journal, the approval rules and the
//! budgets live on the host side and a VM cannot skip them.
//!
//! Values cross the boundary as `serde_json::Value`, so the host never sees a VM type.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::Value;

use super::surface::CtxSurface;

/// The budgets of one run (D99). Hitting one ends the run as failed, with the reason.
/// The defaults are the starting values of spike S-D (D104), to be tuned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budgets {
    /// Interrupt points of the VM: loop back-edges and calls.
    pub steps: u64,
    /// Time the script itself runs. Time spent inside a host call (an approval, a wait, a process,
    /// a Git operation) is not counted: those calls have their own timeouts.
    pub script_time: Duration,
    /// Bytes the VM may allocate.
    pub memory: usize,
    /// The longest string a script can build with `string.rep`; other concatenation is capped by
    /// `memory`.
    pub string_bytes: usize,
    /// Calls on `ctx`, including the helpers. Replaces the 1000-transition ceiling of D78.
    pub ctx_calls: u32,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            steps: 20_000_000,
            script_time: Duration::from_secs(2),
            memory: 64 * 1024 * 1024,
            string_bytes: 16 * 1024 * 1024,
            ctx_calls: 1000,
        }
    }
}

/// Which budget ended a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetKind {
    Steps,
    Time,
    Memory,
    StringSize,
    StackDepth,
    CtxCalls,
}

impl BudgetKind {
    /// The words the run's end reason uses.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Steps => "instruction budget",
            Self::Time => "time budget",
            Self::Memory => "memory budget",
            Self::StringSize => "string size budget",
            Self::StackDepth => "call depth budget",
            Self::CtxCalls => "ctx call budget",
        }
    }
}

/// Why a run failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// A syntax or runtime error of the script, or a malformed module. The message carries the
    /// chunk name and the line when the VM knows them.
    Script(String),
    Budget(BudgetKind),
    /// A `ctx` call ended with an outcome other than `done`, outside `ctx.try`.
    Outcome {
        func: String,
        outcome: String,
        message: String,
    },
    /// `ctx.fail(reason)`.
    Reason(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Script(message) => f.write_str(message),
            Self::Budget(kind) => write!(f, "{} exceeded", kind.label()),
            Self::Outcome {
                func,
                outcome,
                message,
            } => {
                if message.is_empty() {
                    write!(f, "ctx.{func} ended with outcome {outcome}")
                } else {
                    write!(f, "ctx.{func} ended with outcome {outcome}: {message}")
                }
            }
            Self::Reason(reason) => f.write_str(reason),
        }
    }
}

/// How a run ended, as the VM reports it.
#[derive(Clone, Debug, PartialEq)]
pub enum RunEnd {
    /// `execute` returned. The value is the JSON of what it returned (`Null` for nothing); it is
    /// the field value of a `transform` run.
    Returned(Value),
    /// `ctx.block(reason)`.
    Blocked(String),
    Failed(Failure),
    /// The cancel flag was set, or a host call reported the cancellation.
    Cancelled,
}

/// What the host answers to one `ctx` call.
#[derive(Clone, Debug, PartialEq)]
pub enum HostReply {
    /// The call ended `done` (or, for `approve` and `wait`, with a value the script reads). The
    /// value becomes the Lua result.
    Value(Value),
    /// The call ended with an outcome other than `done`. Outside `ctx.try` the run stops
    /// with `Failure::Outcome` and a `pcall` cannot catch it; inside `ctx.try` the script gets
    /// `{ outcome = ..., <fields of value> }`.
    Outcome {
        outcome: String,
        value: Value,
        message: String,
    },
    /// The arguments are wrong. A plain Lua error: `pcall` can catch it.
    Error(String),
    /// The call ended the run. The VM latches it, so no code of the script runs after it.
    End(HostEnd),
}

/// A call that ends the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEnd {
    /// `ctx.block(reason)`.
    Blocked(String),
    /// `ctx.fail(reason)`.
    Failed(String),
    /// The run was cancelled while the call was waiting.
    Cancelled,
}

/// The host side of `ctx`. One value per run; called on the run's thread.
pub trait CtxHost {
    /// One call on `ctx`. `func` is the dotted name without `ctx.`, such as `branch.create`, or
    /// one of the helpers `now`, `random`, `block` and `fail`. `args` is a JSON object keyed by the
    /// catalog field names: a positional call such as `ctx.exec(argv, opts)` arrives as
    /// `{ "argv": [...], ...opts }` (see `CtxSurface`).
    fn call(&mut self, func: &str, args: Value) -> HostReply;
}

/// Everything the VM needs for one run.
pub struct RunRequest<'a> {
    /// The name errors cite, such as the file name.
    pub chunk: &'a str,
    pub source: &'a str,
    /// The `input` value: `inputs`, `trigger` and `repo`.
    pub input: &'a Value,
    pub surface: &'a CtxSurface,
    pub budgets: Budgets,
    /// Set from another thread to cancel the run. The VM checks it at every interrupt point. A
    /// host call that blocks has to wake on its own (see `run`).
    pub cancel: &'a AtomicBool,
}

/// One problem found in a script, with the line it points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptDiagnostic {
    /// 1-based; 0 when the problem has no single place.
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ScriptDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// A VM that runs workflow scripts in a sandbox (D99). It has no ambient authority: no `io`, `os`,
/// `debug`, `require`, `load`, `string.dump`, no clock and no randomness of its own, and a fresh
/// read-only environment for each call.
pub trait ScriptEngine: Send + Sync {
    /// Compiles the script and checks that it is a module that returns a table with `execute`
    /// (a function). Runs no script logic beyond loading the chunk.
    fn check(&self, chunk: &str, source: &str) -> Result<(), Vec<ScriptDiagnostic>>;

    /// Reads `M.meta` as JSON, in a sandbox without `ctx` and without any host function. Loading
    /// the chunk runs its top level under the budgets; `execute` is not called.
    fn read_meta(&self, chunk: &str, source: &str) -> Result<Value, Vec<ScriptDiagnostic>>;

    /// Runs `execute(ctx, input)` on the calling thread, in a new VM. Never panics on a script
    /// error: every end is a `RunEnd`.
    fn run(&self, request: &RunRequest<'_>, host: &mut dyn CtxHost) -> RunEnd;
}

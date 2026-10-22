//! The VM of one run: the sandbox (D99), the budgets enforced from the interrupt hook, and the
//! latch. A budget end or a host end is latched once. From then on every interrupt point and
//! every host entry raises again, and the `pcall`-family wrappers re-raise, so a script cannot
//! catch the end and carry on (spike S-D).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mlua::{
    Error as LuaError, Function, Lua, MultiValue, Result as LuaResult, Table, Value, VmState,
};
use serde_json::Value as Json;

use crate::script::port::{BudgetKind, Budgets, Failure, RunEnd};

/// Headroom below the memory budget at which the hook ends the run before the allocator does.
const MEMORY_SLACK: usize = 256 * 1024;
/// Memory is sampled every this many interrupt points (`used_memory` takes a lock).
const SAMPLE_EVERY: u64 = 1024;
/// Script time is read every this many interrupt points.
const CLOCK_EVERY: u64 = 256;

/// The globals a script does not get (D99). Removed before `sandbox(true)`, which makes the
/// remaining globals read-only. `io`, `package`, `loadfile` and `dofile` are absent from Luau.
const REMOVE_GLOBALS: &[&str] = &[
    "os",
    "io",
    "debug",
    "package",
    "require",
    "load",
    "loadstring",
    "dofile",
    "loadfile",
    "collectgarbage",
    "gcinfo",
    "print",
    "warn",
    "getfenv",
    "setfenv",
    "newproxy",
];

/// Library fields a script does not get. Both are sources of nondeterminism; `ctx.now()` and
/// `ctx.random()` replace them.
const REMOVE_FIELDS: &[(&str, &str)] = &[
    ("math", "random"),
    ("math", "randomseed"),
    ("string", "dump"),
];

/// Why a run ended early, latched once for the run.
#[derive(Clone, Debug)]
pub(super) enum Latch {
    Budget(BudgetKind),
    Outcome {
        func: String,
        outcome: String,
        message: String,
    },
    /// `ctx.fail(reason)`.
    Reason(String),
    /// `ctx.block(reason)`.
    Blocked(String),
    Cancelled,
}

impl Latch {
    /// The end the run reports.
    pub(super) fn end(&self) -> RunEnd {
        match self {
            Self::Budget(kind) => RunEnd::Failed(Failure::Budget(*kind)),
            Self::Outcome {
                func,
                outcome,
                message,
            } => RunEnd::Failed(Failure::Outcome {
                func: func.clone(),
                outcome: outcome.clone(),
                message: message.clone(),
            }),
            Self::Reason(reason) => RunEnd::Failed(Failure::Reason(reason.clone())),
            Self::Blocked(reason) => RunEnd::Blocked(reason.clone()),
            Self::Cancelled => RunEnd::Cancelled,
        }
    }

    /// The text of the error that carries the latch through the script.
    pub(super) fn message(&self) -> String {
        match self.end() {
            RunEnd::Failed(failure) => failure.to_string(),
            RunEnd::Blocked(reason) => reason,
            RunEnd::Cancelled => "run cancelled".to_owned(),
            RunEnd::Returned(_) => String::new(),
        }
    }
}

/// The result of the last `ctx` call inside one `ctx.try`.
#[derive(Clone, Debug)]
pub(super) struct Record {
    pub outcome: String,
    pub value: Json,
    pub message: String,
}

impl Record {
    pub(super) fn done(value: Json) -> Self {
        Self {
            outcome: "done".to_owned(),
            value,
            message: String::new(),
        }
    }
}

/// The state of one run. The hook and the wrappers hold it through an `Arc`, so it is `Send`.
pub(super) struct Run {
    pub budgets: Budgets,
    started: Instant,
    /// The caller's cancel flag, shared with the hook.
    pub cancel: Arc<AtomicBool>,
    steps: AtomicU64,
    ctx_calls: AtomicU64,
    /// Time spent inside `CtxHost::call`, in nanoseconds. Not script time.
    host_nanos: AtomicU64,
    latch: Mutex<Option<Latch>>,
    /// One entry per `ctx.try` being run, innermost last.
    frames: Mutex<Vec<Option<Record>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Run {
    pub(super) fn new(budgets: Budgets, cancel: Arc<AtomicBool>) -> Self {
        Self {
            budgets,
            started: Instant::now(),
            cancel,
            steps: AtomicU64::new(0),
            ctx_calls: AtomicU64::new(0),
            host_nanos: AtomicU64::new(0),
            latch: Mutex::new(None),
            frames: Mutex::new(Vec::new()),
        }
    }

    /// The latched end, if one was latched.
    pub(super) fn latched(&self) -> Option<Latch> {
        lock(&self.latch).clone()
    }

    /// Latches `latch` unless an end is latched already, and returns the error that raises it.
    /// The first end wins.
    pub(super) fn raise(&self, latch: Latch) -> LuaError {
        let effective = lock(&self.latch).get_or_insert(latch).clone();
        LuaError::RuntimeError(effective.message())
    }

    /// Raises if the run has ended or was cancelled.
    pub(super) fn live(&self) -> LuaResult<()> {
        if let Some(latch) = self.latched() {
            return Err(LuaError::RuntimeError(latch.message()));
        }
        if self.cancel.load(SeqCst) {
            return Err(self.raise(Latch::Cancelled));
        }
        Ok(())
    }

    /// Charges one `ctx` call (or helper, or `try`) against the budget.
    pub(super) fn charge_ctx(&self) -> LuaResult<()> {
        self.live()?;
        let n = self.ctx_calls.fetch_add(1, SeqCst) + 1;
        if n > u64::from(self.budgets.ctx_calls) {
            return Err(self.raise(Latch::Budget(BudgetKind::CtxCalls)));
        }
        Ok(())
    }

    pub(super) fn add_host_time(&self, elapsed: Duration) {
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.host_nanos.fetch_add(nanos, SeqCst);
    }

    /// Time the script itself has run: elapsed time minus the time inside host calls.
    pub(super) fn script_time(&self) -> Duration {
        self.started
            .elapsed()
            .saturating_sub(Duration::from_nanos(self.host_nanos.load(SeqCst)))
    }

    /// Called by Luau at every interrupt point: loop back-edges and calls.
    pub(super) fn on_interrupt(&self, lua: &Lua) -> LuaResult<VmState> {
        self.live()?;
        let n = self.steps.fetch_add(1, SeqCst) + 1;
        if n > self.budgets.steps {
            return Err(self.raise(Latch::Budget(BudgetKind::Steps)));
        }
        if n.is_multiple_of(SAMPLE_EVERY)
            && self.budgets.memory > 0
            && lua.used_memory().saturating_add(MEMORY_SLACK) >= self.budgets.memory
        {
            return Err(self.raise(Latch::Budget(BudgetKind::Memory)));
        }
        if n.is_multiple_of(CLOCK_EVERY) && self.script_time() > self.budgets.script_time {
            return Err(self.raise(Latch::Budget(BudgetKind::Time)));
        }
        Ok(VmState::Continue)
    }

    /// Enters a `ctx.try`. Its frame records the last `ctx` call made inside it.
    pub(super) fn enter_try(&self) {
        lock(&self.frames).push(None);
    }

    /// Leaves the innermost `ctx.try` and returns the last call it made. The record also becomes
    /// the last call of the enclosing `try`, if there is one.
    pub(super) fn leave_try(&self) -> Option<Record> {
        let mut frames = lock(&self.frames);
        let last = frames.pop().flatten();
        if let (Some(record), Some(outer)) = (&last, frames.last_mut()) {
            *outer = Some(record.clone());
        }
        last
    }

    pub(super) fn in_try(&self) -> bool {
        !lock(&self.frames).is_empty()
    }

    /// Records the result of a `ctx` call in the innermost `try`.
    pub(super) fn set_last(&self, record: Record) {
        if let Some(top) = lock(&self.frames).last_mut() {
            *top = Some(record);
        }
    }
}

/// A fresh sandboxed VM for one run, with the hook and the wrappers installed.
pub(super) fn new_vm(run: &Arc<Run>) -> LuaResult<Lua> {
    let lua = Lua::new();
    let globals = lua.globals();

    // `string.rep` is the one builder whose size is known before it runs, so it is capped here.
    let string: Table = globals.get("string")?;
    let original_rep: Function = string.get("rep")?;
    let guard = Arc::clone(run);
    let rep = lua.create_function(
        move |lua, (text, count, sep): (mlua::String, i64, Option<mlua::String>)| {
            guard.live()?;
            let sep_len = sep.as_ref().map_or(0, |s| s.as_bytes().len());
            let copies = usize::try_from(count.max(0)).unwrap_or(usize::MAX);
            let total = (text.as_bytes().len() + sep_len).saturating_mul(copies);
            if total > guard.budgets.string_bytes {
                return Err(guard.raise(Latch::Budget(BudgetKind::StringSize)));
            }
            if guard.budgets.memory > 0
                && lua.used_memory().saturating_add(total) > guard.budgets.memory
            {
                return Err(guard.raise(Latch::Budget(BudgetKind::Memory)));
            }
            original_rep.call::<mlua::String>((text, count, sep))
        },
    )?;
    string.raw_set("rep", rep)?;

    // The catch points. A script that catches an error gets it back only if no end is latched.
    wrap_catch(&lua, run, &globals, "pcall")?;
    wrap_catch(&lua, run, &globals, "xpcall")?;
    let coroutine: Table = globals.get("coroutine")?;
    wrap_catch(&lua, run, &coroutine, "resume")?;
    wrap_catch(&lua, run, &coroutine, "close")?;

    for name in REMOVE_GLOBALS {
        globals.raw_set(*name, Value::Nil)?;
    }
    for (library, field) in REMOVE_FIELDS {
        if let Value::Table(table) = globals.get::<Value>(*library)? {
            table.raw_set(*field, Value::Nil)?;
        }
    }

    if run.budgets.memory > 0 {
        lua.set_memory_limit(run.budgets.memory)?;
    }
    let hook = Arc::clone(run);
    lua.set_interrupt(move |lua| hook.on_interrupt(lua));
    freeze(&lua)?;
    Ok(lua)
}

/// Makes the global environment read-only, and every library table in it, and the string
/// metatable. This is `luaL_sandbox` without its per-thread global table: that table would let a
/// script assign globals, and the docs promise a read-only environment (D99).
fn freeze(lua: &Lua) -> LuaResult<()> {
    let globals = lua.globals();
    for pair in globals.pairs::<Value, Value>() {
        if let (_, Value::Table(library)) = pair? {
            library.set_readonly(true);
        }
    }
    let get_metatable: Function = globals.get("getmetatable")?;
    let string_meta: Table = get_metatable.call(lua.create_string("")?)?;
    string_meta.set_readonly(true);
    globals.set_readonly(true);
    Ok(())
}

/// Wraps a function that catches errors. After it returns, a latched end is raised again, so
/// the script cannot continue past a budget or a host end. An allocation failure is a memory
/// budget end.
fn wrap_catch(lua: &Lua, run: &Arc<Run>, table: &Table, name: &str) -> LuaResult<()> {
    let original: Function = table.get(name)?;
    let guard = Arc::clone(run);
    let wrapped = lua.create_function(move |_, args: MultiValue| {
        let result: MultiValue = original.call(args)?;
        if let Some(latch) = guard.latched() {
            return Err(LuaError::RuntimeError(latch.message()));
        }
        if let (Some(Value::Boolean(false)), Some(Value::String(message))) =
            (result.front(), result.get(1))
            && message.to_string_lossy().contains("not enough memory")
        {
            return Err(guard.raise(Latch::Budget(BudgetKind::Memory)));
        }
        Ok(result)
    })?;
    table.raw_set(name, wrapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_latched_end_wins() {
        let run = Run::new(Budgets::default(), Arc::new(AtomicBool::new(false)));
        let _ = run.raise(Latch::Budget(BudgetKind::Steps));
        let _ = run.raise(Latch::Blocked("later".into()));
        assert!(matches!(
            run.latched(),
            Some(Latch::Budget(BudgetKind::Steps))
        ));
    }

    #[test]
    fn a_blocked_latch_reports_the_reason() {
        let latch = Latch::Blocked("no".into());
        assert_eq!(latch.end(), RunEnd::Blocked("no".into()));
        assert_eq!(latch.message(), "no");
    }
}

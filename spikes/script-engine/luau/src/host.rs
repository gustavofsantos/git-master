//! Host model for the Luau spike: one fresh VM per run, a read-only `ctx`,
//! budgets enforced from the interrupt hook and sticky "latched" failures.
//!
//! Key design point under test: the budget reason is latched in host state.
//! Once latched, every later interrupt point raises again, and the run's
//! outcome is the latched reason even if the script caught the error with
//! `pcall` and returned normally.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering::Relaxed, Ordering::SeqCst};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mlua::{Error, Function, Lua, MultiValue, Result as LuaResult, Table, Value, VmState};

pub const MIB: usize = 1 << 20;
/// Headroom below the memory limit at which the interrupt latches `Memory`.
const MEMORY_SLACK: usize = 256 * 1024;
/// Wall clock is read every this many interrupt points (Instant::now is not free).
const WALL_CHECK_EVERY: u64 = 256;
/// Memory is sampled every this many interrupt points (lua.used_memory takes a lock).
const SAMPLE_EVERY: u64 = 1024;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Interrupt points (loop back-edges and Lua calls) allowed per run.
    pub steps: u64,
    pub wall: Duration,
    /// Allocator limit in bytes (0 = none).
    pub memory: usize,
    /// Max bytes of one string built by `string.rep` (the only guarded builder).
    pub string_bytes: usize,
    /// Number of `ctx` calls per run.
    pub ctx_calls: u64,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        steps: 20_000_000,
        wall: Duration::from_secs(2),
        memory: 64 * MIB,
        string_bytes: 16 * MIB,
        ctx_calls: 1000,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
    Steps = 1,
    Wall = 2,
    Memory = 3,
    StringSize = 4,
    Cancelled = 5,
    CtxCalls = 6,
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Reason::Steps => "steps",
            Reason::Wall => "wall_clock",
            Reason::Memory => "memory",
            Reason::StringSize => "string_size",
            Reason::Cancelled => "cancelled",
            Reason::CtxCalls => "ctx_calls",
        }
    }
    fn from_u8(v: u8) -> Option<Reason> {
        Some(match v {
            1 => Reason::Steps,
            2 => Reason::Wall,
            3 => Reason::Memory,
            4 => Reason::StringSize,
            5 => Reason::Cancelled,
            6 => Reason::CtxCalls,
            _ => return None,
        })
    }
}

/// Per-run shared state. Lives in an `Arc`, cloned into every closure.
pub struct Shared {
    pub limits: Limits,
    pub started: Instant,
    pub cancel: AtomicBool,
    latch: AtomicU8,
    pub steps: AtomicU64,
    pub ctx_calls: AtomicU64,
    pub peak_memory: AtomicUsize,
    pub clock: AtomicU64,
    pub notes: Mutex<Vec<String>>,
    wake_tx: mpsc::Sender<Option<String>>,
    wake_rx: Mutex<mpsc::Receiver<Option<String>>>,
}

impl Shared {
    pub fn new(limits: Limits) -> Arc<Shared> {
        let (tx, rx) = mpsc::channel();
        Arc::new(Shared {
            limits,
            started: Instant::now(),
            cancel: AtomicBool::new(false),
            latch: AtomicU8::new(0),
            steps: AtomicU64::new(0),
            ctx_calls: AtomicU64::new(0),
            peak_memory: AtomicUsize::new(0),
            clock: AtomicU64::new(1_700_000_000_000),
            notes: Mutex::new(Vec::new()),
            wake_tx: tx,
            wake_rx: Mutex::new(rx),
        })
    }

    /// Flip the cancel flag and wake a thread blocked in `ctx.block_recv`.
    pub fn cancel(&self) {
        self.cancel.store(true, SeqCst);
        let _ = self.wake_tx.send(None);
    }

    /// Host sends a value to a blocked `ctx.block_recv`.
    pub fn send_value(&self, v: &str) {
        let _ = self.wake_tx.send(Some(v.to_string()));
    }

    pub fn reason(&self) -> Option<Reason> {
        Reason::from_u8(self.latch.load(SeqCst))
    }

    /// First latched reason wins.
    pub fn latch(&self, r: Reason) {
        let _ = self.latch.compare_exchange(0, r as u8, SeqCst, SeqCst);
    }

    pub fn message(r: Reason, l: &Limits) -> String {
        match r {
            Reason::Steps => format!("budget exceeded: steps (limit {} interrupt points)", l.steps),
            Reason::Wall => format!("budget exceeded: wall clock (limit {} ms)", l.wall.as_millis()),
            Reason::Memory => format!("budget exceeded: memory (limit {} bytes)", l.memory),
            Reason::StringSize => format!("budget exceeded: string size (limit {} bytes)", l.string_bytes),
            Reason::CtxCalls => format!("budget exceeded: ctx calls (limit {})", l.ctx_calls),
            Reason::Cancelled => "run cancelled".to_string(),
        }
    }

    /// Latch `r` (if nothing is latched yet) and return the error that ends the run.
    fn fail(&self, r: Reason) -> Error {
        self.latch(r);
        let first = self.reason().unwrap_or(r);
        Error::RuntimeError(Shared::message(first, &self.limits))
    }

    /// Sticky check: once latched, every host entry point raises again.
    fn sticky(&self) -> LuaResult<()> {
        match self.reason() {
            Some(r) => Err(Error::RuntimeError(Shared::message(r, &self.limits))),
            None => Ok(()),
        }
    }

    /// Called by Luau at every interrupt point (loop back-edge, Lua call).
    pub fn on_interrupt(&self, lua: &Lua) -> LuaResult<VmState> {
        if self.latch.load(Relaxed) != 0 {
            return Err(self.sticky().unwrap_err());
        }
        if self.cancel.load(Relaxed) {
            return Err(self.fail(Reason::Cancelled));
        }
        // Only the VM's own thread runs this hook, so a plain load/store is enough.
        let n = self.steps.load(Relaxed) + 1;
        self.steps.store(n, Relaxed);
        if n > self.limits.steps {
            return Err(self.fail(Reason::Steps));
        }
        if n & (SAMPLE_EVERY - 1) == 0 {
            // Headroom check and peak sample. The allocator enforces the limit itself on every
            // allocation; this only catches a script that is about to hit it.
            let used = lua.used_memory();
            self.peak_memory.fetch_max(used, Relaxed);
            if self.limits.memory > 0 && used + MEMORY_SLACK >= self.limits.memory {
                return Err(self.fail(Reason::Memory));
            }
        }
        if n & (WALL_CHECK_EVERY - 1) == 0 && self.started.elapsed() > self.limits.wall {
            return Err(self.fail(Reason::Wall));
        }
        Ok(VmState::Continue)
    }

    /// Charge one `ctx` call against the per-run budget.
    pub fn charge_ctx(&self) -> LuaResult<u64> {
        self.sticky()?;
        if self.cancel.load(Relaxed) {
            return Err(self.fail(Reason::Cancelled));
        }
        let n = self.ctx_calls.fetch_add(1, Relaxed) + 1;
        if n > self.limits.ctx_calls {
            return Err(self.fail(Reason::CtxCalls));
        }
        Ok(n)
    }

    pub fn check_string(&self, bytes: usize) -> LuaResult<()> {
        self.sticky()?;
        if bytes > self.limits.string_bytes {
            return Err(self.fail(Reason::StringSize));
        }
        Ok(())
    }

    /// Blocks the calling thread until a host value arrives, the run is cancelled,
    /// or the wall-clock budget runs out. Cancel also sends a wake message, so the
    /// normal path is a direct wake-up; the 50 ms poll is only a fallback.
    pub fn block_recv(&self) -> LuaResult<String> {
        self.charge_ctx()?;
        loop {
            self.sticky()?;
            if self.cancel.load(Relaxed) {
                return Err(self.fail(Reason::Cancelled));
            }
            let remaining = self.limits.wall.saturating_sub(self.started.elapsed());
            if remaining.is_zero() {
                return Err(self.fail(Reason::Wall));
            }
            let rx = self.wake_rx.lock().unwrap();
            match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(Some(v)) => return Ok(v),
                Ok(None) => return Err(self.fail(Reason::Cancelled)),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(self.fail(Reason::Cancelled)),
            }
        }
    }
}

/// Options that vary between experiments.
#[derive(Clone, Copy, Debug)]
pub struct Opts {
    /// Install the interrupt hook (steps, wall clock, cancel). Off only for the overhead baseline.
    pub hook: bool,
    /// Install the `string.rep` guard.
    pub string_guard: bool,
}

impl Opts {
    pub const FULL: Opts = Opts { hook: true, string_guard: true };
}

/// Globals the design forbids. Removed before `sandbox(true)`, because the
/// globals table is read-only afterwards.
pub const REMOVE_GLOBALS: &[&str] = &[
    "os", "io", "debug", "package", "require", "load", "loadstring", "dofile", "loadfile",
    "collectgarbage", "gcinfo", "print", "warn", "getfenv", "setfenv", "newproxy",
];
/// Fields the design forbids (`string.dump` is absent in Luau; kept for the record).
pub const REMOVE_FIELDS: &[(&str, &str)] = &[("math", "random"), ("math", "randomseed"), ("string", "dump")];

pub struct Vm {
    pub lua: Lua,
    pub ctx: Table,
}

/// Builds a fresh sandboxed Luau state with the host model installed.
pub fn build_vm(shared: &Arc<Shared>, opts: Opts) -> LuaResult<Vm> {
    let lua = Lua::new();
    let g = lua.globals();

    if opts.string_guard {
        let string: Table = g.get("string")?;
        let orig_rep: Function = string.get("rep")?;
        let sh = shared.clone();
        let guarded = lua.create_function(move |lua, (s, n, sep): (mlua::String, i64, Option<mlua::String>)| {
            let sep_len = sep.as_ref().map_or(0, |x| x.as_bytes().len());
            let total = (s.as_bytes().len() + sep_len).saturating_mul(n.max(0) as usize);
            sh.check_string(total)?;
            // The allocator refuses when used + request > limit. Check that rule here, so the
            // outcome does not depend on where the failing allocation happens to be.
            let limit = sh.limits.memory;
            if limit > 0 && lua.used_memory().saturating_add(total) > limit {
                return Err(sh.fail(Reason::Memory));
            }
            orig_rep.call::<mlua::String>((s, n, sep))
        })?;
        string.raw_set("rep", guarded)?;
    }

    // Catch points. A memory error caught by pcall/xpcall/coroutine.resume is latched, because
    // an allocation can fail with plenty of headroom left (a large table resize), so the
    // interrupt's headroom check alone cannot see it. Budget errors are already latched.
    for (lib, name) in [("", "pcall"), ("", "xpcall"), ("coroutine", "resume")] {
        let table = if lib.is_empty() { g.clone() } else { g.get::<Table>(lib)? };
        let orig: Function = table.get(name)?;
        let sh = shared.clone();
        let wrapped = lua.create_function(move |_, args: MultiValue| {
            let res: MultiValue = orig.call(args)?;
            if let Some(Value::Boolean(false)) = res.front() {
                if let Some(Value::String(msg)) = res.get(1) {
                    if msg.to_string_lossy().contains("not enough memory") {
                        sh.latch(Reason::Memory);
                    }
                }
            }
            Ok(res)
        })?;
        table.raw_set(name, wrapped)?;
    }

    for name in REMOVE_GLOBALS {
        g.raw_set(*name, Value::Nil)?;
    }
    for (lib, field) in REMOVE_FIELDS {
        if let Value::Table(t) = g.get::<Value>(*lib)? {
            t.raw_set(*field, Value::Nil)?;
        }
    }

    let ctx = lua.create_table()?;
    let sh = shared.clone();
    ctx.set("notify", lua.create_function(move |_, text: String| {
        sh.charge_ctx()?;
        sh.notes.lock().unwrap().push(text);
        Ok(())
    })?)?;
    let sh = shared.clone();
    ctx.set("now", lua.create_function(move |_, ()| {
        sh.charge_ctx()?;
        Ok(sh.clock.fetch_add(1, Relaxed) as i64)
    })?)?;
    let sh = shared.clone();
    ctx.set("count", lua.create_function(move |_, ()| {
        let n = sh.charge_ctx()?;
        Ok(n as i64)
    })?)?;
    let sh = shared.clone();
    ctx.set("block_recv", lua.create_function(move |_, ()| sh.block_recv())?)?;
    let sh = shared.clone();
    ctx.set("try", lua.create_function(move |lua, f: Function| {
        sh.charge_ctx()?;
        let t = lua.create_table()?;
        match f.call::<Value>(()) {
            Ok(v) => {
                t.set("outcome", "done")?;
                t.set("value", v)?;
            }
            Err(e) => {
                // Budget latches and memory errors are not caught by ctx.try.
                sh.sticky()?;
                if is_memory_error(&e) {
                    sh.latch(Reason::Memory);
                    return Err(e);
                }
                t.set("outcome", "error")?;
                t.set("message", error_message(&e))?;
            }
        }
        Ok(t)
    })?)?;
    ctx.set_readonly(true);

    if shared.limits.memory > 0 {
        lua.set_memory_limit(shared.limits.memory)?;
    }
    if opts.hook {
        let sh = shared.clone();
        lua.set_interrupt(move |lua| sh.on_interrupt(lua));
    }
    lua.sandbox(true)?;
    Ok(Vm { lua, ctx })
}

/// mlua reports an allocation failure as `MemoryError`, except when it crosses a Rust call,
/// where it arrives as `RuntimeError("<nil>")`. Both mean the memory limit was hit.
pub fn is_memory_error(e: &Error) -> bool {
    matches!(e, Error::MemoryError(_)) || error_message(e) == "<nil>" || error_message(e).contains("not enough memory")
}

pub fn error_message(e: &Error) -> String {
    match e {
        Error::RuntimeError(s) => s.clone(),
        Error::CallbackError { cause, .. } => error_message(cause),
        other => other.to_string(),
    }
}

/// `ctx` function names as the host sees them (sorted).
pub fn ctx_names(vm: &Vm) -> Vec<String> {
    let mut names: Vec<String> = vm
        .ctx
        .pairs::<String, Value>()
        .filter_map(|p| p.ok().map(|(k, _)| k))
        .collect();
    names.sort();
    names
}

pub fn display(v: &Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => format!("{n}"),
        Value::String(s) => s.to_string_lossy(),
        other => other.type_name().to_string(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Class {
    Completed,
    Budget(Reason),
    StackOverflow,
    SyntaxError,
    RuntimeError,
}

impl Class {
    pub fn label(&self) -> String {
        match self {
            Class::Completed => "completed".into(),
            Class::Budget(r) => format!("budget:{}", r.name()),
            Class::StackOverflow => "stack_overflow".into(),
            Class::SyntaxError => "syntax_error".into(),
            Class::RuntimeError => "runtime_error".into(),
        }
    }
}

#[derive(Debug)]
pub struct Outcome {
    pub class: Class,
    pub returned: Option<String>,
    pub error: Option<String>,
    pub notes: Vec<String>,
    pub steps: u64,
    pub ctx_calls: u64,
    pub peak_memory: usize,
    pub elapsed: Duration,
    pub ended: Instant,
}

fn exec_module(vm: &Vm, src: &str) -> LuaResult<String> {
    let lua = &vm.lua;
    let module: Table = lua.load(src).set_name("=workflow").eval()?;
    let execute: Function = module.get("execute")?;
    let input = lua.create_table()?;
    input.set("repo", "/repo")?;
    input.set("count", 3)?;
    input.set_readonly(true);
    let v: Value = execute.call((vm.ctx.clone(), input))?;
    Ok(display(&v))
}

/// Runs a whole module on the calling thread: build VM, load `src`, call `M.execute(ctx, input)`.
pub fn run_module(src: &str, shared: &Arc<Shared>, opts: Opts) -> Outcome {
    let result: LuaResult<String> = match build_vm(shared, opts) {
        Ok(vm) => exec_module(&vm, src),
        Err(e) => Err(e),
    };
    let ended = Instant::now();
    let (returned, error, class) = match result {
        Ok(s) => (Some(s), None, Class::Completed),
        Err(e) => {
            let msg = error_message(&e);
            let class = match &e {
                Error::SyntaxError { .. } => Class::SyntaxError,
                // The allocator refused: the memory limit is the only cause.
                _ if is_memory_error(&e) => Class::Budget(Reason::Memory),
                _ if msg.contains("stack overflow") => Class::StackOverflow,
                _ => Class::RuntimeError,
            };
            (None, Some(msg), class)
        }
    };
    // A latched reason always wins: the script may have caught the error and carried on.
    // The message is the budget's own text (mlua's MemoryError carries no message).
    let (class, error) = match shared.reason() {
        Some(r) => (Class::Budget(r), Some(Shared::message(r, &shared.limits))),
        None if class == Class::Budget(Reason::Memory) => (class, Some(Shared::message(Reason::Memory, &shared.limits))),
        None => (class, error),
    };
    Outcome {
        class,
        returned,
        error,
        notes: shared.notes.lock().unwrap().clone(),
        steps: shared.steps.load(Relaxed),
        ctx_calls: shared.ctx_calls.load(Relaxed),
        peak_memory: shared.peak_memory.load(Relaxed),
        elapsed: ended.duration_since(shared.started),
        ended,
    }
}

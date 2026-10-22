//! Host model for the Rhai spike: fresh engine per run, allow-listed packages,
//! the `ctx` object, budgets and cancellation, run on its own thread.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rhai::packages::{
    BasicArrayPackage, BasicMapPackage, BasicMathPackage, CorePackage, LogicPackage,
    MoreStringPackage, Package,
};
use rhai::{
    AST, Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Map, NativeCallContext,
    ParseError, Position, Scope,
};

use crate::alloc;

/// Fake clock value returned by `ctx.now()`.
pub const FAKE_CLOCK: i64 = 1_700_000_000;
/// Stack for each run thread. Chosen so that the call-depth budget fits (see RESULTS.md).
pub const RUN_STACK: usize = 16 * 1024 * 1024;
/// Value the host sends to wake a blocked `ctx.block_recv()` on cancel.
const WAKE: i64 = i64::MIN;

/// Budgets. A value of 0 (or `None`) means the budget is off.
#[derive(Clone, Copy, Debug)]
pub struct Budgets {
    pub max_ops: u64,
    pub wall: Option<Duration>,
    pub mem: Option<usize>,
    pub max_string: usize,
    pub max_array: usize,
    pub max_map: usize,
    pub max_call_levels: usize,
    pub max_ctx_calls: u64,
}

impl Budgets {
    /// Starting values chosen for the spike. Tuned later from the numbers in RESULTS.md.
    pub const SPIKE: Budgets = Budgets {
        max_ops: 50_000_000,
        wall: Some(Duration::from_secs(2)),
        mem: Some(64 << 20),
        max_string: 1 << 20,
        max_array: 20_000,
        max_map: 10_000,
        max_call_levels: 64,
        max_ctx_calls: 10_000,
    };

    /// Everything off. Used by the cancellation and overhead tests.
    pub const OFF: Budgets = Budgets {
        max_ops: 0,
        wall: None,
        mem: None,
        max_string: 0,
        max_array: 0,
        max_map: 0,
        max_call_levels: 64,
        max_ctx_calls: u64::MAX,
    };
}

/// Shared per-run state behind the `ctx` object.
pub struct CtxState {
    pub notes: Mutex<Vec<String>>,
    pub calls: AtomicU64,
    pub max_calls: u64,
    pub cancel: Arc<AtomicBool>,
    /// The run's wall-clock deadline. The progress hook never runs during a host call, so
    /// `block_recv` must check it itself, or a blocked script would outlive its budget.
    pub deadline: Option<Instant>,
    pub rx: Mutex<Receiver<i64>>,
}

/// The host object given to `execute` as `ctx`. Methods take it by value (an Arc clone),
/// so they also work on a non-`mut` binding.
#[derive(Clone)]
pub struct Ctx(pub Arc<CtxState>);

/// Rhai's `RhaiResult` alias is private; this is the same type.
pub type RhaiResult = Result<Dynamic, Box<EvalAltResult>>;

pub fn terminated(reason: &str) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorTerminated(
        Dynamic::from(reason.to_string()),
        Position::NONE,
    ))
}

fn charge(c: &Ctx) -> Result<u64, Box<EvalAltResult>> {
    let n = c.0.calls.fetch_add(1, Ordering::Relaxed) + 1;
    if n > c.0.max_calls {
        return Err(terminated("ctx-calls"));
    }
    Ok(n)
}

fn ctx_notify(c: Ctx, text: ImmutableString) -> RhaiResult {
    charge(&c)?;
    c.0.notes.lock().unwrap().push(text.to_string());
    Ok(Dynamic::UNIT)
}

fn ctx_now(c: Ctx) -> RhaiResult {
    charge(&c)?;
    Ok(Dynamic::from(FAKE_CLOCK))
}

fn ctx_count(c: Ctx) -> RhaiResult {
    let n = charge(&c)?;
    Ok(Dynamic::from(n as i64))
}

/// Blocks until the host sends a value (or wakes it on cancel). Polls the flag as well,
/// so a missed wake-up still ends the run within one poll period.
fn ctx_block_recv(c: Ctx) -> RhaiResult {
    charge(&c)?;
    let rx = c.0.rx.lock().unwrap();
    loop {
        if c.0.cancel.load(Ordering::Acquire) {
            return Err(terminated("cancelled"));
        }
        if c.0.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(terminated("wall-clock"));
        }
        match rx.recv_timeout(Duration::from_millis(5)) {
            Ok(WAKE) => {
                if c.0.cancel.load(Ordering::Acquire) {
                    return Err(terminated("cancelled"));
                }
            }
            Ok(v) => return Ok(Dynamic::from(v)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(terminated("channel-closed")),
        }
    }
}

/// `ctx.try_run(fn)` (Lua `ctx.try`; `try` is a Rhai keyword and cannot follow a dot): runs `fn`, returns `#{outcome, value | message}`. System errors (budgets,
/// termination, stack, data size) are re-raised, never turned into an "error" outcome.
fn ctx_try(nctx: NativeCallContext, c: Ctx, f: FnPtr) -> RhaiResult {
    charge(&c)?;
    let mut m = Map::new();
    match f.call_within_context::<Dynamic>(&nctx, ()) {
        Ok(v) => {
            m.insert("outcome".into(), Dynamic::from("done".to_string()));
            m.insert("value".into(), v);
        }
        Err(e) => {
            if e.unwrap_inner().is_system_exception() {
                return Err(e);
            }
            m.insert("outcome".into(), Dynamic::from("error".to_string()));
            m.insert("message".into(), Dynamic::from(e.unwrap_inner().to_string()));
        }
    }
    Ok(Dynamic::from_map(m))
}

/// A fresh engine: allow-listed packages only, `eval` disabled, print/debug swallowed,
/// size and depth limits from `b`. No module resolver is set (`new_raw`), so `import` fails.
pub fn sandbox_engine(b: &Budgets, with_ctx: bool) -> Engine {
    let mut e = Engine::new_raw();
    CorePackage::new().register_into_engine(&mut e);
    LogicPackage::new().register_into_engine(&mut e);
    BasicMathPackage::new().register_into_engine(&mut e);
    BasicArrayPackage::new().register_into_engine(&mut e);
    BasicMapPackage::new().register_into_engine(&mut e);
    MoreStringPackage::new().register_into_engine(&mut e);
    e.disable_symbol("eval");
    e.on_print(|_| {});
    e.on_debug(|_, _, _| {});
    if b.max_ops > 0 {
        e.set_max_operations(b.max_ops);
    }
    e.set_max_call_levels(b.max_call_levels);
    e.set_max_expr_depths(64, 32);
    if b.max_string > 0 {
        e.set_max_string_size(b.max_string);
    }
    if b.max_array > 0 {
        e.set_max_array_size(b.max_array);
    }
    if b.max_map > 0 {
        e.set_max_map_size(b.max_map);
    }
    if with_ctx {
        e.register_type_with_name::<Ctx>("Ctx")
            .register_fn("notify", ctx_notify)
            .register_fn("now", ctx_now)
            .register_fn("count", ctx_count)
            .register_fn("block_recv", ctx_block_recv)
            .register_fn("try_run", ctx_try);
    }
    e
}

/// Progress hook: cancel and memory are checked on every call (a single `[a, a]` can double
/// the heap, so a sampled check could overshoot badly); the wall clock every 1024 calls.
fn install_hook(e: &mut Engine, cancel: Arc<AtomicBool>, deadline: Option<Instant>, mem: Option<(isize, isize)>) {
    e.on_progress(move |ops| {
        if cancel.load(Ordering::Acquire) {
            return Some(Dynamic::from("cancelled".to_string()));
        }
        if let Some((base, limit)) = mem {
            if alloc::live() - base > limit {
                return Some(Dynamic::from("memory".to_string()));
            }
        }
        if ops & 1023 == 0 {
            if let Some(d) = deadline {
                if Instant::now() >= d {
                    return Some(Dynamic::from("wall-clock".to_string()));
                }
            }
        }
        None
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Instructions,
    WallClock,
    Memory,
    StringSize,
    ArraySize,
    MapSize,
    StackDepth,
    CtxCalls,
    Cancelled,
    ChannelClosed,
    Capability,
    Parse,
    Script,
    Other,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::Instructions => "instructions",
            Reason::WallClock => "wall-clock",
            Reason::Memory => "memory",
            Reason::StringSize => "string-size",
            Reason::ArraySize => "array-size",
            Reason::MapSize => "map-size",
            Reason::StackDepth => "stack-depth",
            Reason::CtxCalls => "ctx-calls",
            Reason::Cancelled => "cancelled",
            Reason::ChannelClosed => "channel-closed",
            Reason::Capability => "capability-missing",
            Reason::Parse => "parse-error",
            Reason::Script => "script-error",
            Reason::Other => "other",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub reason: Reason,
    pub message: String,
    /// For `throw <value>`: the thrown value, debug-formatted.
    pub payload: Option<String>,
}

pub fn classify(e: &EvalAltResult) -> Failure {
    let inner = e.unwrap_inner();
    let message = inner.to_string();
    let (reason, payload) = match inner {
        EvalAltResult::ErrorTooManyOperations(_) => (Reason::Instructions, None),
        EvalAltResult::ErrorStackOverflow(_) => (Reason::StackDepth, None),
        EvalAltResult::ErrorDataTooLarge(kind, _) => {
            let r = if kind.starts_with("Length of string") {
                Reason::StringSize
            } else if kind.starts_with("Size of array") {
                Reason::ArraySize
            } else if kind.starts_with("Size of object") {
                Reason::MapSize
            } else {
                Reason::Other
            };
            (r, None)
        }
        EvalAltResult::ErrorTerminated(p, _) => {
            let r = match p.to_string().as_str() {
                "wall-clock" => Reason::WallClock,
                "memory" => Reason::Memory,
                "ctx-calls" => Reason::CtxCalls,
                "cancelled" => Reason::Cancelled,
                "channel-closed" => Reason::ChannelClosed,
                _ => Reason::Other,
            };
            (r, None)
        }
        EvalAltResult::ErrorFunctionNotFound(..)
        | EvalAltResult::ErrorVariableNotFound(..)
        | EvalAltResult::ErrorModuleNotFound(..) => (Reason::Capability, None),
        EvalAltResult::ErrorRuntime(v, _) => (Reason::Script, Some(format!("{v:?}"))),
        _ => (Reason::Script, None),
    };
    Failure { reason, message, payload }
}

pub fn parse_failure(pe: &ParseError) -> Failure {
    Failure {
        reason: Reason::Parse,
        message: format!(
            "{pe} (line {}, column {})",
            pe.1.line().unwrap_or(0),
            pe.1.position().unwrap_or(0)
        ),
        payload: None,
    }
}

pub struct Job {
    pub src: String,
    pub input: Map,
    pub budgets: Budgets,
    /// Install the progress hook (cancel, wall clock, memory, and the op counter).
    pub hook: bool,
    pub cancel: Arc<AtomicBool>,
    pub rx: Receiver<i64>,
}

pub struct Out {
    pub result: Result<Dynamic, Failure>,
    pub notes: Vec<String>,
    pub ctx_calls: u64,
    /// Peak bytes live on the run thread during the run (counting allocator).
    pub mem_peak: isize,
    pub finished: Instant,
    pub elapsed: Duration,
}

/// Runs a module on the current thread: fresh engine, compile, `execute(ctx, input)`.
pub fn run_here(job: Job) -> Out {
    let start = Instant::now();
    let base = alloc::live();
    alloc::reset_peak();
    let b = job.budgets;
    let mut e = sandbox_engine(&b, true);
    if job.hook {
        let deadline = b.wall.map(|d| start + d);
        let mem = b.mem.map(|m| (base, m as isize));
        install_hook(&mut e, job.cancel.clone(), deadline, mem);
    }
    let ctx = Ctx(Arc::new(CtxState {
        notes: Mutex::new(Vec::new()),
        calls: AtomicU64::new(0),
        max_calls: b.max_ctx_calls,
        cancel: job.cancel.clone(),
        deadline: b.wall.map(|d| start + d),
        rx: Mutex::new(job.rx),
    }));
    let result = match e.compile(&job.src) {
        Err(pe) => Err(parse_failure(&pe)),
        Ok(ast) => call_execute(&e, &ast, ctx.clone(), job.input),
    };
    let finished = Instant::now();
    let mem_peak = alloc::peak() - base;
    Out {
        result,
        notes: ctx.0.notes.lock().unwrap().clone(),
        ctx_calls: ctx.0.calls.load(Ordering::Relaxed),
        mem_peak,
        finished,
        elapsed: finished - start,
    }
}

fn call_execute(e: &Engine, ast: &AST, ctx: Ctx, input: Map) -> Result<Dynamic, Failure> {
    let mut scope = Scope::new();
    e.call_fn::<Dynamic>(&mut scope, ast, "execute", (ctx, input))
        .map_err(|err| classify(&err))
}

/// Runs a job on its own OS thread, as the host does for every run.
pub fn run_thread(job: Job) -> Out {
    thread::Builder::new()
        .stack_size(RUN_STACK)
        .spawn(move || run_here(job))
        .expect("spawn run thread")
        .join()
        .expect("run thread panicked")
}

/// Reads `M.meta` (the top-level `const META`) with no ctx and no host functions:
/// an engine with no packages and no registered types, running only the top level.
pub fn read_meta(src: &str) -> Result<Dynamic, Failure> {
    let mut e = Engine::new_raw();
    e.disable_symbol("eval");
    e.set_max_operations(100_000);
    let ast = e.compile(src).map_err(|pe| parse_failure(&pe))?;
    let mut scope = Scope::new();
    e.run_ast_with_scope(&mut scope, &ast).map_err(|err| classify(&err))?;
    scope.get_value::<Dynamic>("META").ok_or(Failure {
        reason: Reason::Other,
        message: "META is not defined at top level".into(),
        payload: None,
    })
}

/// The script's own counting of its work: runs with the hook installed and reports the
/// op counter the hook last saw. Used to size the instruction budget.
pub fn ops_used(src: &str, input: Map) -> u64 {
    use std::sync::atomic::AtomicU64 as A;
    let last = Arc::new(A::new(0));
    let mut e = sandbox_engine(&Budgets::OFF, true);
    let l2 = last.clone();
    e.on_progress(move |ops| {
        l2.store(ops, Ordering::Relaxed);
        None
    });
    let (_tx, rx) = std::sync::mpsc::channel::<i64>();
    let ctx = Ctx(Arc::new(CtxState {
        notes: Mutex::new(Vec::new()),
        calls: AtomicU64::new(0),
        max_calls: u64::MAX,
        cancel: Arc::new(AtomicBool::new(false)),
        deadline: None,
        rx: Mutex::new(rx),
    }));
    if let Ok(ast) = e.compile(src) {
        let _ = call_execute(&e, &ast, ctx, input);
    }
    last.load(Ordering::Relaxed)
}

/// Builds a standard job for a source string with the given budgets.
pub fn job(src: &str, budgets: Budgets, hook: bool, cancel: Arc<AtomicBool>, rx: Receiver<i64>) -> Job {
    Job { src: src.to_string(), input: default_input(), budgets, hook, cancel, rx }
}

/// A single-use channel for a run. The sender lets the driver wake a blocked `block_recv`.
pub fn channel() -> (std::sync::mpsc::Sender<i64>, Receiver<i64>) {
    std::sync::mpsc::channel()
}

pub fn wake_value() -> i64 {
    WAKE
}

pub fn default_input() -> Map {
    let mut m = Map::new();
    m.insert("repo".into(), Dynamic::from("/tmp/example-repo".to_string()));
    m.insert("branch".into(), Dynamic::from("main".to_string()));
    m.insert("count".into(), Dynamic::from(3_i64));
    m
}

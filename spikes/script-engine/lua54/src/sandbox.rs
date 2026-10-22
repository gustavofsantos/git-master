//! Lua 5.4 (mlua, vendored 5.4.8) sandbox used by the S-D battery.
//!
//! One fresh VM per run. The script never sees the real globals: it runs with an `env` proxy
//! whose `__index` is an allow list and whose `__newindex` errors. Library tables are read-only
//! proxies, the string metatable is locked, and `ctx` is a read-only proxy over Rust closures.
//! Budgets are enforced by an instruction hook (every `hook_every` VM instructions) that sets a
//! shared stop reason. Once a reason is set, the hook keeps firing on every instruction in the
//! thread that runs; it yields when the thread is yieldable (so `pcall` cannot catch it) and
//! raises a sticky error otherwise.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering::*};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use mlua::{
    ffi, Debug, Error as LuaError, Function, HookTriggers, Lua, LuaOptions, Result as LuaResult,
    StdLib, Table, ThreadStatus, Value, VmState,
};

/// Trusted host-side Lua. It runs once per VM with the full base library and returns
/// `runner, env`. Everything it leaves out of `env` is unreachable from the script.
pub const PRELUDE: &str = r#"
local h = ...
local pcall_, error_, type_, tostring_ = pcall, error, type, tostring
local setmetatable_, getmetatable_, pairs_ = setmetatable, getmetatable, pairs
local LOCKED = "locked"

local function deny(name)
  return function(_, k)
    error_("attempt to modify read-only " .. name .. " (key '" .. tostring_(k) .. "')", 2)
  end
end

local function ro(t, name)
  local p = {}
  setmetatable_(p, { __index = t, __newindex = deny(name), __metatable = LOCKED })
  return p
end

string.dump = nil
math.random = nil
math.randomseed = nil
if h.rep then string.rep = h.rep end

local S = ro(string, "string")
local smt = getmetatable_("")
smt.__index = S
smt.__metatable = LOCKED

local T = ro(table, "table")
local M = ro(math, "math")
local U = ro(utf8, "utf8")
local C = ro(coroutine, "coroutine")

local function describe(e)
  if type_(e) == "string" then return e end
  return "non-string error (" .. type_(e) .. ")"
end

local function try(f)
  local ok, r = pcall_(f)
  if ok then return { outcome = "done", value = r } end
  if type_(r) == "string" and r:find("not enough memory", 1, true) then h.trip("memory") end
  return { outcome = "error", message = describe(r) }
end

local ctx = nil
if not h.no_ctx then
  ctx = ro({ notify = h.notify, now = h.now, count = h.count, block_recv = h.block_recv, try = try }, "ctx")
end

local function deep(t)
  local p = {}
  for k, v in pairs_(t) do
    if type_(v) == "table" then v = deep(v) end
    p[k] = v
  end
  return ro(p, "input")
end

-- Lua 5.4 disables hooks inside __gc finalizers, so a finalizer cannot be budgeted. Lua only
-- honours __gc present at setmetatable time, so rejecting it there closes the hole.
local real_setmetatable = setmetatable
local function guarded_setmetatable(t, mt)
  if type_(mt) == "table" and rawget(mt, "__gc") ~= nil then
    error_("__gc is not supported in workflow scripts", 2)
  end
  return real_setmetatable(t, mt)
end

local env = {}
local allow = {
  assert = assert, error = error, ipairs = ipairs, pairs = pairs, next = next,
  select = select, tonumber = tonumber, tostring = tostring, type = type,
  rawequal = rawequal, rawlen = rawlen, rawget = rawget, _VERSION = _VERSION,
  getmetatable = getmetatable, setmetatable = guarded_setmetatable,
  string = S, table = T, math = M, utf8 = U, coroutine = C,
  _G = env, ctx = ctx,
}
if h.keep_pcall then allow.pcall = pcall; allow.xpcall = xpcall end
if h.keep_rawset then allow.rawset = rawset end
setmetatable_(env, { __index = allow, __newindex = deny("global environment"), __metatable = LOCKED })

local function runner(modfn, input)
  local mod = modfn()
  if type_(mod) ~= "table" or type_(mod.execute) ~= "function" then
    error_("the module must return a table with an execute(ctx, input) function", 0)
  end
  return mod.execute(ctx, deep(input or {}))
end

return runner, env
"#;

/// Why a run was stopped. The first trip wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
    Instructions = 1,
    WallClock = 2,
    Memory = 3,
    StringSize = 4,
    StackDepth = 5,
    CtxCalls = 6,
    Cancelled = 7,
}

const ALL_REASONS: [Reason; 7] = [
    Reason::Instructions,
    Reason::WallClock,
    Reason::Memory,
    Reason::StringSize,
    Reason::StackDepth,
    Reason::CtxCalls,
    Reason::Cancelled,
];

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::Instructions => "instructions",
            Reason::WallClock => "wall_clock",
            Reason::Memory => "memory",
            Reason::StringSize => "string_size",
            Reason::StackDepth => "stack_depth",
            Reason::CtxCalls => "ctx_calls",
            Reason::Cancelled => "cancelled",
        }
    }
    fn from_u8(v: u8) -> Option<Reason> {
        ALL_REASONS.iter().copied().find(|r| *r as u8 == v)
    }
    fn from_label(s: &str) -> Option<Reason> {
        ALL_REASONS.iter().copied().find(|r| r.label() == s)
    }
}

/// Starting budgets. `0` memory means no limit (mlua convention).
#[derive(Clone, Copy, Debug)]
pub struct Budgets {
    pub instructions: u64,
    pub wall: Duration,
    pub memory: usize,
    pub string_max: usize,
    pub depth: usize,
    pub ctx_calls: u64,
}

impl Budgets {
    pub fn start() -> Self {
        Budgets {
            instructions: 50_000_000,
            wall: Duration::from_secs(5),
            memory: 32 << 20,
            string_max: 4 << 20,
            depth: 1000,
            ctx_calls: 10_000,
        }
    }
    /// Everything effectively off (used to isolate one budget at a time).
    pub fn unlimited() -> Self {
        Budgets {
            instructions: u64::MAX,
            wall: Duration::from_secs(3600),
            memory: 0,
            string_max: usize::MAX,
            depth: usize::MAX,
            ctx_calls: u64::MAX,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Profile {
    /// Expose the raw `pcall`/`xpcall` to the script (the production profile does not).
    pub keep_pcall: bool,
    /// Expose the raw `rawset` (bypasses `__newindex`; production does not).
    pub keep_rawset: bool,
    /// Replace `string.rep` with a host function that enforces `string_max`.
    pub wrap_rep: bool,
    /// Install the instruction hook at all.
    pub hooks: bool,
    /// Install the hook on the run's thread only (per-thread) instead of globally.
    pub per_thread_root: bool,
    /// Build the env without `ctx` (used to read `M.meta` with no host functions).
    pub no_ctx: bool,
    /// Naive variant, for measurement only: after a trip, raise an error and never yield.
    pub naive_error: bool,
    /// Measurement only: yield-or-error as usual, but do not switch to a per-instruction hook.
    pub no_switch: bool,
    pub hook_every: u32,
}

impl Profile {
    pub fn production() -> Self {
        Profile {
            keep_pcall: false,
            keep_rawset: false,
            wrap_rep: true,
            hooks: true,
            per_thread_root: false,
            no_ctx: false,
            naive_error: false,
            no_switch: false,
            hook_every: 1000,
        }
    }
}

pub struct Shared {
    pub budgets: Budgets,
    pub naive: bool,
    pub switch: bool,
    pub every: u64,
    pub started: Instant,
    pub cancel: Arc<AtomicBool>,
    stop: AtomicU8,
    pub ticks: AtomicU64,
    pub peak_mem: AtomicUsize,
    pub ctx_calls: AtomicU64,
    pub notify: Mutex<Vec<String>>,
    pub recv: Mutex<Option<mpsc::Receiver<String>>>,
}

impl Shared {
    fn trip(&self, r: Reason) {
        let _ = self.stop.compare_exchange(0, r as u8, SeqCst, SeqCst);
    }
    pub fn stopped(&self) -> Option<Reason> {
        Reason::from_u8(self.stop.load(SeqCst))
    }
}

fn charge(s: &Shared) -> LuaResult<u64> {
    if let Some(r) = s.stopped() {
        return Err(LuaError::RuntimeError(format!("run already stopped: {}", r.label())));
    }
    let n = s.ctx_calls.fetch_add(1, Relaxed) + 1;
    if n > s.budgets.ctx_calls {
        s.trip(Reason::CtxCalls);
        return Err(LuaError::RuntimeError(format!(
            "budget exceeded: ctx_calls (limit {})",
            s.budgets.ctx_calls
        )));
    }
    Ok(n)
}

fn block_recv(s: &Shared) -> LuaResult<String> {
    charge(s)?;
    let guard = s.recv.lock().unwrap();
    let Some(rx) = guard.as_ref() else {
        return Err(LuaError::RuntimeError("ctx.block_recv has no channel".into()));
    };
    loop {
        if s.cancel.load(Relaxed) {
            s.trip(Reason::Cancelled);
            return Err(LuaError::RuntimeError("cancelled inside ctx.block_recv".into()));
        }
        if let Some(r) = s.stopped() {
            return Err(LuaError::RuntimeError(format!("run stopped: {}", r.label())));
        }
        if s.started.elapsed() > s.budgets.wall {
            s.trip(Reason::WallClock);
            return Err(LuaError::RuntimeError("budget exceeded: wall_clock (in ctx.block_recv)".into()));
        }
        match rx.recv_timeout(Duration::from_millis(1)) {
            Ok(v) => return Ok(v),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(LuaError::RuntimeError("ctx.block_recv channel closed".into()))
            }
        }
    }
}

/// Stop mode: keep hooking every instruction in this thread. Yield if yieldable, else error.
fn stop_mode(lua: &Lua, s: &Arc<Shared>) -> LuaResult<VmState> {
    let th = lua.current_thread();
    if s.naive {
        // Naive variant: keep the period at N and raise an error; no yield, no switch to 1.
        let why = s.stopped().map(|r| r.label()).unwrap_or("unknown");
        return Err(LuaError::RuntimeError(format!("budget exceeded: {why}")));
    }
    if s.switch {
        let s2 = s.clone();
        let _ = th.set_hook(HookTriggers::new().every_nth_instruction(1), move |lua: &Lua, _: &Debug| {
            stop_mode(lua, &s2)
        });
    }
    if unsafe { ffi::lua_isyieldable(th.state()) } != 0 {
        Ok(VmState::Yield)
    } else {
        let why = s.stopped().map(|r| r.label()).unwrap_or("unknown");
        Err(LuaError::RuntimeError(format!("budget exceeded: {why}")))
    }
}

fn on_hook(lua: &Lua, s: &Arc<Shared>) -> LuaResult<VmState> {
    if s.stopped().is_none() {
        let ticks = s.ticks.fetch_add(1, Relaxed) + 1;
        let used = lua.used_memory();
        s.peak_mem.fetch_max(used, Relaxed);
        let b = &s.budgets;
        if ticks.saturating_mul(s.every) > b.instructions {
            s.trip(Reason::Instructions);
        } else if s.cancel.load(Relaxed) {
            s.trip(Reason::Cancelled);
        } else if s.started.elapsed() > b.wall {
            s.trip(Reason::WallClock);
        } else if b.memory > 0 && used > b.memory {
            s.trip(Reason::Memory);
        } else if lua.inspect_stack(b.depth, |_| ()).is_some() {
            s.trip(Reason::StackDepth);
        }
        if s.stopped().is_none() {
            return Ok(VmState::Continue);
        }
    }
    stop_mode(lua, s)
}

#[derive(Clone, Debug)]
pub enum Finish {
    Returned(String),
    Yielded,
    Failed(String),
    Compile(String),
}

#[derive(Clone, Debug)]
pub struct RunReport {
    pub finish: Finish,
    pub stop: Option<Reason>,
    pub elapsed: Duration,
    pub ticks: u64,
    pub ctx_calls: u64,
    pub notify: Vec<String>,
    pub peak_mem: usize,
}

impl RunReport {
    pub fn finish_text(&self) -> String {
        match &self.finish {
            Finish::Returned(v) => format!("returned {v:?}"),
            Finish::Yielded => "yielded (stopped)".into(),
            Finish::Failed(e) => format!("error: {e}"),
            Finish::Compile(e) => format!("compile error: {e}"),
        }
    }
    pub fn stop_label(&self) -> &'static str {
        self.stop.map(|r| r.label()).unwrap_or("none")
    }
}

pub struct Vm {
    pub lua: Lua,
    runner: Function,
    pub env: Table,
    pub shared: Arc<Shared>,
    pub profile: Profile,
}

fn es<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Fresh VM: safe stdlib subset, allow-list env, read-only proxies, ctx, budgets and hook.
pub fn new_vm(
    budgets: Budgets,
    profile: Profile,
    cancel: Arc<AtomicBool>,
    rx: Option<mpsc::Receiver<String>>,
) -> Result<Vm, String> {
    let shared = Arc::new(Shared {
        budgets,
        naive: profile.naive_error,
        switch: !profile.no_switch,
        every: profile.hook_every as u64,
        started: Instant::now(),
        cancel,
        stop: AtomicU8::new(0),
        ticks: AtomicU64::new(0),
        peak_mem: AtomicUsize::new(0),
        ctx_calls: AtomicU64::new(0),
        notify: Mutex::new(Vec::new()),
        recv: Mutex::new(rx),
    });
    let libs = StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8;
    let lua = Lua::new_with(libs, LuaOptions::default()).map_err(es)?;
    lua.set_memory_limit(budgets.memory).map_err(es)?;

    let h = lua.create_table().map_err(es)?;
    let s = shared.clone();
    h.set(
        "notify",
        lua.create_function(move |_: &Lua, text: String| {
            charge(&s)?;
            s.notify.lock().unwrap().push(text);
            Ok(())
        })
        .map_err(es)?,
    )
    .map_err(es)?;
    let s = shared.clone();
    h.set(
        "count",
        lua.create_function(move |_: &Lua, _: ()| -> LuaResult<u64> { charge(&s) })
            .map_err(es)?,
    )
    .map_err(es)?;
    let s = shared.clone();
    h.set(
        "now",
        lua.create_function(move |_: &Lua, _: ()| -> LuaResult<i64> {
            Ok(1_700_000_000 + charge(&s)? as i64)
        })
        .map_err(es)?,
    )
    .map_err(es)?;
    let s = shared.clone();
    h.set(
        "block_recv",
        lua.create_function(move |_: &Lua, _: ()| -> LuaResult<String> { block_recv(&s) })
            .map_err(es)?,
    )
    .map_err(es)?;
    let s = shared.clone();
    h.set(
        "trip",
        lua.create_function(move |_: &Lua, label: String| {
            if let Some(r) = Reason::from_label(&label) {
                s.trip(r);
            }
            Ok(())
        })
        .map_err(es)?,
    )
    .map_err(es)?;
    if profile.wrap_rep {
        let s = shared.clone();
        let rep = lua
            .create_function(
                move |lua: &Lua, (src, n, sep): (mlua::String, i64, Option<mlua::String>)| -> LuaResult<mlua::String> {
                    if n <= 0 {
                        return lua.create_string(b"");
                    }
                    let sep_len = sep.as_ref().map(|x| x.as_bytes().len()).unwrap_or(0) as u128;
                    let total = (src.as_bytes().len() as u128 + sep_len) * n as u128;
                    if total > s.budgets.string_max as u128 {
                        s.trip(Reason::StringSize);
                        return Err(LuaError::RuntimeError(format!(
                            "budget exceeded: string_size ({total} bytes requested, limit {})",
                            s.budgets.string_max
                        )));
                    }
                    let mut out = Vec::with_capacity(total as usize);
                    for i in 0..n {
                        if i > 0 {
                            if let Some(sp) = &sep {
                                out.extend_from_slice(&sp.as_bytes());
                            }
                        }
                        out.extend_from_slice(&src.as_bytes());
                    }
                    lua.create_string(&out)
                },
            )
            .map_err(es)?;
        h.set("rep", rep).map_err(es)?;
    }
    h.set("keep_pcall", profile.keep_pcall).map_err(es)?;
    h.set("keep_rawset", profile.keep_rawset).map_err(es)?;
    h.set("no_ctx", profile.no_ctx).map_err(es)?;

    let (runner, env): (Function, Table) = lua
        .load(PRELUDE)
        .set_name("=prelude")
        .call(h)
        .map_err(es)?;

    if profile.hooks && !profile.per_thread_root {
        let s = shared.clone();
        lua.set_global_hook(
            HookTriggers::new().every_nth_instruction(profile.hook_every),
            move |lua: &Lua, _: &Debug| on_hook(lua, &s),
        )
        .map_err(es)?;
    }
    Ok(Vm { lua, runner, env, shared, profile })
}

impl Vm {
    /// Evaluates a host-written table constructor (trusted text, not the script).
    pub fn input(&self, table_src: &str) -> Result<Table, String> {
        self.lua.load(format!("return {table_src}")).eval::<Table>().map_err(es)
    }

    /// Compiles `src` as a module in this VM's sandbox env and runs `execute` in a new thread
    /// (a Lua coroutine) so the budget hook can yield it out.
    pub fn run_module(&self, src: &str, input: Option<Table>) -> RunReport {
        let t0 = Instant::now();
        let modfn = self
            .lua
            .load(src)
            .set_name("workflow.lua")
            .set_environment(self.env.clone())
            .into_function();
        let modfn = match modfn {
            Ok(f) => f,
            Err(LuaError::SyntaxError { message, .. }) => {
                return self.report(Finish::Compile(message), t0);
            }
            Err(e) => return self.report(Finish::Compile(e.to_string()), t0),
        };
        let th = match self.lua.create_thread(self.runner.clone()) {
            Ok(t) => t,
            Err(e) => return self.report(Finish::Failed(e.to_string()), t0),
        };
        if self.profile.per_thread_root && self.profile.hooks {
            let s = self.shared.clone();
            let _ = th.set_hook(
                HookTriggers::new().every_nth_instruction(self.profile.hook_every),
                move |lua: &Lua, _: &Debug| on_hook(lua, &s),
            );
        }
        let res = th.resume::<Value>((modfn, input));
        let status = th.status();
        let finish = match res {
            Ok(v) => {
                if matches!(status, ThreadStatus::Resumable) {
                    Finish::Yielded
                } else {
                    Finish::Returned(render(&v))
                }
            }
            Err(LuaError::MemoryError(m)) => {
                self.shared.trip(Reason::Memory);
                Finish::Failed(m)
            }
            Err(e) => {
                let text = e.to_string();
                let short = text.split("\nstack traceback:").next().unwrap_or("").to_string();
                Finish::Failed(short)
            }
        };
        self.report(finish, t0)
    }

    fn report(&self, finish: Finish, t0: Instant) -> RunReport {
        RunReport {
            finish,
            stop: self.shared.stopped(),
            elapsed: t0.elapsed(),
            ticks: self.shared.ticks.load(Relaxed),
            ctx_calls: self.shared.ctx_calls.load(Relaxed),
            notify: self.shared.notify.lock().unwrap().clone(),
            peak_mem: self.shared.peak_mem.load(Relaxed),
        }
    }
}

fn render(v: &Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.to_str().map(|x| x.to_string()).unwrap_or_else(|_| "<bytes>".into()),
        other => other.type_name().to_string(),
    }
}

/// Runs `src` in a fresh VM on its own OS thread. Returns None if the run did not finish within
/// `timeout`. Then the cancel flag is set and a second, short wait is made; `Some(Err(report))`
/// means it only ended after the host cancelled it, and `None` twice means it never ended.
pub enum Timed {
    Done(RunReport),
    EndedAfterCancel(RunReport),
    Hung,
}

pub fn exec_timed(src: &str, budgets: Budgets, profile: Profile, timeout: Duration) -> Timed {
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let c2 = cancel.clone();
    let src = src.to_string();
    std::thread::spawn(move || {
        let rep = match new_vm(budgets, profile, c2, None) {
            Ok(vm) => {
                let input = None;
                vm.run_module(&src, input)
            }
            Err(e) => RunReport {
                finish: Finish::Compile(format!("vm setup: {e}")),
                stop: None,
                elapsed: Duration::ZERO,
                ticks: 0,
                ctx_calls: 0,
                notify: vec![],
                peak_mem: 0,
            },
        };
        let _ = tx.send(rep);
    });
    match rx.recv_timeout(timeout) {
        Ok(r) => Timed::Done(r),
        Err(_) => {
            cancel.store(true, SeqCst);
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(r) => Timed::EndedAfterCancel(r),
                Err(_) => Timed::Hung,
            }
        }
    }
}

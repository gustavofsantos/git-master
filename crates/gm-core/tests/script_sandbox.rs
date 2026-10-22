//! The Luau sandbox (D99), end to end: escape attempts, tampering between runs, budgets,
//! cancellation, the `ctx` surface and the checks of a module. Each run is a fresh VM, so
//! nothing here depends on the order of the tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::thread;
use std::time::{Duration, Instant};

use gm_core::script::fake::ScriptedHost;
use gm_core::script::luau::LuauEngine;
use gm_core::script::port::{
    BudgetKind, Budgets, CtxHost, Failure, HostEnd, HostReply, RunEnd, RunRequest, ScriptEngine,
};
use gm_core::script::surface::CtxSurface;
use serde_json::{Value, json};

/// Small budgets, so each budget test ends in milliseconds.
fn small() -> Budgets {
    Budgets {
        steps: 200_000,
        script_time: Duration::from_millis(300),
        memory: 8 << 20,
        string_bytes: 1 << 20,
        ctx_calls: 1000,
    }
}

/// `M.execute` with `body` as its body, and the module shape the spec requires.
fn module(body: &str) -> String {
    format!("local M = {{}}\nfunction M.execute(ctx, input)\n{body}\nend\nreturn M\n")
}

fn input() -> Value {
    json!({"inputs": {"ticket": "ABC-1"}, "trigger": {}, "repo": {"root": "/repo", "branch": "main"}})
}

/// Runs `source` against `host` with `budgets`, through the port.
fn run_with(source: &str, host: &mut dyn CtxHost, budgets: Budgets) -> RunEnd {
    let surface = CtxSurface::from_catalog();
    let input = input();
    let cancel = Arc::new(AtomicBool::new(false));
    let request = RunRequest {
        chunk: "flow.lua",
        source,
        input: &input,
        surface: &surface,
        budgets,
        cancel: Arc::clone(&cancel),
    };
    LuauEngine::new().run(&request, host)
}

/// Runs a module body with a scripted host, and returns the end and the host (for its calls).
fn run_body(body: &str, host: ScriptedHost) -> (RunEnd, ScriptedHost) {
    let mut host = host;
    let end = run_with(&module(body), &mut host, small());
    (end, host)
}

/// The message of a `Failed(Script)` end, or a panic that names the actual end.
fn script_message(end: &RunEnd) -> &str {
    match end {
        RunEnd::Failed(Failure::Script(message)) => message,
        other => panic!("expected Failed(Script), got {other:?}"),
    }
}

fn returned(end: RunEnd) -> Value {
    match end {
        RunEnd::Returned(value) => value,
        other => panic!("expected Returned, got {other:?}"),
    }
}

// -- 1. Escape attempts ---------------------------------------------------------------------

/// A case: its name, the body of `M.execute`, and what the error must say.
type Case = (&'static str, String, &'static str);

/// The first half of the attempts: the process, the files and the loaders.
fn escape_cases_system(write_probe: &str) -> Vec<Case> {
    vec![
        ("os.execute", "os.execute('echo PWNED')".into(), "execute"),
        ("os.getenv", "os.getenv('HOME')".into(), "getenv"),
        ("os.time", "os.time()".into(), "time"),
        ("os.clock", "os.clock()".into(), "clock"),
        ("os.date", "os.date('%Y')".into(), "date"),
        ("os.exit", "os.exit(0)".into(), "exit"),
        ("io.open (probe)", write_probe.to_owned(), "open"),
        ("io.popen", "io.popen('echo PWNED')".into(), "popen"),
        ("io.read", "io.read()".into(), "read"),
        ("load", "load('return 1 + 1')()".into(), "call a nil value"),
        (
            "loadstring",
            "loadstring('return 2')()".into(),
            "call a nil value",
        ),
        (
            "dofile",
            "dofile('/etc/hostname')".into(),
            "call a nil value",
        ),
        (
            "loadfile",
            "loadfile('/etc/hostname')".into(),
            "call a nil value",
        ),
        ("require", "require('os')".into(), "call a nil value"),
        ("package", "return package.loaded".into(), "loaded"),
        (
            "string.dump",
            "return string.dump(function() end)".into(),
            "call a nil value",
        ),
        ("debug.getinfo", "return debug.getinfo(1)".into(), "getinfo"),
        (
            "debug.sethook",
            "debug.sethook(function() end, '', 1)".into(),
            "sethook",
        ),
        (
            "debug.getregistry",
            "return debug.getregistry()".into(),
            "getregistry",
        ),
        (
            "debug.traceback",
            "return debug.traceback()".into(),
            "traceback",
        ),
        (
            "collectgarbage",
            "return collectgarbage('count')".into(),
            "call a nil value",
        ),
    ]
}

/// The second half: the globals, the metatables and `ctx` itself.
fn escape_cases_environment() -> Vec<Case> {
    vec![
        ("gcinfo", "return gcinfo()".into(), "call a nil value"),
        ("getfenv", "return getfenv(1)".into(), "call a nil value"),
        ("setfenv", "setfenv(1, {})".into(), "call a nil value"),
        (
            "newproxy",
            "return newproxy(true)".into(),
            "call a nil value",
        ),
        ("print", "print('leak')".into(), "call a nil value"),
        ("warn", "warn('leak')".into(), "call a nil value"),
        (
            "math.random",
            "return math.random(1, 100)".into(),
            "call a nil value",
        ),
        (
            "math.randomseed",
            "math.randomseed(42)".into(),
            "call a nil value",
        ),
        (
            "string metatable __index",
            "getmetatable('').__index = {}".into(),
            "readonly table",
        ),
        (
            "string.format replaced",
            "string.format = function() return 'PWNED' end".into(),
            "readonly table",
        ),
        (
            "setmetatable(_G, {})",
            "setmetatable(_G, {})".into(),
            "readonly table",
        ),
        (
            "rawset(_G, ...)",
            "rawset(_G, 'leak', 1)".into(),
            "readonly table",
        ),
        ("_G.leak = 1", "_G.leak = 1".into(), "readonly table"),
        ("_ENV.leak = 1", "_ENV.leak = 1".into(), "leak"),
        (
            "ctx.notify = nil",
            "ctx.notify = nil".into(),
            "readonly table",
        ),
        ("ctx.leak = 1", "ctx.leak = 1".into(), "readonly table"),
        (
            "ctx.branch.create = f",
            "ctx.branch.create = function() end".into(),
            "readonly table",
        ),
        (
            "rawset(ctx, ...)",
            "rawset(ctx, 'x', 1)".into(),
            "readonly table",
        ),
        (
            "input.repo = x",
            "input.repo = 'x'".into(),
            "readonly table",
        ),
        (
            "input.repo.root = x",
            "input.repo.root = 'x'".into(),
            "readonly table",
        ),
        ("ctx.nosuch", "ctx.nosuch()".into(), "does not exist"),
        ("ctx.__index", "return ctx.__index".into(), "does not exist"),
    ]
}

/// Each attempt is the whole body, with no `pcall`: a blocked attempt is an uncaught error, so
/// the run ends `Failed(Script)` and the message names what is missing.
#[test]
fn escape_attempts_end_failed_and_reach_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let probe = dir.path().join("probe");
    let write_probe = format!("io.open('{}', 'w')", probe.display());
    let mut cases = escape_cases_system(&write_probe);
    cases.extend(escape_cases_environment());
    for (name, body, fragment) in cases {
        let (end, host) = run_body(&body, ScriptedHost::new());
        let message = script_message(&end);
        assert!(
            message.contains(fragment),
            "{name}: message {message:?} does not name {fragment:?}"
        );
        assert!(
            host.calls.is_empty(),
            "{name}: host was called: {:?}",
            host.calls
        );
    }
    assert!(!probe.exists(), "the probe file was created");
}

/// A run that writes to a global, or to a table of a library, changes only its own VM. The
/// tamper test below shows the next run sees the originals.
#[test]
fn getmetatable_of_ctx_is_locked_and_hides_the_internals() {
    let (end, host) = run_body(
        "local mt = getmetatable(ctx)\n\
         if mt ~= 'locked' then error('metatable exposed: ' .. tostring(mt)) end\n\
         ctx.nosuch()",
        ScriptedHost::new(),
    );
    assert!(script_message(&end).contains("does not exist"), "{end:?}");
    assert!(host.calls.is_empty());
}

#[test]
fn iterating_ctx_lists_only_the_public_names() {
    let (end, host) = run_body(
        "local names = {}\n\
         for k in pairs(ctx) do names[#names + 1] = k end\n\
         table.sort(names)\n\
         error('names: ' .. table.concat(names, ','))",
        ScriptedHost::new(),
    );
    let message = script_message(&end);
    assert!(
        message.contains("branch") && message.contains("notify"),
        "{message}"
    );
    assert!(
        message.contains("try") && message.contains("now"),
        "{message}"
    );
    assert!(
        !message.contains("host") && !message.contains("__"),
        "{message}"
    );
    assert!(host.calls.is_empty());
}

#[test]
fn a_coroutine_that_spins_is_stopped_by_the_step_budget() {
    let (end, host) = run_body(
        "local f = coroutine.wrap(function() while true do end end)\nf()\nctx.notify('after')",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::Steps)));
    assert!(host.calls.is_empty());
}

#[test]
fn a_yield_cannot_cross_a_host_call() {
    let (end, host) = run_body(
        "local co = coroutine.create(function()\n\
           return ctx.try(function() coroutine.yield('x'); ctx.notify('after') end)\n\
         end)\n\
         local ok, r = coroutine.resume(co)\n\
         error('resume=' .. tostring(ok) .. ' ' .. tostring(r))",
        ScriptedHost::new(),
    );
    // The yield does not reach the host: whatever the coroutine returns, no call is made.
    assert!(matches!(end, RunEnd::Failed(_)), "{end:?}");
    assert!(host.calls.is_empty(), "host calls: {:?}", host.calls);
}

#[test]
fn a_string_rep_bomb_ends_as_a_string_size_budget() {
    let (end, _) = run_body(
        "local s = string.rep('x', 2147483648)\nreturn #s",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::StringSize)));
}

#[test]
fn a_table_growth_bomb_ends_as_a_memory_budget() {
    let budgets = Budgets {
        steps: 10_000_000,
        ..small()
    };
    let mut host = ScriptedHost::new();
    let end = run_with(
        &module("local t = {}\nfor i = 1, 1000000000 do t[i] = i end\nreturn #t"),
        &mut host,
        budgets,
    );
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::Memory)));
}

#[test]
fn runaway_concatenation_ends_as_a_memory_budget() {
    let (end, _) = run_body(
        "local s = 'x'\nfor i = 1, 60 do s = s .. s end\nreturn #s",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::Memory)));
}

#[test]
fn an_infinite_loop_ends_as_a_steps_budget() {
    let (end, _) = run_body("while true do end\nreturn 1", ScriptedHost::new());
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::Steps)));
}

#[test]
fn an_infinite_tail_recursion_is_stopped_too() {
    // Either the call interrupt or the call depth ends it; both are budgets of the run.
    let (end, _) = run_body(
        "local function f() return f() end\nreturn f()",
        ScriptedHost::new(),
    );
    assert!(
        matches!(
            end,
            RunEnd::Failed(Failure::Budget(BudgetKind::Steps | BudgetKind::StackDepth))
        ),
        "{end:?}"
    );
}

#[test]
fn deep_non_tail_recursion_ends_as_a_call_depth_budget() {
    let surface = CtxSurface::from_catalog();
    let input = input();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut host = ScriptedHost::new();
    let source = module("local function f(n) return 1 + f(n + 1) end\nreturn f(1)");
    let request = RunRequest {
        chunk: "flow.lua",
        source: &source,
        input: &input,
        surface: &surface,
        budgets: Budgets::default(),
        cancel: Arc::clone(&cancel),
    };
    let end = LuauEngine::new().run(&request, &mut host);
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::StackDepth)));
}

#[test]
fn a_flood_of_ctx_calls_ends_as_a_ctx_call_budget_at_the_limit() {
    let (end, host) = run_body(
        "for i = 1, 100000 do ctx.notify('x') end",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::CtxCalls)));
    assert_eq!(
        host.calls.len(),
        1000,
        "the 1001st call must not reach the host"
    );
}

#[test]
fn pcall_cannot_swallow_a_budget_and_go_on() {
    let (end, host) = run_body(
        "local ok = pcall(function() while true do end end)\n\
         ctx.notify('after ' .. tostring(ok))",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::Steps)));
    assert!(
        host.calls.is_empty(),
        "the code after pcall ran: {:?}",
        host.calls
    );
}

#[test]
fn xpcall_cannot_swallow_a_budget_and_go_on() {
    let (end, host) = run_body(
        "local ok = xpcall(function() while true do end end, function(e) return e end)\n\
         ctx.notify('after ' .. tostring(ok))",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::Steps)));
    assert!(
        host.calls.is_empty(),
        "the code after xpcall ran: {:?}",
        host.calls
    );
}

#[test]
fn pcall_in_a_loop_cannot_swallow_a_budget() {
    let (end, host) = run_body(
        "local n = 0\n\
         while true do\n\
           pcall(function() while true do end end)\n\
           n = n + 1\n\
         end",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::Steps)));
    assert!(host.calls.is_empty());
}

#[test]
fn ctx_try_cannot_swallow_a_budget() {
    let (end, host) = run_body(
        "local r = ctx.try(function() while true do end end)\nctx.notify(r.outcome)",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::Steps)));
    assert!(host.calls.is_empty());
}

#[test]
fn pcall_cannot_swallow_an_outcome_from_a_ctx_call() {
    let host = ScriptedHost::new().reply(
        "restack.apply",
        HostReply::Outcome {
            outcome: "conflict".into(),
            value: json!({}),
            message: "feat/b conflicts".into(),
        },
    );
    let (end, host) = run_body(
        "pcall(function() ctx.restack.apply({ plan_id = 'p1' }) end)\n\
         ctx.notify('after')",
        host,
    );
    assert_eq!(
        end,
        RunEnd::Failed(Failure::Outcome {
            func: "restack.apply".into(),
            outcome: "conflict".into(),
            message: "feat/b conflicts".into(),
        })
    );
    assert_eq!(host.names(), ["restack.apply"]);
}

#[test]
fn pcall_of_ctx_fail_does_not_continue() {
    let (end, host) = run_body(
        "pcall(function() ctx.fail('nope') end)\nctx.notify('after')",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Failed(Failure::Reason("nope".into())));
    assert_eq!(host.names(), ["fail"]);
}

#[test]
fn pcall_of_ctx_block_does_not_continue() {
    let (end, host) = run_body(
        "pcall(function() ctx.block('no') end)\nctx.notify('after')",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Blocked("no".into()));
    assert_eq!(host.names(), ["block"]);
}

#[test]
fn error_objects_that_are_not_strings_end_failed() {
    for body in [
        "error({ code = 7 })",
        "error(function() end)",
        "error(setmetatable({}, { __tostring = function() return 'custom' end }))",
        "error(nil)",
    ] {
        let (end, host) = run_body(body, ScriptedHost::new());
        assert!(
            matches!(end, RunEnd::Failed(Failure::Script(_))),
            "{body}: {end:?}"
        );
        assert!(host.calls.is_empty());
    }
}

// -- 2. Tamper between runs -----------------------------------------------------------------

#[test]
fn a_run_that_tampers_with_the_builtins_does_not_change_the_next_run() {
    let tamper = module(
        "local res = {}\n\
         local function try(name, f) local ok = pcall(f); res[#res + 1] = name .. '=' .. (ok and 'changed' or 'refused') end\n\
         try('string.format', function() string.format = function() return 'PWNED' end end)\n\
         try('table.insert', function() table.insert = function() end end)\n\
         try('math.pi', function() math.pi = 3 end)\n\
         try('string.rep', function() string.rep = function() return 'x' end end)\n\
         try('ctx.notify', function() ctx.notify = function() end end)\n\
         try('_G.leak', function() _G.leak = 1 end)\n\
         try('global', function() leak = 'bare' end)\n\
         try('string-meta', function() getmetatable('').__index = {} end)\n\
         return table.concat(res, ' ')",
    );
    let observe = module(
        "return table.concat({\n\
           tostring(_G.leak), tostring(leak), string.format('%d', 7), tostring(table.insert ~= nil),\n\
           tostring(math.pi), ('x'):upper(), tostring(string.rep('a', 2)), type(ctx.notify),\n\
         }, ' | ')",
    );
    let mut host = ScriptedHost::new();
    let first = run_with(&tamper, &mut host, small());
    let refused = returned(first);
    assert_eq!(
        refused.as_str().unwrap(),
        "string.format=refused table.insert=refused math.pi=refused string.rep=refused \
         ctx.notify=refused _G.leak=refused global=refused string-meta=refused"
    );
    let second = returned(run_with(&observe, &mut host, small()));
    assert_eq!(
        second.as_str().unwrap(),
        "nil | nil | 7 | true | 3.141592653589793 | X | aa | function"
    );
}

// -- 3. Budgets -----------------------------------------------------------------------------

#[test]
fn a_script_time_budget_ends_a_run_with_no_steps_limit() {
    let budgets = Budgets {
        steps: u64::MAX,
        script_time: Duration::from_millis(300),
        ..small()
    };
    let started = Instant::now();
    let mut host = ScriptedHost::new();
    let end = run_with(&module("while true do end"), &mut host, budgets);
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::Time)));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_memory_budget_ends_a_run() {
    // The steps budget is raised so that the memory is what runs out first.
    let budgets = Budgets {
        steps: 10_000_000,
        ..small()
    };
    let mut host = ScriptedHost::new();
    let end = run_with(
        &module("local t = {}\nfor i = 1, 1000000000 do t[i] = i end"),
        &mut host,
        budgets,
    );
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::Memory)));
}

#[test]
fn a_string_size_budget_ends_a_run() {
    let budgets = Budgets {
        string_bytes: 1 << 20,
        ..small()
    };
    let mut host = ScriptedHost::new();
    let end = run_with(
        &module("return #string.rep('x', 2097152)"),
        &mut host,
        budgets,
    );
    assert_eq!(end, RunEnd::Failed(Failure::Budget(BudgetKind::StringSize)));
}

#[test]
fn a_well_behaved_loop_completes() {
    let mut host = ScriptedHost::new();
    let end = run_with(
        &module("local s = 0\nfor i = 1, 100000 do s = s + i * 2 % 7 end\nreturn s"),
        &mut host,
        small(),
    );
    assert_eq!(returned(end), json!(300_001));
}

/// A host call that takes longer than the whole script budget does not count against it.
#[test]
fn time_inside_a_host_call_is_not_script_time() {
    struct SlowHost;
    impl CtxHost for SlowHost {
        fn call(&mut self, _func: &str, _args: Value) -> HostReply {
            thread::sleep(Duration::from_millis(400));
            HostReply::Value(json!({"text": "ok", "exit_code": 0}))
        }
    }
    let budgets = Budgets {
        script_time: Duration::from_millis(200),
        ..small()
    };
    let mut host = SlowHost;
    let end = run_with(
        &module("local r = ctx.exec({ 'slow' })\nreturn r.text"),
        &mut host,
        budgets,
    );
    assert_eq!(returned(end), json!("ok"));
}

// -- 4. Cancellation ------------------------------------------------------------------------

#[test]
fn a_cancel_from_another_thread_stops_a_tight_loop_within_half_a_second() {
    let cancel = Arc::new(AtomicBool::new(false));
    let budgets = Budgets {
        steps: u64::MAX,
        script_time: Duration::from_secs(60),
        ..small()
    };
    let source = module("while true do end");
    let surface = CtxSurface::from_catalog();
    let input = input();
    let request = RunRequest {
        chunk: "flow.lua",
        source: &source,
        input: &input,
        surface: &surface,
        budgets,
        cancel: Arc::clone(&cancel),
    };
    let mut host = ScriptedHost::new();
    let end = thread::scope(|scope| {
        let setter = scope.spawn(|| {
            thread::sleep(Duration::from_millis(100));
            let set_at = Instant::now();
            cancel.store(true, SeqCst);
            set_at
        });
        let end = LuauEngine::new().run(&request, &mut host);
        let set_at = setter.join().expect("setter");
        assert!(
            set_at.elapsed() < Duration::from_millis(500),
            "late cancel: {:?}",
            set_at.elapsed()
        );
        end
    });
    assert_eq!(end, RunEnd::Cancelled);
}

#[test]
fn a_host_call_that_reports_cancellation_ends_the_run_and_later_code_does_not_run() {
    let host = ScriptedHost::new().reply("exec", HostReply::End(HostEnd::Cancelled));
    let (end, host) = run_body("ctx.exec({ 'sleep', '60' })\nctx.notify('later')", host);
    assert_eq!(end, RunEnd::Cancelled);
    assert_eq!(host.names(), ["exec"]);
}

// -- 5. ctx behaviour -----------------------------------------------------------------------

#[test]
fn an_unknown_name_lists_the_valid_names_at_its_level() {
    let (end, _) = run_body("ctx.branch.make({})", ScriptedHost::new());
    assert_eq!(
        script_message(&end),
        "ctx.branch.make does not exist. Valid names at ctx.branch: checkout, create"
    );
}

#[test]
fn an_unknown_top_level_name_lists_the_top_level_names() {
    let (end, _) = run_body("ctx.nope()", ScriptedHost::new());
    let message = script_message(&end);
    assert!(
        message.starts_with("ctx.nope does not exist. Valid names at ctx: "),
        "{message}"
    );
    assert!(
        message.contains("branch") && message.contains("try"),
        "{message}"
    );
}

#[test]
fn ctx_try_reports_done_with_the_fields_of_the_value() {
    let (end, host) = run_body(
        "local r = ctx.try(function() return ctx.exec({ 'git', 'rev-parse' }) end)\n\
         return { outcome = r.outcome, text = r.text, exit = r.exit_code }",
        ScriptedHost::new().reply(
            "exec",
            HostReply::Value(json!({"text": "main", "exit_code": 0})),
        ),
    );
    assert_eq!(
        returned(end),
        json!({"outcome": "done", "text": "main", "exit": 0})
    );
    assert_eq!(host.names(), ["exec"]);
}

#[test]
fn ctx_try_returns_a_conflict_outcome_without_raising() {
    let host = ScriptedHost::new().reply(
        "restack.apply",
        HostReply::Outcome {
            outcome: "conflict".into(),
            value: json!({"branch": "feat/b"}),
            message: "feat/b conflicts".into(),
        },
    );
    let (end, host) = run_body(
        "local r = ctx.try(function() return ctx.restack.apply({ plan_id = 'p1' }) end)\n\
         return { r.outcome, r.message, r.branch }",
        host,
    );
    assert_eq!(
        returned(end),
        json!(["conflict", "feat/b conflicts", "feat/b"])
    );
    assert_eq!(host.names(), ["restack.apply"]);
}

#[test]
fn ctx_try_of_a_function_without_a_ctx_call_is_done_with_its_return() {
    let (end, host) = run_body(
        "return ctx.try(function() return 42 end)",
        ScriptedHost::new(),
    );
    assert_eq!(returned(end), json!({"outcome": "done", "value": 42}));
    assert!(host.calls.is_empty());
}

#[test]
fn ctx_try_rethrows_an_ordinary_lua_error() {
    let (end, _) = run_body("ctx.try(function() error('boom') end)", ScriptedHost::new());
    let message = script_message(&end);
    assert!(message.contains("boom"), "{message}");
    assert!(!message.contains("outcome"), "{message}");
}

#[test]
fn ctx_try_does_not_swallow_a_ctx_fail() {
    let (end, host) = run_body(
        "ctx.try(function() ctx.fail('stop') end)\nctx.notify('after')",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Failed(Failure::Reason("stop".into())));
    assert_eq!(host.names(), ["fail"]);
}

#[test]
fn an_outcome_outside_ctx_try_ends_the_run() {
    let host = ScriptedHost::new().reply(
        "restack.apply",
        HostReply::Outcome {
            outcome: "stale".into(),
            value: json!({}),
            message: String::new(),
        },
    );
    let (end, host) = run_body(
        "ctx.restack.apply({ plan_id = 'p1' })\nctx.notify('after')",
        host,
    );
    assert_eq!(
        end,
        RunEnd::Failed(Failure::Outcome {
            func: "restack.apply".into(),
            outcome: "stale".into(),
            message: String::new(),
        })
    );
    assert_eq!(host.names(), ["restack.apply"]);
}

#[test]
fn positional_arguments_and_options_reach_the_host_as_named_fields() {
    let (end, host) = run_body(
        "ctx.exec({ 'git', 'status' }, { timeout = '30s' })\n\
         ctx.notify('x')\n\
         ctx.branch.create({ name = 'a', checkout = true })\n\
         ctx.stage.all()",
        ScriptedHost::new(),
    );
    assert!(matches!(end, RunEnd::Returned(_)), "{end:?}");
    assert_eq!(
        host.calls[0],
        (
            "exec".into(),
            json!({"argv": ["git", "status"], "timeout": "30s"})
        )
    );
    assert_eq!(host.calls[1], ("notify".into(), json!({"text": "x"})));
    assert_eq!(
        host.calls[2],
        (
            "branch.create".into(),
            json!({"name": "a", "checkout": true})
        )
    );
    assert_eq!(host.calls[3], ("stage.all".into(), json!({})));
}

#[test]
fn a_positional_value_wins_over_the_same_name_in_the_options() {
    let (_, host) = run_body(
        "ctx.exec({ 'a' }, { argv = { 'b' }, timeout = '1s' })",
        ScriptedHost::new(),
    );
    assert_eq!(host.calls[0].1, json!({"argv": ["a"], "timeout": "1s"}));
}

#[test]
fn a_table_argument_that_is_not_a_table_is_a_lua_error() {
    let (end, host) = run_body("ctx.branch.create('x')", ScriptedHost::new());
    assert!(script_message(&end).contains("ctx.branch.create expects a table"));
    assert!(host.calls.is_empty());
}

#[test]
fn a_function_argument_is_refused_before_the_host() {
    let (end, host) = run_body("ctx.notify(function() end)", ScriptedHost::new());
    assert!(script_message(&end).contains("cannot pass a function to ctx.notify"));
    assert!(host.calls.is_empty());
}

#[test]
fn a_host_value_round_trips_into_lua_with_null_as_nil() {
    let host = ScriptedHost::new().reply(
        "exec",
        HostReply::Value(json!({"items": [1, 2, {"gone": null}], "name": "x"})),
    );
    let (end, _) = run_body("return ctx.exec({ 'list' })", host);
    assert_eq!(returned(end), json!({"items": [1, 2, {}], "name": "x"}));
}

#[test]
fn the_helpers_reach_the_host_under_their_names() {
    let (end, host) = run_body(
        "local t = ctx.now()\nlocal r = ctx.random()\nctx.block('wait for review')",
        ScriptedHost::new(),
    );
    assert_eq!(end, RunEnd::Blocked("wait for review".into()));
    assert_eq!(host.names(), ["now", "random", "block"]);
    assert_eq!(host.calls[2].1, json!({"reason": "wait for review"}));
}

#[test]
fn the_return_value_is_the_json_of_what_execute_returned() {
    let (end, _) = run_body(
        "return { ok = true, n = 3, list = { 'a' } }",
        ScriptedHost::new(),
    );
    assert_eq!(returned(end), json!({"ok": true, "n": 3, "list": ["a"]}));
    let (end, _) = run_body("return nil", ScriptedHost::new());
    assert_eq!(returned(end), Value::Null);
    let (end, _) = run_body("return function() end", ScriptedHost::new());
    assert_eq!(script_message(&end), "execute returned a function");
}

// -- 6. check and read_meta -----------------------------------------------------------------

fn check(source: &str) -> Result<(), Vec<gm_core::script::port::ScriptDiagnostic>> {
    LuauEngine::new().check("flow.lua", source)
}

#[test]
fn check_accepts_a_module_with_execute() {
    assert!(check(&module("ctx.notify('x')")).is_ok());
}

#[test]
fn check_reports_a_syntax_error_on_its_line() {
    let diagnostics =
        check("local M = {}\n\nlocal x = = 1\nfunction M.execute(ctx, input) end\nreturn M\n")
            .expect_err("syntax error");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].line, 3, "{diagnostics:?}");
}

#[test]
fn check_reports_a_missing_execute_at_line_zero() {
    let diagnostics = check("local M = {}\nreturn M\n").expect_err("no execute");
    assert_eq!(diagnostics[0].line, 0);
    assert!(
        diagnostics[0].message.contains("execute"),
        "{diagnostics:?}"
    );
}

#[test]
fn check_reports_an_execute_that_is_not_a_function() {
    let diagnostics = check("local M = { execute = 3 }\nreturn M\n").expect_err("ill-typed");
    assert_eq!(diagnostics[0].line, 0);
}

#[test]
fn check_runs_the_top_level_without_ctx() {
    let diagnostics =
        check("local M = {}\nctx.notify('x')\nfunction M.execute(ctx, input) end\nreturn M\n")
            .expect_err("ctx is not a global");
    assert_eq!(diagnostics[0].line, 2, "{diagnostics:?}");
}

#[test]
fn check_stops_a_top_level_loop_under_the_default_budgets() {
    let diagnostics = check("while true do end\n").expect_err("no module");
    assert!(diagnostics[0].message.contains("budget"), "{diagnostics:?}");
}

#[test]
fn read_meta_returns_the_table_as_json() {
    let source = "local M = {}\n\
                  M.meta = { name = 'branch-from-ticket', inputs = { ticket = { type = 'string' } } }\n\
                  function M.execute(ctx, input) end\n\
                  return M\n";
    let meta = LuauEngine::new()
        .read_meta("flow.lua", source)
        .expect("meta");
    assert_eq!(
        meta,
        json!({"name": "branch-from-ticket", "inputs": {"ticket": {"type": "string"}}})
    );
}

#[test]
fn read_meta_refuses_a_missing_meta_and_a_function_inside_it() {
    let missing = LuauEngine::new()
        .read_meta("flow.lua", &module("ctx.notify('x')"))
        .expect_err("no meta");
    assert!(missing[0].message.contains("missing"), "{missing:?}");

    let with_function = "local M = {}\n\
                         M.meta = { name = 'x', run = function() end }\n\
                         function M.execute(ctx, input) end\n\
                         return M\n";
    let diagnostics = LuauEngine::new()
        .read_meta("flow.lua", with_function)
        .expect_err("function in meta");
    assert_eq!(diagnostics[0].line, 0);
    assert!(
        diagnostics[0].message.contains("function"),
        "{diagnostics:?}"
    );
}

// -- Helpers of the run as a whole ----------------------------------------------------------

#[test]
fn a_run_works_from_a_spawned_thread() {
    let handle = thread::spawn(|| {
        let mut host = ScriptedHost::new();
        run_with(&module("return 'from a thread'"), &mut host, small())
    });
    assert_eq!(
        returned(handle.join().expect("thread")),
        json!("from a thread")
    );
}

#[test]
fn the_failure_message_has_no_traceback() {
    let (end, _) = run_body(
        "local function f() error('deep') end\nf()",
        ScriptedHost::new(),
    );
    let message = script_message(&end);
    assert!(message.contains("deep"), "{message}");
    assert!(!message.contains("stack traceback"), "{message}");
}

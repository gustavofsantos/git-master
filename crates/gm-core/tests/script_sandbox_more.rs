//! More attacks on the Luau sandbox, aimed at the host side of the boundary (D99): what a script
//! can make the Rust code do with the data it passes, and the VM features that run script code
//! outside a plain call. Each case runs under a watchdog, so a hang or a stack overflow fails the
//! test instead of stalling it.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::Duration;

use gm_core::script::fake::ScriptedHost;
use gm_core::script::luau::LuauEngine;
use gm_core::script::port::{Budgets, Failure, RunEnd, RunRequest, ScriptEngine};
use gm_core::script::surface::CtxSurface;
use serde_json::json;

fn small() -> Budgets {
    Budgets {
        steps: 2_000_000,
        script_time: Duration::from_millis(500),
        memory: 16 << 20,
        string_bytes: 1 << 20,
        ctx_calls: 1000,
    }
}

/// Runs a body under a watchdog. Returns the end and the number of host calls.
fn run(body: &str) -> (RunEnd, usize) {
    let source = format!("local M = {{}}\nfunction M.execute(ctx, input)\n{body}\nend\nreturn M\n");
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let surface = CtxSurface::from_catalog();
            let input = json!({});
            let mut host = ScriptedHost::new();
            let request = RunRequest {
                chunk: "flow.lua",
                source: &source,
                input: &input,
                surface: &surface,
                budgets: small(),
                cancel: Arc::new(AtomicBool::new(false)),
            };
            let end = LuauEngine::new().run(&request, &mut host);
            let _ = tx.send((end, host.calls.len()));
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(20))
        .expect("the run hung or crashed the thread")
}

fn failed(body: &str) -> Failure {
    match run(body).0 {
        RunEnd::Failed(f) => f,
        other => panic!("expected a failure, got {other:?} for:\n{body}"),
    }
}

#[test]
fn a_table_that_contains_itself_cannot_cross_to_the_host() {
    let f = failed("local t = {} t.me = t ctx.notify(t)");
    assert!(matches!(f, Failure::Script(_)), "{f:?}");
    let f = failed("local t = {} t.me = t return t");
    assert!(matches!(f, Failure::Script(_)), "{f:?}");
}

#[test]
fn a_table_nested_a_hundred_thousand_levels_deep_cannot_overflow_the_host_stack() {
    let f = failed("local t = {} for i = 1, 100000 do t = { t } end ctx.notify({ text = t })");
    assert!(
        matches!(f, Failure::Script(_) | Failure::Budget(_)),
        "{f:?}"
    );
    let f = failed("local t = {} for i = 1, 100000 do t = { t } end return t");
    assert!(
        matches!(f, Failure::Script(_) | Failure::Budget(_)),
        "{f:?}"
    );
}

#[test]
fn a_huge_table_is_stopped_by_the_memory_budget_not_by_the_host() {
    let f = failed("local t = {} for i = 1, 1e9 do t[i] = i end ctx.notify({ text = t })");
    assert!(matches!(f, Failure::Budget(_)), "{f:?}");
}

#[test]
fn error_values_with_hostile_tostring_do_not_hang_the_host() {
    let f = failed("error(setmetatable({}, { __tostring = function() while true do end end }))");
    assert!(
        matches!(f, Failure::Script(_) | Failure::Budget(_)),
        "{f:?}"
    );
    let f = failed(
        "local x = tostring(setmetatable({}, { __tostring = function() while true do end end }))",
    );
    assert!(matches!(f, Failure::Budget(_)), "{f:?}");
}

#[test]
fn a_hostile_index_metamethod_on_a_value_passed_to_the_host_is_not_run_by_the_host() {
    // The converter reads raw fields; a metamethod that loops would hang it if it ran.
    let (end, _) = run(
        "local t = setmetatable({}, { __index = function() while true do end end }) ctx.notify({ text = t })",
    );
    assert_eq!(end, RunEnd::Returned(serde_json::Value::Null));
}

#[test]
fn memory_bombs_by_the_other_builders_end_as_budget_failures() {
    for body in [
        "local s = 'x' for i = 1, 40 do s = s .. s end",
        "local b = buffer.create(2000000000)",
        "local s = ('x'):rep(2000000000)",
        "local t = table.create(500000000, 0)",
        "local s = string.rep('ab', 1e9, ',')",
        "local t = {} for i = 1, 40 do t = { t, t } end local s = table.concat({ string.rep('x', 900000), string.rep('y', 900000) })  local u = {} for i = 1, 1e7 do u[i] = s end",
    ] {
        let f = failed(body);
        assert!(matches!(f, Failure::Budget(_)), "{body} -> {f:?}");
    }
}

#[test]
fn the_metatable_of_ctx_gives_nothing_and_cannot_be_replaced() {
    let (end, calls) = run("assert(getmetatable(ctx) == 'locked') \
         local ok = pcall(setmetatable, ctx, {}) assert(not ok) \
         local ok2 = pcall(function() ctx.branch.evil = 1 end) assert(not ok2) \
         local ok3 = pcall(rawset, ctx, 'evil', 1) assert(not ok3)");
    assert_eq!(end, RunEnd::Returned(serde_json::Value::Null));
    assert_eq!(calls, 0);
}

#[test]
fn a_coroutine_can_call_ctx_and_cannot_outlive_the_budget() {
    let (end, calls) =
        run("local co = coroutine.wrap(function() ctx.now() return 1 end) assert(co() == 1)");
    assert_eq!(end, RunEnd::Returned(serde_json::Value::Null));
    assert_eq!(calls, 1);
    let f = failed("local co = coroutine.wrap(function() while true do end end) co()");
    assert!(matches!(f, Failure::Budget(_)), "{f:?}");
    let f = failed(
        "local co = coroutine.create(function() while true do end end) \
         local ok = coroutine.resume(co) ctx.notify('after') ",
    );
    assert!(matches!(f, Failure::Budget(_)), "{f:?}");
}

#[test]
fn builtins_with_huge_counts_fail_cleanly() {
    for body in [
        "local x = select('#', table.unpack({}, 1, 1e8))",
        "local x = ('%099999999d'):format(1)",
        "local x = string.char(table.unpack({}, 1, 1e7))",
        "local x = utf8.char(table.unpack({}, 1, 1e7))",
    ] {
        let f = failed(body);
        assert!(
            matches!(f, Failure::Script(_) | Failure::Budget(_)),
            "{body} -> {f:?}"
        );
    }
}

#[test]
fn the_arguments_of_a_call_reach_the_host_with_sorted_keys() {
    // The host, the journal and the dry run read these as text, so the order must not depend on
    // the order Lua visits a table in or on how `serde_json` was built.
    let source = "local M = {}\nfunction M.execute(ctx, input)\n\
        ctx.branch.create({ name = 'x', checkout = true })\n\
        ctx.exec({ 'git' }, { timeout = '30s', stdin = 's' })\nend\nreturn M\n";
    let surface = CtxSurface::from_catalog();
    let input = json!({});
    let mut host = ScriptedHost::new();
    let request = RunRequest {
        chunk: "flow.lua",
        source,
        input: &input,
        surface: &surface,
        budgets: small(),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    LuauEngine::new().run(&request, &mut host);
    let text: Vec<String> = host
        .calls
        .iter()
        .map(|(_, args)| args.to_string())
        .collect();
    assert_eq!(
        text,
        [
            r#"{"checkout":true,"name":"x"}"#,
            r#"{"argv":["git"],"stdin":"s","timeout":"30s"}"#
        ]
    );
}

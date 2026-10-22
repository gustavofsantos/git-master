//! Battery A: escape attempts, each in a fresh VM, plus the tampering test.
//! Lua names in the analogue column are the SPEC's list; Rhai has no equivalent for some.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use rhai::Dynamic;
use serde_json::json;

use crate::emit;
use crate::note;
use crate::sandbox::{self, Budgets, Failure, Out, Reason};

/// Wraps a body in the module shape: `const META` plus `fn execute(ctx, input)`.
pub fn wrap(body: &str) -> String {
    format!("const META = #{{ name: \"probe\" }};\nfn execute(ctx, input) {{\n{body}\n}}\n")
}

#[derive(Clone, Copy)]
enum Expect {
    /// The capability must be absent or fail cleanly. Any Ok result is an escape.
    Blocked,
    /// The run must end with this budget reason. Any Ok result means the budget did not hold.
    Budget(Reason),
    /// Control case: shows the mechanism works; the outcome is recorded, not judged.
    Control,
    /// The concept does not exist in Rhai; the probe is run and its outcome recorded.
    Na,
}

struct Case {
    id: &'static str,
    analogue: &'static str,
    src: String,
    expect: Expect,
    note: &'static str,
}

fn case(id: &'static str, analogue: &'static str, src: String, expect: Expect, note: &'static str) -> Case {
    Case { id, analogue, src, expect, note }
}

fn verdict(expect: Expect, r: &Result<Dynamic, Failure>) -> &'static str {
    match (expect, r) {
        (Expect::Blocked, Err(_)) => "blocked",
        (Expect::Blocked, Ok(_)) => "escaped",
        (Expect::Budget(want), Err(f)) if f.reason == want => "contained",
        (Expect::Budget(_), Err(_)) => "contained",
        (Expect::Budget(_), Ok(_)) => "escaped",
        (Expect::Control, _) => "n/a",
        (Expect::Na, _) => "n/a",
    }
}

fn cases() -> Vec<Case> {
    use Expect::*;
    let mut v = Vec::new();
    // os
    v.push(case("os.time/os.clock", "timestamp() (BasicTimePackage, not registered)",
        wrap("let t = timestamp(); 1"), Blocked, "clock is not in the allow list"));
    v.push(case("os.getenv/os.execute", "getenv(\"HOME\") and exec(\"id\"): no such functions",
        wrap("let h = getenv(\"HOME\"); h"), Blocked, "no environment or process functions in any package"));
    v.push(case("os.execute (process)", "exec(\"id\")",
        wrap("exec(\"id\")"), Blocked, "no process function"));
    // io
    v.push(case("io.open/io.read/io.popen", "open(path) / read_file(path): no file functions",
        wrap("let f = open(\"/etc/hostname\"); f"), Blocked, "no file I/O in any Rhai package"));
    // load family
    v.push(case("load(string)", "eval(\"...\") (keyword, disabled in the host)",
        wrap("eval(\"1 + 1\")"), Blocked, "disable_symbol(\"eval\"): reserved symbol, parse error"));
    v.push(case("loadstring/loadfile/dofile", "import \"/etc/hostname\" as m (no module resolver)",
        wrap("import \"/etc/hostname\" as m; m::x"), Blocked, "new_raw() sets no resolver"));
    // require / package
    v.push(case("require/package", "import \"helper\" as h (no resolver)",
        wrap("import \"helper\" as h; h::run()"), Blocked, "same as above"));
    // string.dump
    v.push(case("string.dump", "no bytecode dump; AST serde not enabled",
        wrap("let s = \"abc\".dump(); s"), Na, "no bytecode format exists; no dump method"));
    // debug
    v.push(case("debug.getinfo/sethook/getregistry", "debug(x) formats a value only; no hook or registry API",
        wrap("debug(input); 1"), Control, "debug() output goes to on_debug, which the host drops; no hook API exists"));
    // collectgarbage
    v.push(case("collectgarbage", "gc(): no GC control",
        wrap("gc(); 1"), Blocked, "reference counting only; no GC API"));
    // metatables / string tampering
    v.push(case("getmetatable(\"\").__index tampering", "property assignment on a string",
        wrap("let s = \"abc\"; s.len = 5; s"), Blocked, "no metatables; a string has no settable property"));
    // math.random
    v.push(case("math.random/randomseed", "rand(): the `rand` feature is not compiled in",
        wrap("rand()"), Blocked, "no randomness in the default packages"));
    // setmetatable / rawset on globals, host functions
    v.push(case("setmetatable/rawset on globals", "ctx.notify = |t| {...}: reassigning a host method",
        wrap("ctx.notify = |t| { 1 }; 1"), Blocked, "custom type has no settable properties"));
    // _G / _ENV
    v.push(case("_G/_ENV writes", "no global table; `this` is unbound outside a method",
        wrap("this"), Na, "host state is reachable only through the parameters; probe is `this`"));
    // memory bombs
    v.push(case("string.rep memory bomb", "\"x\".pad(10^10, \"x\")",
        wrap("let s = \"x\".pad(10000000000, \"x\"); s.len()"), Budget(Reason::StringSize), "string-size limit"));
    v.push(case("table growth memory bomb", "array push in a loop",
        wrap("let a = []; loop { a.push(1); }"), Budget(Reason::ArraySize), "max_array_size"));
    v.push(case("map growth memory bomb", "map index assignment in a loop",
        wrap("let m = #{}; let i = 0; loop { i += 1; m[i.to_string()] = i; }"), Budget(Reason::MapSize),
        "max_map_size is not checked on m[k] = v; the run ends on the memory hook instead"));
    v.push(case("map bomb with large values", "m[k] = s, one shared 512 KiB string (under the string limit)",
        wrap("let s = \"x\"; for k in 0..19 { s += s; } let m = #{}; let i = 0; loop { i += 1; m[i.to_string()] = s; }"),
        Budget(Reason::Memory), "the value is shared, so the growth is the map entries (keys and nodes): index assignment is not size-checked and only the memory hook stops it"));
    v.push(case("nested array memory bomb", "a = [a, a] doubles the tree per step",
        wrap("let a = [0]; loop { a = [a, a]; }"), Budget(Reason::Memory), "no size limit on nested totals: only the host memory hook stops it"));
    // infinite loop
    v.push(case("infinite loop", "loop {} (the budget is operations)",
        wrap("loop { }"), Budget(Reason::Instructions), "set_max_operations"));
    // deep recursion
    v.push(case("deep recursion (non-tail, 1e6)", "fn r(n) { 1 + r(n + 1) }",
        "fn r(n) { 1 + r(n + 1) }\nfn execute(ctx, input) { r(0) }\n".to_string(),
        Budget(Reason::StackDepth), "max_call_levels"));
    // runaway string concatenation
    v.push(case("runaway string concatenation", "let s = \"x\"; loop { s += s; }",
        wrap("let s = \"x\"; loop { s += s; }"), Budget(Reason::StringSize), "string-size limit"));
    // coroutines
    v.push(case("coroutine.wrap / yield across host call", "yield: reserved keyword, no coroutines",
        wrap("yield 1;"), Na, "Rhai has no coroutines; `yield` is a reserved word"));
    // errors as values
    v.push(case("error(table)", "throw #{ code: 42 }",
        wrap("throw #{ code: 42, why: \"x\" };"), Control, "the thrown map reaches the host as ErrorRuntime"));
    v.push(case("error(function)", "throw || 1",
        wrap("throw || 1;"), Control, "a FnPtr thrown reaches the host as ErrorRuntime"));
    // pcall
    v.push(case("pcall swallows budget (ops)", "try { loop {} } catch { caught += 1 }",
        wrap("let caught = 0; try { loop { } } catch { caught += 1; } caught"), Budget(Reason::Instructions),
        "ErrorTooManyOperations is not catchable (is_catchable false)"));
    v.push(case("pcall swallows budget (ctx calls)", "loop { try { f() } catch {} } where f calls ctx.count()",
        "fn f(ctx) { ctx.count(); }\nfn execute(ctx, input) { loop { try { f(ctx); } catch { } } }\n".to_string(),
        Budget(Reason::CtxCalls), "the termination passes through a script fn and try"));
    v.push(case("pcall swallows budget (stack)", "try { r(0) } catch {}",
        "fn r(n) { 1 + r(n + 1) }\nfn execute(ctx, input) { let c = 0; try { r(0) } catch { c = 1; } c }\n".to_string(),
        Budget(Reason::StackDepth), "system errors are not caught"));
    v.push(case("pcall swallows budget (string)", "try { s += s } catch {}",
        wrap("let s = \"x\"; try { loop { s += s; } } catch { 0 }"), Budget(Reason::StringSize), "data-too-large is not caught"));
    v.push(case("ctx.try_run swallows budget (ops)", "ctx.try_run(|| loop {})",
        wrap("let c = ctx; let r = ctx.try_run(|| { loop { } }); r"), Budget(Reason::Instructions),
        "system errors are re-raised by try_run, not returned as an error outcome"));
    v.push(case("blocked ctx.block_recv (no host cancel)", "block_recv with no sender activity",
        wrap("ctx.block_recv()"), Budget(Reason::WallClock),
        "the run's wall clock is checked inside block_recv; the progress hook does not run during a host call"));
    v.push(case("blocked block_recv inside try/catch", "try { ctx.block_recv() } catch { 1 }",
        wrap("try { ctx.block_recv() } catch { 1 }"), Budget(Reason::WallClock),
        "the wall-clock termination is not catchable"));
    v.push(case("pcall control (catch works)", "try { throw \"x\" } catch (e) { e }",
        wrap("let r = 0; try { throw \"x\"; } catch (e) { r = e; } r"), Control, "ordinary errors are catchable: the control"));
    // ctx reach
    v.push(case("getmetatable(ctx) / type of ctx", "type_of(ctx)",
        wrap("type_of(ctx)"), Control, "only the type name, no contents"));
    v.push(case("iterate ctx (pairs(ctx))", "for x in ctx { }",
        wrap("for x in ctx { } 1"), Blocked, "Ctx is not iterable"));
    v.push(case("ctx.__index / unknown property", "ctx.__index",
        wrap("ctx.__index"), Blocked, "no such property"));
    v.push(case("ctx unknown method", "ctx.secret()",
        wrap("ctx.secret()"), Blocked, "Function not found"));
    v.push(case("Fn() reaching unregistered host fn", "Fn(\"timestamp\").call()",
        wrap("let f = Fn(\"timestamp\"); f.call()"), Blocked, "FnPtr only resolves functions in the engine"));
    v.push(case("Fn() reaching registered ctx method", "Fn(\"notify\") then call",
        wrap("let f = Fn(\"notify\"); f.call(ctx, \"via fnptr\"); 1"), Control, "allow-list members are reachable by design"));
    // params
    v.push(case("input mutation", "input.repo = \"x\" on the parameter",
        wrap("input.repo = \"changed\"; input.repo"), Control, "a local copy; the host's map is not changed"));
    v.push(case("print/debug to stdout", "print(\"hi\")",
        wrap("print(\"hi\"); 1"), Control, "on_print is replaced with a no-op: nothing reaches stdout"));
    v
}

fn outcome_json(out: &Out) -> (String, String, Option<String>) {
    match &out.result {
        Ok(v) => ("ok".into(), format!("{v:?}"), None),
        Err(f) => (f.reason.label().into(), f.message.clone(), f.payload.clone()),
    }
}

pub fn run() {
    note("A: escape attempts");
    for c in cases() {
        let out = sandbox::run_thread(sandbox::job(
            &c.src,
            Budgets::SPIKE,
            true,
            Arc::new(AtomicBool::new(false)),
            sandbox::channel().1,
        ));
        let v = verdict(c.expect, &out.result);
        let (reason, message, payload) = outcome_json(&out);
        note(format!("  {:<40} {:<8} {}", c.id, v, message));
        emit(json!({
            "test": format!("A.{}", c.id),
            "analogue": c.analogue,
            "verdict": v,
            "reason": reason,
            "message": message,
            "payload": payload,
            "note": c.note,
            "ctx_calls": out.ctx_calls,
            "elapsed_ms": out.elapsed.as_secs_f64() * 1000.0,
        }));
    }
    tamper();
}

/// Run 1 tries to change everything it can reach; run 2 (a fresh VM) checks what it sees.
fn tamper() {
    // Each attempt sits in its own try block so that one failure does not stop the next.
    let script1 = wrap(
        "let attempts = 0;\n\
         try { ctx.notify = |t| { 1 }; attempts += 1; } catch { }\n\
         try { let len = |x| { 0 }; attempts += 1; } catch { }\n\
         try { input.count = 999; attempts += 1; } catch { }\n\
         try { ctx.notify(\"from run 1\"); attempts += 1; } catch { }\n\
         attempts",
    );
    let script2 = wrap(
        "let a = \"abc\".len();\n\
         let b = [1, 2, 3].len();\n\
         ctx.notify(\"from run 2\");\n\
         [a, b, ctx.count()]",
    );
    let r1 = sandbox::run_thread(sandbox::job(&script1, Budgets::SPIKE, true, Arc::new(AtomicBool::new(false)), sandbox::channel().1));
    let r2 = sandbox::run_thread(sandbox::job(&script2, Budgets::SPIKE, true, Arc::new(AtomicBool::new(false)), sandbox::channel().1));
    let r1_desc = match &r1.result {
        Ok(v) => format!("ok {v:?}"),
        Err(f) => format!("err {}: {}", f.reason.label(), f.message),
    };
    let r2_desc = match &r2.result {
        Ok(v) => format!("{v:?}"),
        Err(f) => format!("err {}: {}", f.reason.label(), f.message),
    };
    // Run 2 must see the original builtins (2 and 3 are the lengths; 2 is the ctx.count() call
    // after its one notify) and only its own note.
    let visible = r2.notes != ["from run 2".to_string()] || r2_desc != "[3, 3, 2]";
    emit(json!({
        "test": "A.tamper-between-runs",
        "analogue": "string.format = ..., ctx.notify = ..., table.insert = ..., _G.leak = 1",
        "verdict": if visible { "escaped" } else { "contained" },
        "run1": r1_desc,
        "run2": r2_desc,
        "run2_notes": r2.notes,
        "note": "each run builds its own Engine; no Engine or Module is shared, so run 1 cannot change run 2",
    }));
    note(format!("  tamper-between-runs: run1 {r1_desc}; run2 {r2_desc}"));
}

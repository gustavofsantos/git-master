//! The measurement battery (SPEC.md, tests A to F) for Lua 5.4 via mlua.

use crate::sandbox::*;
use mlua::{Lua, LuaOptions, StdLib, Table};
use serde_json::{json, Value as J};
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

pub fn emit(test: &str, verdict: &str, summary: &str, extra: J) {
    let mut obj = json!({"test": test, "verdict": verdict, "summary": summary});
    if let J::Object(m) = extra {
        obj.as_object_mut().unwrap().extend(m);
    }
    println!("{obj}");
    eprintln!("{test:<34} {verdict:<10} {summary}");
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e6).round() / 1e3
}

fn rj(r: &RunReport) -> J {
    json!({
        "finish": r.finish_text(),
        "stop": r.stop_label(),
        "elapsed_ms": ms(r.elapsed),
        "hook_ticks": r.ticks,
        "ctx_calls": r.ctx_calls,
        "peak_mem_sampled_bytes": r.peak_mem,
        "notify": r.notify,
    })
}

/// Wraps a body (the text of `execute`) into the module format of the design.
pub fn module(body: &str) -> String {
    format!("local M = {{}}\nfunction M.execute(ctx, input)\n{body}\nend\nreturn M\n")
}

/// Runs a module on the current thread with its own fresh VM (for tests that always terminate).
fn run_here(src: &str, budgets: Budgets, profile: Profile, rx: Option<mpsc::Receiver<String>>) -> RunReport {
    let vm = new_vm(budgets, profile, Arc::new(AtomicBool::new(false)), rx).expect("vm setup");
    vm.run_module(src, None)
}

/// Returns (report, ended_only_after_cancel). A run that never ends gets a synthetic report with
/// `Finish::Failed("HUNG")` and `hung = true`; the spinning thread is left behind.
fn timed(src: &str, budgets: Budgets, profile: Profile) -> (RunReport, bool) {
    match exec_timed(src, budgets, profile, Duration::from_secs(30)) {
        Timed::Done(r) => (r, false),
        Timed::EndedAfterCancel(r) => (r, true),
        Timed::Hung => (
            RunReport {
                finish: Finish::Failed("HUNG: did not end in 30 s, ignored cancel".into()),
                stop: None,
                elapsed: Duration::from_secs(35),
                ticks: 0,
                ctx_calls: 0,
                notify: vec![],
                peak_mem: 0,
            },
            true,
        ),
    }
}

fn prod() -> Profile {
    Profile::production()
}

// ---------------------------------------------------------------- judges

fn only_nil_like(v: &str) -> bool {
    !v.is_empty() && v.split_whitespace().all(|w| matches!(w, "nil" | "0" | "locked"))
}

/// Budget stop => contained. Failure or a nil/absent answer => blocked. Anything else => escaped.
fn judge_default(r: &RunReport) -> &'static str {
    if r.stop.is_some() {
        return "contained";
    }
    match &r.finish {
        Finish::Returned(v) if only_nil_like(v) => "blocked",
        Finish::Failed(_) | Finish::Compile(_) => "blocked",
        _ => "escaped",
    }
}

fn judge_rawset_keep(r: &RunReport) -> &'static str {
    if r.stop.is_none() {
        if let Finish::Returned(v) = &r.finish {
            if v == "1" {
                return "contained"; // works, but only inside this run's own proxy
            }
        }
    }
    judge_default(r)
}

fn judge_pure_result(r: &RunReport) -> &'static str {
    if r.stop.is_some() {
        return "contained";
    }
    match &r.finish {
        Finish::Failed(_) => "n/a",
        Finish::Returned(_) => "n/a",
        _ => "n/a",
    }
}

fn judge_pcall_keep(r: &RunReport) -> &'static str {
    // A budget stop must end the run. Seeing "survived" in the output means it was caught.
    match (&r.finish, r.stop) {
        (Finish::Returned(v), Some(_)) if v.contains("survived") => "escaped",
        (_, Some(_)) => "contained",
        _ => "escaped",
    }
}

// ---------------------------------------------------------------- A

fn a_cases() -> Vec<(&'static str, &'static str, Profile, fn(&RunReport) -> &'static str, &'static str)> {
    let p = prod();
    let keep_pcall = Profile { keep_pcall: true, ..prod() };
    let keep_rawset = Profile { keep_rawset: true, ..prod() };
    let no_wrap = Profile { wrap_rep: false, ..prod() };
    vec![
        ("os.table", "return type(os)", p, judge_default, ""),
        ("os.execute", "return os.execute('true')", p, judge_default, ""),
        ("os.getenv", "return os.getenv('HOME')", p, judge_default, ""),
        ("os.time_clock", "return os.time() .. ' ' .. os.clock()", p, judge_default, ""),
        ("os.exit", "os.exit(0)", p, judge_default, ""),
        ("io.table", "return type(io)", p, judge_default, ""),
        ("io.open", "return io.open('/etc/hostname')", p, judge_default, ""),
        ("io.popen", "return io.popen('id')", p, judge_default, ""),
        ("io.read", "return io.read('l')", p, judge_default, ""),
        ("load.table", "return type(load)", p, judge_default, ""),
        ("load.call", "return load('return 40 + 2')()", p, judge_default, ""),
        ("loadstring", "return type(loadstring)", p, judge_default, ""),
        ("dofile_loadfile", "return type(dofile) .. ' ' .. type(loadfile)", p, judge_default, ""),
        ("dofile.call", "return dofile('/etc/hostname')", p, judge_default, ""),
        ("require_package", "return type(require) .. ' ' .. type(package)", p, judge_default, ""),
        ("require.call", "return require('os')", p, judge_default, ""),
        ("string.dump", "return type(string.dump)", p, judge_default, ""),
        ("string.dump.call", "return string.dump(function() return 1 end)", p, judge_default, ""),
        ("debug.table", "return type(debug)", p, judge_default, ""),
        ("debug.getinfo", "return debug.getinfo(1)", p, judge_default, ""),
        ("debug.sethook", "debug.sethook(function() end, '', 1)", p, judge_default, ""),
        ("debug.getregistry", "return debug.getregistry()", p, judge_default, ""),
        ("collectgarbage", "return type(collectgarbage)", p, judge_default, ""),
        ("print_warn", "return type(print) .. ' ' .. type(warn)", p, judge_default, ""),
        ("math.random", "return type(math.random) .. ' ' .. type(math.randomseed)", p, judge_default, ""),
        ("string_mt.getmetatable", "return tostring(getmetatable(''))", p, judge_default, ""),
        ("string_mt.index_write", "getmetatable('').__index.format = function() return 'x' end return 'ok'", p, judge_default, ""),
        ("string_lib.write", "string.format = function() return 'pwned' end return 'ok'", p, judge_default, ""),
        ("table_lib.write", "table.insert = nil return 'ok'", p, judge_default, ""),
        ("ctx.write", "ctx.notify = nil return 'ok'", p, judge_default, ""),
        ("ctx.getmetatable", "return tostring(getmetatable(ctx))", p, judge_default, ""),
        ("ctx.__index", "return tostring(ctx.__index)", p, judge_default, ""),
        ("ctx.iterate", "local n = 0 for _ in pairs(ctx) do n = n + 1 end return tostring(n)", p, judge_default, ""),
        ("ctx.rawset", "rawset(ctx, 'notify', nil) return 'ok'", p, judge_default, ""),
        ("ctx.unknown_name", "ctx.nosuch() return 'ok'", p, judge_default, ""),
        ("global.write", "_G.leak = 1 return tostring(leak)", p, judge_default, ""),
        ("global.rawset (production)", "rawset(_G, 'leak', 1) return tostring(leak)", p, judge_default, ""),
        ("global.rawset (keep_rawset)", "rawset(_G, 'leak', 1) return tostring(leak)", keep_rawset, judge_rawset_keep, "bypasses __newindex; only this run's proxy is changed"),
        ("global.setmetatable", "setmetatable(_G, {}) return 'ok'", p, judge_default, ""),
        ("string_lib.setmetatable", "setmetatable(string, {}) return 'ok'", p, judge_default, ""),
        ("global.iterate", "local n = 0 for _ in pairs(_G) do n = n + 1 end return tostring(n)", p, judge_default, ""),
        ("env.upvalue_assign", "_ENV = {} return 'ok'", p, judge_pure_result, "rebinds this chunk's _ENV only; the real env is unreachable"),
        ("env.local_shadow", "local _ENV = {} return tostring(os)", p, judge_default, ""),
        ("string.rep bomb (wrapped)", "return #string.rep('x', 1000000000)", p, judge_default, ""),
        ("string.rep bomb (no wrap)", "return #string.rep('x', 1000000000)", no_wrap, judge_default, "memory limit is the only stop"),
        ("table growth bomb", "local t = {} for i = 1, 1000000000 do t[i] = i end return #t", p, judge_default, ""),
        ("concat bomb", "local s = 'x' for i = 1, 60 do s = s .. s end return #s", p, judge_default, ""),
        ("infinite loop", "while true do end", p, judge_default, ""),
        ("deep recursion (1e6)", "local function f(n) return 1 + f(n + 1) end return f(1)", p, judge_default, ""),
        ("coroutine.wrap infinite loop", "local f = coroutine.wrap(function() while true do end end) f() return 'after'", p, judge_default, "root returns before its next hook tick; host checks stop flag"),
        ("coroutine.create infinite loop", "local co = coroutine.create(function() while true do end end) coroutine.resume(co) return 'after'", p, judge_default, ""),
        ("yield across table.sort", "coroutine.wrap(function() table.sort({3, 2, 1}, function(a, b) coroutine.yield() return a < b end) end)()", p, judge_default, ""),
        ("yield across ctx.try (Lua)", "local co = coroutine.wrap(function() return ctx.try(function() coroutine.yield('y') return 1 end) end) return tostring(co())", p, judge_pure_result, "documented behaviour, not a leak"),
        ("pcall absent (production)", "local ok = pcall(function() while true do end end) return 'survived'", p, judge_default, ""),
        ("pcall swallows budget (keep_pcall)", "local ok, e = pcall(function() while true do end end) return 'survived ' .. tostring(ok)", keep_pcall, judge_pcall_keep, ""),
        ("pcall loop (keep_pcall)", "local n = 0 while true do pcall(function() while true do end end) n = n + 1 end return tostring(n)", keep_pcall, judge_pcall_keep, ""),
        ("non-yieldable: sort spin", "table.sort({1, 2}, function() while true do end end) return 'x'", p, judge_default, ""),
        ("non-yieldable: sort + try loop", "table.sort({1, 2}, function() while true do ctx.try(function() while true do end end) end end) return 'x'", p, judge_default, "adversarial alignment case"),
        ("non-yieldable: __tostring spin", "local t = setmetatable({}, {__tostring = function() while true do end end}) return tostring(t)", p, judge_default, ""),
        ("close: normal exit spin", "do local x <close> = setmetatable({}, {__close = function() while true do end end}) end return 'x'", p, judge_default, ""),
        ("close: coroutine.close spin", "local co = coroutine.create(function() local x <close> = setmetatable({}, {__close = function() while true do end end}) coroutine.yield() end) coroutine.resume(co) coroutine.close(co) return 'x'", p, judge_default, "close runs non-yieldable"),
        ("close: error unwind spin", "local x <close> = setmetatable({}, {__close = function() while true do end end}) error('boom')", p, judge_default, ""),
        ("C-call __index spin (table.concat)", "return table.concat(setmetatable({}, {__index = function() while true do end end}), ',', 1, 2)", p, judge_default, ""),
        ("C-call gsub callback spin", "return (string.gsub('a', 'a', function() while true do end end))", p, judge_default, ""),
        ("gc finalizer spin", "for i = 1, 1000000 do setmetatable({}, {__gc = function() while true do end end}) end return 'done'", p, judge_default, ""),
        ("error object: table", "error({code = 7})", p, judge_pure_result, "host gets a string with an address"),
        ("error object: function", "error(function() return 1 end)", p, judge_pure_result, ""),
        ("ctx call budget", "for i = 1, 20000 do ctx.count() end return 'ok'", p, judge_default, ""),
    ]
}

pub fn section_a() {
    let cases = a_cases();
    let total = cases.len();
    for (name, body, profile, judge, note) in cases {
        let src = module(body);
        let t0 = Instant::now();
        let (rep, ended_after_cancel) = timed(&src, Budgets::start(), profile);
        let mut verdict = judge(&rep);
        if ended_after_cancel {
            verdict = "escaped";
        }
        if matches!(&rep.finish, Finish::Failed(m) if m.starts_with("HUNG")) {
            verdict = "escaped";
        }
        let summary = format!("{} | stop={}", rep.finish_text(), rep.stop_label());
        let mut extra = rj(&rep);
        extra["note"] = json!(note);
        extra["judge_wall_ms"] = json!(ms(t0.elapsed()));
        emit(&format!("A.{name}"), verdict, &summary, json!({"observed": extra}));
    }
    eprintln!("A: {total} escape cases run");

    // Tampering between runs: run 1 writes to builtins, run 2 (new VM) must see none of it.
    let tamper1 = r#"
local out = {}
local function attempt(label, f)
  local r = ctx.try(f)
  out[#out + 1] = label .. '=' .. r.outcome
end
attempt('string.format', function() string.format = function() return 'pwned' end end)
attempt('table.insert', function() table.insert = nil end)
attempt('ctx.notify', function() ctx.notify = function() end end)
attempt('_G.leak', function() _G.leak = 1 end)
attempt('getmetatable', function() getmetatable('').__index.format = nil end)
attempt('rawset(string,format)', function() rawset(string, 'format', function() return 'pwned' end) end)
attempt('rawset(_G,leak)', function() rawset(_G, 'leak', 1) end)
return table.concat(out, ' ; ')
"#;
    let prof1 = Profile { keep_rawset: true, ..prod() };
    let (r1, _) = timed(&module(tamper1), Budgets::start(), prof1);
    let tamper2 = "ctx.notify('run2 started')\nreturn string.format('%d', 7) .. ' ' .. tostring(_G.leak) .. ' ' .. type(table.insert)";
    let (r2, _) = timed(&module(tamper2), Budgets::start(), prod());
    let ok2 = matches!(&r2.finish, Finish::Returned(v) if v == "7 nil function");
    let summary = format!("run1: {} | run2: {}", r1.finish_text(), r2.finish_text());
    emit(
        "A.tamper_between_runs",
        if ok2 { "contained" } else { "escaped" },
        &summary,
        json!({"run1": rj(&r1), "run2": rj(&r2)}),
    );
}

// ---------------------------------------------------------------- B

fn well_behaved_body() -> &'static str {
    r#"
local acc = 0
for i = 1, 1000000 do acc = (acc + i * 7) % 1000003 end
local parts = {}
for i = 1, 20000 do parts[#parts + 1] = string.format('%d:%s', i, 'x') end
local s = table.concat(parts, ',')
return tostring(acc) .. ' ' .. tostring(#s)
"#
}

pub fn section_b() {
    // One budget at a time, the others left large.
    let base = Budgets::unlimited();
    let cases: Vec<(&str, Budgets, &str, Option<mpsc::Receiver<String>>, Profile)> = vec![
        ("B.instructions", Budgets { instructions: 1_000_000, ..base }, "while true do end", None, prod()),
        ("B.wall_clock", Budgets { wall: Duration::from_millis(200), ..base }, "while true do end", None, prod()),
        ("B.memory", Budgets { memory: 8 << 20, ..base }, "local t = {} for i = 1, 1000000000 do t[i] = i end return #t", None, prod()),
        ("B.string_size", Budgets { string_max: 1 << 20, ..base }, "return #string.rep('x', 2 ^ 21)", None, prod()),
        ("B.stack_depth", Budgets { depth: 1000, ..base }, "local st = { d = 0 } local function f() st.d = st.d + 1 return 1 + f() end f() return tostring(st.d)", None, prod()),
        ("B.ctx_calls", Budgets { ctx_calls: 100, ..base }, "for i = 1, 1000 do ctx.count() end return 'ok'", None, prod()),
    ];
    for (name, budgets, body, rx, profile) in cases {
        let t0 = Instant::now();
        let rep = run_here(&module(body), budgets, profile, rx);
        let wall = t0.elapsed();
        let summary = format!("{} | stop={} | {:.1} ms", rep.finish_text(), rep.stop_label(), ms(wall));
        let verdict = if rep.stop.is_some() { "contained" } else { "escaped" };
        emit(name, verdict, &summary, rj(&rep));
    }

    // Wall clock while blocked in ctx.block_recv (sender kept alive, nothing is ever sent).
    let (tx, rx) = mpsc::channel::<String>();
    let t0 = Instant::now();
    let rep = run_here(
        &module("ctx.block_recv() return 'got'"),
        Budgets { wall: Duration::from_millis(200), ..base },
        prod(),
        Some(rx),
    );
    let summary = format!("{} | stop={} | {:.1} ms", rep.finish_text(), rep.stop_label(), ms(t0.elapsed()));
    emit("B.wall_clock_blocked_recv", if rep.stop.is_some() { "contained" } else { "escaped" }, &summary, rj(&rep));
    drop(tx);

    // Native stack depth with no depth budget: where Lua itself stops (catchable, via ctx.try).
    let native = "local st = { d = 0 } local function f() st.d = st.d + 1 return 1 + f() end local r = ctx.try(f) return tostring(st.d) .. ' | ' .. r.message";
    let rep = run_here(&module(native), base, prod(), None);
    let summary = format!("{}", rep.finish_text());
    emit("B.stack_depth_native", "n/a", &summary, rj(&rep));

    // Budget overhead on a well-behaved script: interleaved runs, hook vs no hook.
    let body = well_behaved_body();
    let src = module(body);
    let configs: Vec<(&str, Profile)> = vec![
        ("no_hook", Profile { hooks: false, ..prod() }),
        ("hook_every_1000", Profile { hook_every: 1000, ..prod() }),
        ("hook_every_100", Profile { hook_every: 100, ..prod() }),
        ("hook_every_10000", Profile { hook_every: 10000, ..prod() }),
    ];
    let mut times: Vec<Vec<f64>> = vec![vec![]; configs.len()];
    let mut last: Vec<Option<RunReport>> = vec![None; configs.len()];
    for _ in 0..25 {
        for (i, (_, prof)) in configs.iter().enumerate() {
            let rep = run_here(&src, Budgets::start(), *prof, None);
            times[i].push(ms(rep.elapsed));
            last[i] = Some(rep);
        }
    }
    let med = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    let base_ms = med(&mut times[0].clone());
    // A ctx-bound workflow: 4000 ctx calls and light string work (few VM instructions).
    let ctx_src = module("local n = 0 for i = 1, 4000 do ctx.count() n = n + i end local parts = {} for i = 1, 2000 do parts[#parts + 1] = string.format('%d', i) end return tostring(n) .. ' ' .. tostring(#table.concat(parts, ','))");
    let mut ctimes: Vec<Vec<f64>> = vec![vec![]; 2];
    let ccfg = [Profile { hooks: false, ..prod() }, prod()];
    for _ in 0..25 {
        for i in 0..2 {
            let rep = run_here(&ctx_src, Budgets::start(), ccfg[i], None);
            ctimes[i].push(ms(rep.elapsed));
        }
    }
    let cbase = med(&mut ctimes[0].clone());
    let cm = med(&mut ctimes[1].clone());
    emit(
        "B.well_behaved_ctx_bound",
        "n/a",
        &format!("4000 ctx calls: no_hook median {cbase:.3} ms, hook median {cm:.3} ms, overhead {:+.1}%", (cm / cbase - 1.0) * 100.0),
        json!({"no_hook_median_ms": cbase, "hook_median_ms": cm, "overhead_pct": (cm / cbase - 1.0) * 100.0, "no_hook_runs_ms": ctimes[0], "hook_runs_ms": ctimes[1]}),
    );
    for (i, (name, _)) in configs.iter().enumerate() {
        let m = med(&mut times[i].clone());
        let rep = last[i].as_ref().unwrap();
        let over = (m / base_ms - 1.0) * 100.0;
        let min_ms = times[i].iter().cloned().fold(f64::MAX, f64::min);
        let summary = format!("median {:.2} ms (min {:.2}) over 25 runs | overhead {:+.1}% | {}", m, min_ms, over, rep.finish_text());
        emit(
            &format!("B.well_behaved.{name}"),
            "n/a",
            &summary,
            json!({"median_ms": m, "min_ms": min_ms, "runs_ms": times[i], "overhead_pct": over, "stop": rep.stop_label(), "result": rep.finish_text()}),
        );
    }
}

// ---------------------------------------------------------------- C

fn lcg(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *state >> 33
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

pub fn section_c() {
    let scenarios: Vec<(&str, &str, Profile, bool)> = vec![
        ("C.tight_loop", "while true do end", prod(), false),
        ("C.host_call_loop", "while true do ctx.count() end", prod(), false),
        ("C.block_recv", "ctx.block_recv() return 'x'", prod(), true),
        ("C.pcall_loop_raw", "while true do pcall(function() end) end", Profile { keep_pcall: true, ..prod() }, false),
        ("C.try_loop", "while true do ctx.try(function() end) end", prod(), false),
        ("C.deep_recursion_spin", "local function f(n) if n == 900 then while true do end end return 1 + f(n + 1) end return f(1)", prod(), false),
    ];
    let mut seed: u64 = 42;
    for (name, body, profile, needs_rx) in scenarios {
        let mut lat = Vec::new();
        let mut odd = 0;
        let mut odd_text = String::new();
        for _ in 0..50 {
            let cancel = Arc::new(AtomicBool::new(false));
            let (tx, rx) = mpsc::channel::<String>();
            let c2 = cancel.clone();
            let src = module(body);
            let budgets = Budgets { wall: Duration::from_secs(60), ctx_calls: u64::MAX, ..Budgets::unlimited() };
            let h = std::thread::spawn(move || {
                let vm = new_vm(budgets, profile, c2, if needs_rx { Some(rx) } else { None }).expect("vm");
                vm.run_module(&src, None)
            });
            let delay = 5 + lcg(&mut seed) % 46;
            std::thread::sleep(Duration::from_millis(delay));
            let t = Instant::now();
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            let rep = h.join().expect("join");
            let l = ms(t.elapsed());
            drop(tx);
            if rep.stop != Some(Reason::Cancelled) {
                odd += 1;
                odd_text = rep.finish_text();
            }
            lat.push(l);
        }
        lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = lat[0];
        let med = pct(&lat, 0.5);
        let p99 = pct(&lat, 0.99);
        let max = *lat.last().unwrap();
        let verdict = if odd == 0 { "contained" } else { "escaped" };
        let summary = format!("n=50 min {min:.3} med {med:.3} p99 {p99:.3} max {max:.3} ms | not-cancelled {odd}");
        emit(
            name,
            verdict,
            &summary,
            json!({"n": 50, "min_ms": min, "median_ms": med, "p99_ms": p99, "max_ms": max, "not_cancelled": odd, "last_odd": odd_text, "samples_ms": lat}),
        );
    }
}

// ---------------------------------------------------------------- D

const WORKFLOW: &str = r#"local M = {}
M.meta = { name = "bench", version = 1 }

local function classify(item)
  if type(item) ~= "table" then return "other" end
  if item.kind == "file" then return "file" end
  return "dir"
end

local function summarize(items)
  local counts = {}
  for _, item in ipairs(items) do
    local k = classify(item)
    counts[k] = (counts[k] or 0) + 1
  end
  return counts
end

local function build(n)
  local items = {}
  for i = 1, n do
    items[#items + 1] = { kind = (i % 3 == 0) and "dir" or "file", size = i * 10 }
  end
  return items
end

function M.execute(ctx, input)
  ctx.notify("start " .. tostring(input.name))
  local items = build(input.n)
  local counts = summarize(items)
  local ratio = ctx.try(function() return counts.file / counts.dir end)
  local now = ctx.now()
  local text = string.format("%d files, %d dirs", counts.file or 0, counts.dir or 0)
  ctx.notify("counted " .. text)
  if ratio.outcome == "error" then
    return "failed: " .. ratio.message
  end
  return text .. " at " .. tostring(now)
end

return M
"#;

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

fn fresh_run(input: &str) -> (Duration, Duration, bool) {
    let t0 = Instant::now();
    let vm = new_vm(Budgets::start(), prod(), Arc::new(AtomicBool::new(false)), None).expect("vm");
    let t_vm = t0.elapsed();
    let inp = vm.input(input).ok();
    let rep = vm.run_module(WORKFLOW, inp);
    let ok = matches!(rep.finish, Finish::Returned(ref v) if v.contains("files"));
    drop(vm);
    (t_vm, t0.elapsed(), ok)
}

fn pctl(v: &mut [f64]) -> (f64, f64, f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[0], pct(v, 0.5), pct(v, 0.99), v[v.len() - 1])
}

pub fn section_d() {
    let input = "{ name = 'x', n = 50 }";
    // Warm up once so the first allocation does not count.
    let _ = fresh_run(input);

    let mut vm_ms = Vec::new();
    let mut total_ms = Vec::new();
    let mut fails = 0;
    for _ in 0..1000 {
        let (a, b, ok) = fresh_run(input);
        vm_ms.push(ms(a));
        total_ms.push(ms(b));
        if !ok {
            fails += 1;
        }
    }
    let (mn, med, p99, mx) = pctl(&mut total_ms.clone());
    let (_, vmed, vp99, _) = pctl(&mut vm_ms.clone());
    emit(
        "D.fresh_vm_serial",
        if fails == 0 { "n/a" } else { "escaped" },
        &format!("1000 runs: total median {med:.3} ms p99 {p99:.3} ms (min {mn:.3}, max {mx:.3}); VM setup median {vmed:.3} ms p99 {vp99:.3} ms; failures {fails}"),
        json!({"runs": 1000, "total_median_ms": med, "total_p99_ms": p99, "total_min_ms": mn, "total_max_ms": mx,
               "setup_median_ms": vmed, "setup_p99_ms": vp99, "failures": fails}),
    );

    // Idle resident memory of one fresh VM: RSS delta over 50 live VMs.
    // 500 live VMs so the RSS delta is well above allocator reuse from the earlier sections.
    let before = rss_kb();
    let mut vms = Vec::new();
    for _ in 0..500 {
        vms.push(new_vm(Budgets::start(), prod(), Arc::new(AtomicBool::new(false)), None).expect("vm"));
    }
    let after = rss_kb();
    let per_kb = (after.saturating_sub(before)) as f64 / 500.0;
    let used: Vec<usize> = vms.iter().map(|v| v.lua.used_memory()).collect();
    let used_avg = used.iter().sum::<usize>() as f64 / used.len() as f64;
    drop(vms);
    emit(
        "D.idle_vm_rss",
        "n/a",
        &format!("RSS per idle fresh VM {per_kb:.1} KiB (500 live VMs, process RSS delta); Lua heap {used_avg:.0} bytes"),
        json!({"rss_per_vm_kib": per_kb, "lua_used_bytes_avg": used_avg, "live_vms": 500}),
    );

    // Eight threads, 1000 runs each.
    let t0 = Instant::now();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(move || {
                let mut totals = Vec::with_capacity(1000);
                let mut bad = 0;
                for _ in 0..1000 {
                    let (_, b, ok) = fresh_run("{ name = 'x', n = 50 }");
                    totals.push(ms(b));
                    if !ok {
                        bad += 1;
                    }
                }
                (totals, bad)
            })
        })
        .collect();
    let mut all = Vec::new();
    let mut bad_total = 0;
    for h in handles {
        let (t, b) = h.join().expect("join");
        all.extend(t);
        bad_total += b;
    }
    let wall = t0.elapsed();
    let hwm = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).map(|l| l.to_string()))
        .unwrap_or_default();
    let (mn, med, p99, mx) = pctl(&mut all);
    emit(
        "D.parallel_8x1000",
        if bad_total == 0 { "n/a" } else { "escaped" },
        &format!("8000 runs in {:.2} s wall: per-run median {med:.3} ms p99 {p99:.3} ms (min {mn:.3}, max {mx:.3}); failures {bad_total}; {hwm}", wall.as_secs_f64()),
        json!({"runs": 8000, "wall_s": wall.as_secs_f64(), "median_ms": med, "p99_ms": p99, "min_ms": mn, "max_ms": mx, "failures": bad_total, "vmhwm": hwm}),
    );
}

// ---------------------------------------------------------------- F

pub fn section_f() {
    // F1 module format, from the battery itself (A and D use it). Here: M.meta with no ctx at all.
    let meta_src = "local M = {}\nM.meta = { name = 'demo', version = 1, fields = { 'a', 'b' } }\nfunction M.execute(ctx, input) return ctx.count() end\nreturn M\n";
    let vm = new_vm(Budgets::start(), Profile { no_ctx: true, ..prod() }, Arc::new(AtomicBool::new(false)), None).expect("vm");
    let meta_res = vm
        .lua
        .load(meta_src)
        .set_name("workflow.lua")
        .set_environment(vm.env.clone())
        .call::<Table>(());
    let (verdict, summary) = match meta_res {
        Ok(m) => {
            let meta: Table = m.get("meta").expect("meta");
            let name: String = meta.get("name").unwrap_or_default();
            let version: i64 = meta.get("version").unwrap_or(-1);
            (
                "n/a",
                format!("sandbox without ctx: M.meta read, name={name} version={version}; no host function needed to load the module"),
            )
        }
        Err(e) => ("escaped", format!("error: {e}")),
    };
    emit("F.meta_without_ctx_sandbox", verdict, &summary, json!({"observed": summary.clone()}));

    // F2: M.meta in a plain Lua 5.4 state with an empty env table (no host at all).
    let lua = Lua::new();
    let empty = lua.create_table().unwrap();
    let r = lua.load(meta_src).set_name("workflow.lua").set_environment(empty).call::<Table>(());
    let s2 = match r {
        Ok(m) => {
            let meta: Table = m.get("meta").unwrap();
            format!("empty env: M.meta.name={}", meta.get::<String>("name").unwrap_or_default())
        }
        Err(e) => format!("error: {e}"),
    };
    emit("F.meta_empty_env", "n/a", &s2, json!({"observed": s2.clone()}));

    // F3 syntax errors: message, line and incomplete flag, with and without a chunk name.
    let bad_line3 = "local a = 1\nlocal b = 2\nlocal c = = 3\n";
    let bad_missing_end = "local M = {}\nfunction M.execute(ctx, input)\n  return 1\n";
    for (label, src, named) in [("line3", bad_line3, true), ("missing_end", bad_missing_end, true), ("line3_unnamed", bad_line3, false)] {
        let lua = Lua::new();
        let mut chunk = lua.load(src);
        if named {
            chunk = chunk.set_name("workflow.lua");
        }
        let r = chunk.into_function();
        let (text, inc) = match r {
            Err(mlua::Error::SyntaxError { message, incomplete_input }) => (message, incomplete_input),
            Err(e) => (format!("other: {e}"), false),
            Ok(_) => ("compiled?".into(), false),
        };
        emit(&format!("F.syntax_error.{label}"), "n/a", &format!("message: {text:?} incomplete_input={inc}"), json!({"message": text, "incomplete_input": inc}));
    }

    // F4 error values: what the host sees for table, function and __tostring error objects.
    let lua = Lua::new();
    let cases = [
        ("table", "error({code = 7})"),
        ("function", "error(function() return 1 end)"),
        ("tostring_meta", "error(setmetatable({}, {__tostring = function() return 'custom' end}))"),
        ("string_level", "error('plain', 0)"),
    ];
    for (label, src) in cases {
        let r = lua.load(src).exec();
        let (disp, dbg) = match r {
            Err(e) => (e.to_string().split("\nstack traceback:").next().unwrap_or("").to_string(), format!("{e:?}").chars().take(160).collect::<String>()),
            Ok(_) => ("ok".into(), String::new()),
        };
        emit(&format!("F.error_value.{label}"), "n/a", &format!("Display: {disp:?} | Debug: {dbg:?}"), json!({"display": disp, "debug": dbg}));
    }

    // F5 ctx enumeration: the proxy hides names, so the host must keep the list.
    let rep = run_here(&module("local n = 0 for _ in pairs(ctx) do n = n + 1 end return tostring(n)"), Budgets::start(), prod(), None);
    emit(
        "F.ctx_enumeration",
        "n/a",
        &format!("pairs(ctx) yields {} keys; host keeps the names [notify, now, count, block_recv, try] for 'valid names are' messages", rep.finish_text()),
        json!({"pairs_ctx": rep.finish_text(), "host_names": ["notify", "now", "count", "block_recv", "try"]}),
    );

    // F6 hook covers coroutines the script creates (global hook, production path).
    let (r, _) = timed(&module("local co = coroutine.create(function() while true do end end) coroutine.resume(co) return 'after'"), Budgets::start(), prod());
    emit("F.hook_covers_script_coroutine", if r.stop.is_some() { "contained" } else { "escaped" }, &format!("{} | stop={}", r.finish_text(), r.stop_label()), rj(&r));

    emit(
        "F.type_stubs",
        "n/a",
        "not tested: Lua 5.4 has no standard type-definition format; LuaLS '---@meta' stubs are the common convention, unverified here",
        json!({"tested": false}),
    );
}

/// Naive variant: the stop path raises an error and never yields. Shows why yield is needed.
pub fn section_f_naive() {
    let p = Profile { keep_pcall: true, naive_error: true, ..prod() };
    let single = module("local ok, e = pcall(function() while true do end end) return 'survived ' .. tostring(ok)");
    match exec_timed(&single, Budgets::start(), p, Duration::from_secs(3)) {
        Timed::Done(r) => emit(
            "F.naive_error.single_pcall",
            if matches!(&r.finish, Finish::Returned(v) if v.contains("survived")) && r.stop.is_some() { "escaped" } else { "contained" },
            &format!("{} | stop={} (the script caught the budget error and continued)", r.finish_text(), r.stop_label()),
            rj(&r),
        ),
        _ => emit("F.naive_error.single_pcall", "escaped", "did not finish in 3 s", json!({"hung": true})),
    }
    let looping = module("local n = 0 while true do pcall(function() while true do end end) n = n + 1 end return tostring(n)");
    match exec_timed(&looping, Budgets::start(), p, Duration::from_secs(3)) {
        Timed::Done(r) => emit("F.naive_error.pcall_loop", "contained", &format!("{} | stop={}", r.finish_text(), r.stop_label()), rj(&r)),
        Timed::EndedAfterCancel(r) => emit(
            "F.naive_error.pcall_loop",
            "escaped",
            &format!("did not finish in 3 s; only the host cancel ended it: {} | stop={}", r.finish_text(), r.stop_label()),
            rj(&r),
        ),
        Timed::Hung => emit(
            "F.naive_error.pcall_loop",
            "escaped",
            "did not finish in 3 s and ignored the cancel flag (every trip is caught by pcall)",
            json!({"hung": true}),
        ),
    }
}

/// Variant without the per-instruction switch: yield when possible, error otherwise, period N.
pub fn section_f_no_switch() {
    let p = Profile { no_switch: true, ..prod() };
    let cases: Vec<(&str, &str, Profile)> = vec![
        ("F.no_switch.sort_spin", "table.sort({1, 2}, function() while true do end end) return 'x'", p),
        ("F.no_switch.sort_try_loop", "table.sort({1, 2}, function() while true do ctx.try(function() while true do end end) end end) return 'x'", p),
        ("F.no_switch.pcall_loop_keep", "local n = 0 while true do pcall(function() while true do end end) n = n + 1 end return tostring(n)", Profile { keep_pcall: true, ..p }),
    ];
    for (name, body, profile) in cases {
        match exec_timed(&module(body), Budgets::start(), profile, Duration::from_secs(3)) {
            Timed::Done(r) => emit(name, if r.stop.is_some() { "contained" } else { "escaped" }, &format!("{} | stop={}", r.finish_text(), r.stop_label()), rj(&r)),
            Timed::EndedAfterCancel(r) => emit(name, "escaped", &format!("did not finish in 3 s; only the host cancel ended it: {} | stop={}", r.finish_text(), r.stop_label()), rj(&r)),
            Timed::Hung => emit(name, "escaped", "did not finish in 3 s and ignored the cancel flag", json!({"hung": true})),
        }
    }
}

/// Pitfall: a hook installed only on the run's thread does not cover coroutines the script creates.
/// Run last: if the escape reproduces, the inner loop spins until the process exits.
pub fn section_f_pitfall() {
    let p = Profile { per_thread_root: true, ..prod() };
    let src = module("local co = coroutine.create(function() while true do end end) coroutine.resume(co) return 'after'");
    match exec_timed(&src, Budgets::start(), p, Duration::from_secs(3)) {
        Timed::Done(r) => emit("F.per_thread_hook_pitfall", "contained", &format!("{} | stop={}", r.finish_text(), r.stop_label()), rj(&r)),
        Timed::EndedAfterCancel(r) => emit(
            "F.per_thread_hook_pitfall",
            "escaped",
            &format!("did not finish in 3 s; ended only after host cancel: {} | stop={}", r.finish_text(), r.stop_label()),
            rj(&r),
        ),
        Timed::Hung => emit(
            "F.per_thread_hook_pitfall",
            "escaped",
            "did not finish in 3 s and ignored the cancel flag: the coroutine spins unguarded (thread left running)",
            json!({"hung": true}),
        ),
    }
}

pub fn lua_stdlib_check() -> Vec<String> {
    let lua = Lua::new_with(StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8, LuaOptions::default()).unwrap();
    let names: Vec<String> = lua
        .globals()
        .pairs::<String, mlua::Value>()
        .filter_map(|p| p.ok().map(|(k, _)| k))
        .collect();
    let mut names = names;
    names.sort();
    names
}

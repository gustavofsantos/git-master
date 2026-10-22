//! Battery B: each budget hit by a script, a well-behaved script with and without the hook.

use crate::host::{self, Class, Limits, Opts, Outcome, Shared};
use crate::util::{emit, stats};

fn module(body: &str) -> String {
    format!("local M = {{}}\nfunction M.execute(ctx, input)\n{body}\nend\nreturn M\n")
}

fn exec(src: &str, limits: Limits, opts: Opts) -> Outcome {
    let shared = Shared::new(limits);
    host::run_module(src, &shared, opts)
}

fn report(test: &str, out: &Outcome, verdict: &str) {
    emit(serde_json::json!({
        "test": format!("B.{test}"),
        "verdict": verdict,
        "class": out.class.label(),
        "observed": out.returned.clone().or(out.error.clone()),
        "ms": out.elapsed.as_secs_f64() * 1000.0,
        "steps": out.steps,
        "ctx_calls": out.ctx_calls,
        "peak_memory_bytes": out.peak_memory,
        "notes": out.notes.len(),
    }));
}

fn budget_verdict(out: &Outcome) -> &'static str {
    match out.class {
        Class::Completed => "completed",
        _ => "contained",
    }
}

pub fn run() {
    let d = Limits::DEFAULT;
    let no_steps = Limits { steps: u64::MAX, ..d };
    // Memory cases run with a 30 s wall clock so that only the memory budget can end them
    // (64 MiB of small strings takes 1.3-1.9 s here, close to the 2 s default).
    let mem = Limits { wall: std::time::Duration::from_secs(30), ..d };

    // Instruction-approximation budget (interrupt points).
    let out = exec(&module("while true do end"), d, Opts::FULL);
    report("steps.infinite-loop", &out, budget_verdict(&out));

    // Wall clock: steps unlimited, so only the 2 s wall budget can stop the loop.
    let out = exec(&module("while true do end"), no_steps, Opts::FULL);
    report("wall.infinite-loop", &out, budget_verdict(&out));

    // Memory: small allocations until the allocator limit (64 MiB).
    let out = exec(
        &module("local t = {}\nwhile true do t[#t + 1] = string.rep('x', 1024) end"),
        mem,
        Opts::FULL,
    );
    report("memory.small-strings", &out, budget_verdict(&out));
    let out = exec(&module("local t = {}\nfor i = 1, 1000000000 do t[i] = i end"), mem, Opts::FULL);
    report("memory.table-growth", &out, budget_verdict(&out));

    // String size: one string.rep above the 16 MiB per-string cap.
    let out = exec(&module("local s = string.rep('x', 1000000000)\nreturn #s"), d, Opts::FULL);
    report("string.rep-1e9", &out, budget_verdict(&out));
    // Boundary: exactly the cap is allowed.
    let out = exec(&module("local s = string.rep('x', 16 * 1024 * 1024)\nreturn tostring(#s)"), d, Opts::FULL);
    report("string.rep-at-cap", &out, budget_verdict(&out));
    // Pure-Luau concatenation: no per-string guard possible, only the memory budget applies.
    let out = exec(&module("local s = 'x'\nfor i = 1, 40 do s = s .. s end\nreturn tostring(#s)"), mem, Opts::FULL);
    report("string.concat-doubling", &out, budget_verdict(&out));

    // Stack depth, Lua frames: count how deep a non-tail recursion gets before the VM stops it.
    let out = exec(
        &module("local d = 0\nlocal function f() d = d + 1 return 1 + f() end\nlocal ok, e = pcall(f)\nreturn tostring(d) .. ' depth, pcall ok=' .. tostring(ok) .. ' err=' .. tostring(e)"),
        d,
        Opts::FULL,
    );
    report("stack.lua-frames-caught", &out, "contained");
    let out = exec(&module("local function f(n) return 1 + f(n + 1) end\nreturn tostring(f(1))"), d, Opts::FULL);
    report("stack.lua-frames-uncaught", &out, budget_verdict(&out));
    // C-stack depth: recursion through ctx.try (a host call per level). ctx budget lifted for this one.
    let ctx_free = Limits { ctx_calls: u64::MAX, ..d };
    let out = exec(
        &module(
            "local d = 0\n\
             local function g()\n\
               d = d + 1\n\
               local r = ctx.try(g)\n\
               return r\n\
             end\n\
             local r = ctx.try(g)\n\
             return tostring(d) .. ' levels, outermost=' .. tostring(r.outcome) .. ' msg=' .. tostring(r.message)",
        ),
        ctx_free,
        Opts::FULL,
    );
    report("stack.c-calls-via-ctx.try", &out, "contained");

    // ctx calls: 1e6 notify calls, limit 1000.
    let out = exec(&module("for i = 1, 1000000 do ctx.notify('x') end\nreturn 'done'"), d, Opts::FULL);
    report("ctx-calls.notify-flood", &out, budget_verdict(&out));

    overhead();
}

/// Well-behaved script: does not trip any budget. Measures the hook overhead.
pub fn overhead() {
    let d = Limits::DEFAULT;
    let good = module(
        "local acc, parts = 0, {}\n\
         for i = 1, 1000000 do\n\
           acc = (acc * 31 + i) % 1000003\n\
           if i % 1000 == 0 then parts[#parts + 1] = string.format('%d', acc) end\n\
         end\n\
         local s = table.concat(parts, ',')\n\
         return tostring(acc) .. ':' .. tostring(#parts) .. ':' .. tostring(#s)",
    );
    let expected = {
        let out = exec(&good, d, Opts::FULL);
        out.returned.clone().unwrap_or_default()
    };
    // Alternate hook on / off to cancel drift; 9 reps each.
    let mut with_hook = Vec::new();
    let mut without_hook = Vec::new();
    let mut last_with = None;
    let mut last_without = None;
    for _ in 0..9 {
        let o = exec(&good, d, Opts::FULL);
        with_hook.push(o.elapsed.as_secs_f64() * 1000.0);
        last_with = Some(o);
        let o = exec(&good, d, Opts { hook: false, string_guard: true });
        without_hook.push(o.elapsed.as_secs_f64() * 1000.0);
        last_without = Some(o);
    }
    let w = last_with.unwrap();
    let wo = last_without.unwrap();
    let sw = stats(&with_hook);
    let swo = stats(&without_hook);
    let overhead = (sw.median - swo.median) / swo.median * 100.0;
    let ok = w.returned.as_deref() == Some(expected.as_str()) && w.class == Class::Completed;
    emit(serde_json::json!({
        "test": "B.well-behaved.loop-1e6",
        "verdict": if ok { "completed" } else { "FAILED" },
        "observed": w.returned.clone().or(w.error.clone()),
        "class_with_hook": w.class.label(),
        "class_without_hook": wo.class.label(),
        "steps_used": w.steps,
        "ms_with_hook": sw.json(),
        "ms_without_hook": swo.json(),
        "hook_overhead_pct_median": overhead,
        "peak_memory_bytes": w.peak_memory,
    }));
}

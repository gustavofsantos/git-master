//! Battery D: cost of a fresh VM per run, idle resident size, and 8 parallel threads.

use std::thread;
use std::time::Instant;

use crate::host::{self, Limits, Opts, Shared};
use crate::util::{emit, rss_kib, stats};

/// A ~40-line workflow module in the design's format.
pub const MODULE: &str = r#"local M = {}
M.meta = { name = "demo", version = 1 }

local function summarize(items)
  local total = 0
  for _, item in ipairs(items) do
    total = total + item.size
  end
  return total
end

local function label(n)
  if n % 15 == 0 then return "fizzbuzz" end
  if n % 3 == 0 then return "fizz" end
  if n % 5 == 0 then return "buzz" end
  return tostring(n)
end

function M.execute(ctx, input)
  local items = {}
  for i = 1, input.count * 7 do
    items[#items + 1] = { name = "file" .. i, size = i * 3 }
  end
  local total = summarize(items)
  local labels = {}
  for i = 1, 30 do
    labels[#labels + 1] = label(i)
  end
  local outcome = ctx.try(function()
    if total > 100000 then
      error("too big")
    end
    return total
  end)
  if outcome.outcome == "error" then
    ctx.notify("error: " .. outcome.message)
  else
    ctx.notify("total " .. tostring(outcome.value))
  end
  local now = ctx.now()
  return table.concat(labels, ",") .. "|" .. tostring(outcome.outcome) .. "|" .. tostring(total)
end

return M
"#;

/// One full run: fresh VM, sandbox, ctx, load the module, call execute, drop everything.
/// Returns (ms, returned string, class label).
fn one_run() -> (f64, String, String) {
    let shared = Shared::new(Limits::DEFAULT);
    let out = host::run_module(MODULE, &shared, Opts::FULL);
    (
        out.elapsed.as_secs_f64() * 1000.0,
        out.returned.clone().or(out.error.clone()).unwrap_or_default(),
        out.class.label(),
    )
}

pub fn run() {
    let (_, expected, _) = one_run();
    let (_, _, _) = one_run();

    // Sequential: 1000 runs.
    let mut ms = Vec::with_capacity(1000);
    let mut mismatches = 0usize;
    for _ in 0..1000 {
        let (t, out, class) = one_run();
        if out != expected || class != "completed" {
            mismatches += 1;
        }
        ms.push(t);
    }
    let s = stats(&ms);
    emit(serde_json::json!({
        "test": "D.fresh-vm.sequential-1000",
        "verdict": if mismatches == 0 { "completed" } else { "FAILED" },
        "ms_per_run": s.json(),
        "mismatches": mismatches,
        "expected": expected,
    }));

    // Idle resident memory of a fresh VM (sandboxed, ctx installed, module not loaded).
    drop(build_held(1));
    let base = rss_kib();
    let held = build_held(200);
    let after = rss_kib();
    let used: usize = held.iter().map(|v| v.lua.used_memory()).sum::<usize>() / held.len();
    emit(serde_json::json!({
        "test": "D.fresh-vm.idle-resident",
        "verdict": "n/a",
        "vms_held": held.len(),
        "rss_kib_per_vm": (after as f64 - base as f64) / held.len() as f64,
        "lua_used_memory_bytes_per_vm": used,
        "note": "RSS delta over 200 live VMs after one warm-up VM; lua_used_memory is the allocator-counted bytes",
    }));
    drop(held);

    // Parallel: 8 threads x 1000 runs.
    let start = Instant::now();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let expected = expected.clone();
            thread::spawn(move || {
                let mut ms = Vec::with_capacity(1000);
                let mut bad = 0usize;
                for _ in 0..1000 {
                    let (t, out, class) = one_run();
                    if out != expected || class != "completed" {
                        bad += 1;
                    }
                    ms.push(t);
                }
                (ms, bad)
            })
        })
        .collect();
    let mut all = Vec::new();
    let mut bad_total = 0usize;
    for h in handles {
        let (ms, bad) = h.join().expect("thread");
        bad_total += bad;
        all.extend(ms);
    }
    let wall = start.elapsed().as_secs_f64() * 1000.0;
    let s = stats(&all);
    emit(serde_json::json!({
        "test": "D.fresh-vm.parallel-8x1000",
        "verdict": if bad_total == 0 { "completed" } else { "FAILED" },
        "total_wall_ms": wall,
        "runs_per_second": 8000.0 / (wall / 1000.0),
        "ms_per_run": s.json(),
        "bad_runs": bad_total,
        "cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
    }));
}

fn build_held(n: usize) -> Vec<host::Vm> {
    (0..n)
        .map(|_| {
            let shared = Shared::new(Limits::DEFAULT);
            host::build_vm(&shared, Opts::FULL).expect("build vm")
        })
        .collect()
}

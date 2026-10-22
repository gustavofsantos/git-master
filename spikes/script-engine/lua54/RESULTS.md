# S-D spike: Lua 5.4 via mlua (RESULTS)

Crate: `spikes/script-engine/lua54` (`spike-script-lua54`). Raw output: `results/lua54.jsonl` (run 1; runs 2 and 3 in `results/lua54-run{2,3}.jsonl`). Human summaries: `results/lua54-summary.txt` and `results/lua54-run{1,2,3}-summary.txt`. Every number below comes from those runs. The three full runs gave identical verdicts. Timing spreads are reported as ranges across runs.

## Pinned versions

From `cargo tree -p spike-script-lua54 --depth 1`:

- `mlua v0.11.6` (features `lua54`, `vendored`, `send`), `mlua-sys v0.10.0`, `lua-src v550.0.0` (Lua **5.4.8**, vendored C)
- `serde_json v1.0.154` (used only for JSON output)
- Toolchain: cargo 1.99.1, gcc/cc 13.3.0 (Ubuntu). Machine: 4 cores, 16 GB RAM, shared with other workers that were compiling during the runs (load average 2 to 3), which explains the timing spread.

## What was built

- `src/sandbox.rs`: the VM, the allow-list env, the host object `ctx`, the budgets, the hook and cancellation. `PRELUDE` is trusted Lua run once per VM; it builds the read-only proxies and returns `runner, env`.
- `src/battery.rs`: tests A to F. `src/main.rs`: section selection (`A B C D F N V P`).
- Module format: `local M = {} ... function M.execute(ctx, input) ... end return M`. The runner calls `execute` inside a Lua coroutine so the hook can yield the run out. `input` is a deep read-only proxy.
- `ctx` is a read-only proxy over Rust closures: `notify`, `now` (fake clock: 1700000000 + ctx calls), `count`, `block_recv`, and `try`. `try` is written in Lua (a `pcall` inside the prelude), so a yield passes through it. A Rust `try` would be a non-yieldable C boundary.
- Starting budgets (`Budgets::start()`): instructions 50,000,000 (checked every 1000 VM instructions via a count hook); wall clock 5 s from VM creation; memory 32 MiB (mlua allocator limit, checked on hook ticks too); string size 4 MiB per `string.rep`; stack depth 1000 Lua frames (checked on hook ticks); ctx calls 10,000.
- Removed from the script's world: `os`, `io`, `load`, `loadfile`, `dofile`, `require`/`package`, `debug`, `collectgarbage`, `print`, `warn`, `string.dump`, `math.random`, `math.randomseed`, `rawset`, `pcall`/`xpcall` (the production profile; see the variant notes). `setmetatable` rejects a `__gc` field. `string.rep` is replaced by a host function that enforces the string budget.
- Remaining base surface (checked with `--globals`): `assert error ipairs pairs next select tonumber tostring type rawequal rawlen rawget getmetatable setmetatable _VERSION _G`, the `coroutine`, `table`, `string`, `math`, `utf8` libraries (read-only proxies), and `ctx`.

## Verdicts against the must-haves

- **Escape attempts (A):** 69 rows (68 cases plus the tamper test): 45 blocked, 20 contained (budget stops, the keep_rawset write that stays inside its run, and the tamper test), 4 n/a (documented behaviour or error-value cases), 0 escaped in production; no escape. The four "escaped" verdicts in the whole file are deliberate broken variants (see Variants), not the production sandbox.
- **Budgets (B):** all six hit, with distinct reasons; the well-behaved script never trips. Instruction-hook overhead on loop-heavy code is large (+70% to +80% median across runs), see "Surprises".
- **Cancellation (C):** all five scenarios end promptly. Median 0.1 to 0.9 ms; the worst tail is 48 ms, under 8-thread contention (section C).
- **Fresh VM (D):** median 0.37 to 0.47 ms per run (VM setup 0.21 to 0.27 ms of it); 0 failures in 1000 serial and 8000 parallel runs; about 37 KiB RSS per idle VM.
- **Binary (E):** 1.2 MB stripped (thin LTO); clean build 20 s wall with 4 cores; C toolchain needed (gcc/cc).
- **Blocking issue:** none that prevents Lua 5.4 from meeting the must-haves, but three conditions were needed to get there: the `__gc` guard, the every-instruction switch after a trip, and `ctx.try` in Lua. Each is detailed below.

## A. Escape attempts (production profile, run 1; runs 2 and 3 give the same verdicts)

Each script is wrapped as the body of `execute`. "stop" is the budget reason the host recorded (`none` if the run ended without one).

| Test | Verdict | Observed (returned value or error text; stop reason) |
|---|---|---|
| `os.table` | blocked | returned "nil" \| stop=none |
| `os.execute` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to index a nil value (global 'os') \| stop=none |
| `os.getenv` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to index a nil value (global 'os') \| stop=none |
| `os.time_clock` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to index a nil value (global 'os') \| stop=none |
| `os.exit` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to index a nil value (global 'os') \| stop=none |
| `io.table` | blocked | returned "nil" \| stop=none |
| `io.open` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to index a nil value (global 'io') \| stop=none |
| `io.popen` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to index a nil value (global 'io') \| stop=none |
| `io.read` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to index a nil value (global 'io') \| stop=none |
| `load.table` | blocked | returned "nil" \| stop=none |
| `load.call` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to call a nil value (global 'load') \| stop=none |
| `loadstring` | blocked | returned "nil" \| stop=none |
| `dofile_loadfile` | blocked | returned "nil nil" \| stop=none |
| `dofile.call` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to call a nil value (global 'dofile') \| stop=none |
| `require_package` | blocked | returned "nil nil" \| stop=none |
| `require.call` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to call a nil value (global 'require') \| stop=none |
| `string.dump` | blocked | returned "nil" \| stop=none |
| `string.dump.call` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to call a nil value (field 'dump') \| stop=none |
| `debug.table` | blocked | returned "nil" \| stop=none |
| `debug.getinfo` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to index a nil value (global 'debug') \| stop=none |
| `debug.sethook` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to index a nil value (global 'debug') \| stop=none |
| `debug.getregistry` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to index a nil value (global 'debug') \| stop=none |
| `collectgarbage` | blocked | returned "nil" \| stop=none |
| `print_warn` | blocked | returned "nil nil" \| stop=none |
| `math.random` | blocked | returned "nil nil" \| stop=none |
| `string_mt.getmetatable` | blocked | returned "locked" \| stop=none |
| `string_mt.index_write` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to index a nil value (field '__index') \| stop=none |
| `string_lib.write` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to modify read-only string (key 'format') \| stop=none |
| `table_lib.write` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to modify read-only table (key 'insert') \| stop=none |
| `ctx.write` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to modify read-only ctx (key 'notify') \| stop=none |
| `ctx.getmetatable` | blocked | returned "locked" \| stop=none |
| `ctx.__index` | blocked | returned "nil" \| stop=none |
| `ctx.iterate` | blocked | returned "0" \| stop=none |
| `ctx.rawset` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to call a nil value (global 'rawset') \| stop=none |
| `ctx.unknown_name` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to call a nil value (field 'nosuch') \| stop=none |
| `global.write` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to modify read-only global environment (key 'leak') \| stop=none |
| `global.rawset (production)` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to call a nil value (global 'rawset') \| stop=none |
| `global.rawset (keep_rawset)` | contained | returned "1" \| stop=none *(bypasses __newindex; only this run's proxy is changed)* |
| `global.setmetatable` | blocked | error: runtime error: prelude:67: cannot change a protected metatable \| stop=none |
| `string_lib.setmetatable` | blocked | error: runtime error: prelude:67: cannot change a protected metatable \| stop=none |
| `global.iterate` | blocked | returned "0" \| stop=none |
| `env.upvalue_assign` | n/a | returned "ok" \| stop=none *(rebinds this chunk's _ENV only; the real env is unreachable)* |
| `env.local_shadow` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to call a nil value (global 'tostring') \| stop=none |
| `string.rep bomb (wrapped)` | contained | error: runtime error: budget exceeded: string_size (1000000000 bytes requested, limit 4194304) \| stop=string_size |
| `string.rep bomb (no wrap)` | contained | error: not enough memory \| stop=memory *(memory limit is the only stop)* |
| `table growth bomb` | contained | error: not enough memory \| stop=memory |
| `concat bomb` | contained | error: not enough memory \| stop=memory |
| `infinite loop` | contained | yielded (stopped) \| stop=instructions |
| `deep recursion (1e6)` | contained | yielded (stopped) \| stop=stack_depth |
| `coroutine.wrap infinite loop` | contained | returned "after" \| stop=instructions *(root returns before its next hook tick; host checks stop flag)* |
| `coroutine.create infinite loop` | contained | returned "after" \| stop=instructions |
| `yield across table.sort` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to yield across a C-call boundary \| stop=none |
| `yield across ctx.try (Lua)` | n/a | returned "y" \| stop=none *(documented behaviour, not a leak)* |
| `pcall absent (production)` | blocked | error: runtime error: [string "workflow.lua"]:3: attempt to call a nil value (global 'pcall') \| stop=none |
| `pcall swallows budget (keep_pcall)` | contained | yielded (stopped) \| stop=instructions |
| `pcall loop (keep_pcall)` | contained | yielded (stopped) \| stop=instructions |
| `non-yieldable: sort spin` | contained | error: runtime error: budget exceeded: instructions \| stop=instructions |
| `non-yieldable: sort + try loop` | contained | error: runtime error: budget exceeded: instructions \| stop=instructions *(adversarial alignment case)* |
| `non-yieldable: __tostring spin` | contained | error: runtime error: budget exceeded: instructions \| stop=instructions |
| `close: normal exit spin` | contained | yielded (stopped) \| stop=instructions |
| `close: coroutine.close spin` | contained | returned "x" \| stop=instructions *(close runs non-yieldable)* |
| `close: error unwind spin` | blocked | error: runtime error: [string "workflow.lua"]:3: boom \| stop=none |
| `C-call __index spin (table.concat)` | contained | error: runtime error: budget exceeded: instructions \| stop=instructions |
| `C-call gsub callback spin` | contained | error: runtime error: budget exceeded: instructions \| stop=instructions |
| `gc finalizer spin` | blocked | error: runtime error: [string "workflow.lua"]:3: __gc is not supported in workflow scripts \| stop=none |
| `error object: table` | n/a | error: runtime error: table: 0x7f30c000d420 \| stop=none *(host gets a string with an address)* |
| `error object: function` | n/a | error: runtime error: function: 0x7f30c000bef0 \| stop=none |
| `ctx call budget` | contained | error: runtime error: budget exceeded: ctx_calls (limit 10000) \| stop=ctx_calls |

Tampering between runs (SPEC A, last item). Run 1 (with `rawset` available, so the rawset-based writes succeed inside that run's own proxy) attempts every builtin write. Run 2 is a new VM:

- run 1: `string.format=error ; table.insert=error ; ctx.notify=error ; _G.leak=error ; getmetatable=error ; rawset(string,format)=done ; rawset(_G,leak)=done`
- run 2: `returned "7 nil function"`: `string.format` works, `_G.leak` is nil, `table.insert` is intact. Verdict `contained`.

Notes on the table:

- Every absence is a `nil`, and every write to a library, to `ctx`, or to `_G` raises `attempt to modify read-only <name>`. `getmetatable("")` returns the string `"locked"` (the string metatable's `__metatable` field). `setmetatable` on `_G`, `string` or any proxy returns `cannot change a protected metatable`.
- `rawset` bypasses `__newindex`. It is not in production. With it present (`keep_rawset`), it changes only the proxy of the current run, which is contained, not blocked.
- The string bomb is stopped by our `string.rep` wrapper (`string_size`). Without the wrapper the Lua allocator refuses at the memory limit (`not enough memory`, reason `memory`). Concatenation (`..`) and `table.concat` are not per-string bounded: they are stopped by the memory limit only.
- Error objects: `error({code=7})` reaches the host as the string `table: 0x...` (mlua stringifies it, with an address). Inside `ctx.try` a non-string error becomes `non-string error (table)`. A host that needs structured errors must use strings.
- `coroutine.wrap` of an infinite loop is stopped by the stop flag, not by the thread status: the inner coroutine yields, `f()` returns, and the root may finish before its next hook tick. The run is still reported as `instructions`. The host must check the stop flag, not only the finish state.
- `A.close: error unwind spin` is `blocked` only in the sense that the `__close` never ran: a coroutine that errors does not run its pending `<close>` handlers (Lua 5.4 runs them on `coroutine.close`, which does run, and is stopped, see `A.close: coroutine.close spin`).

## B. Budgets (one budget at a time; run 1; spread over the three runs in brackets)

| Budget | Script | Reason | Elapsed (ms) | Notes |
|---|---|---|---|---|
| instructions (1e6) | `while true do end` | `instructions` | 4.2 [4.2, 9.0] | yields; ~1e6 VM instructions |
| wall clock (200 ms) | `while true do end` | `wall_clock` | 199.9 [199.6, 199.9] | |
| memory (8 MiB) | table growth to 1e9 | `memory` (`not enough memory`) | 8.0 [7.2, 15.2] | sampled peak 4.2 MB (hook samples only) |
| string size (1 MiB) | `string.rep('x', 2^21)` | `string_size` | 0.06 [0.06, 0.15] | stopped before allocation |
| stack depth (1000) | recursion | `stack_depth` | 0.15 [0.15, 0.41] | |
| ctx calls (100) | 1000 `ctx.count()` | `ctx_calls` | 0.03 [0.03, 0.07] | |
| wall clock inside `ctx.block_recv` (200 ms) | blocks on an empty channel | `wall_clock` | 200.0 [200.4, 200.6] | woken by the 1 ms poll |

Other B results:

- **Native stack depth** with no budget: Lua 5.4 stops at **999,988 Lua frames** with a catchable `stack overflow` (LUAI_MAXSTACK = 1,000,000 slots). The 1000-frame budget is therefore two orders of magnitude below the native limit and is the one that matters.
- **Well-behaved script** (1e6-iteration arithmetic loop, 20k `string.format`, `table.concat`; ~5.2 M VM instructions; returns `21 148893`, identical with and without the hook). Median over 25 interleaved runs per configuration:

| Configuration | Median (ms) | Overhead vs no hook |
|---|---|---|
| no hook | 18.3 to 25.7 | 0 |
| hook, every 1000 instructions (production) | 31.4 to 45.9 | +72% to +79% |
| hook, every 100 | 34.2 to 58.4 | +87% to +127% |
| hook, every 10000 | 31.6 to 46.6 | +72% to +90% |
| ctx-bound: 4000 `ctx.count()` calls, no hook | 0.76 to 0.92 | |
| ctx-bound: same, with hook | 0.91 to 1.12 | +17% to +21% |

The spread is large because the machine was loaded. The overhead does not depend on the period N (the same for 100, 1000 and 10000). See "Surprises" for why.

## C. Cancellation latency (50 trials each; cancel at a random point 5 to 50 ms after start)

Latency = time from setting the flag to the run thread returning (join). All 50 trials per scenario ended with `stop=cancelled` in all three runs.

| Scenario | min (ms) | median (ms) | p99 (ms) | max (ms) |
|---|---|---|---|---|
| tight pure loop `while true do end` | 0.09 to 0.13 | 0.14 to 0.18 | 0.32 to 0.96 | 0.32 to 0.96 |
| cheap host call loop `while true do ctx.count() end` | 0.06 to 0.09 | 0.15 to 0.15 | 0.27 to 48.1 | 0.27 to 48.1 |
| blocked in `ctx.block_recv()` | 0.17 to 0.20 | 0.70 to 0.85 | 1.44 to 3.11 | 1.44 to 3.11 |
| `pcall` in a loop (raw pcall, keep_pcall profile) | - | 0.15 to 0.17 | 0.32 to 3.98 | |
| `ctx.try` in a loop (production) | - | 0.14 to 0.19 | 0.25 to 1.79 | |
| deep recursion (spin at depth 900) | 0.06 to 0.11 | 0.17 to 0.23 | 0.31 to 0.90 | 0.31 to 0.90 |

With n = 50, p99 equals the max. The 48 ms outlier was in one run and is a scheduling stall under load; the medians are stable.

## D. Cost of a fresh VM (1000 serial runs each; module ~40 lines)

| Measure | Result (range over three runs) |
|---|---|
| Total per run (create VM, prelude, hook, compile, `execute`, drop): median | 0.37 to 0.47 ms |
| Total per run: p99 | 0.65 to 1.02 ms |
| VM setup alone: median / p99 | 0.21 to 0.27 ms / 0.31 to 0.64 ms |
| Idle fresh VM, RSS delta (500 live VMs) | 36.8 KiB (Lua heap 33,852 bytes) |
| 8 threads x 1000 runs, per-run median / p99 | 0.28 to 0.36 ms / 8.6 to 12.5 ms |
| 8 x 1000 total wall | 1.07 to 1.11 s |
| Failures (any run not returning the expected value) | 0 in every configuration |

Under 8 threads on 4 cores the tail grows (p99 about 9 to 12 ms, max 21 to 221 ms in one run) because the 8 threads plus other workers compete for 4 cores. There were no failures. Peak RSS for the parallel run was about 113 MB (glibc per-thread arenas), which is allocator behaviour, not VM size.

## E. Binary and build

- Stripped release binary (`CARGO_PROFILE_RELEASE_STRIP=true CARGO_PROFILE_RELEASE_LTO=thin`, default opt-level): **1,232,344 bytes** (1.2 MB). Unstripped, default profile: 1,560,264 bytes.
- Clean build: a fresh target dir in the scratchpad, so the shared cache was not touched: `cargo build --release --offline -p spike-script-lua54`: 20.0 s wall, 37 s CPU (4 cores). The C of Lua 5.4.8 is compiled by `lua-src` through `mlua-sys`'s build script with the system `cc` (gcc 13.3.0).
- Dependency graph: 37 lines in `cargo tree -p spike-script-lua54`, 27 third-party packages (including build-only ones): `mlua`, `mlua-sys`, `lua-src`, `luajit-src`, `serde_json`, `parking_lot`, `bstr`, `memchr`, `libc`, `cc`, `pkg-config`, `which`, `rustc-hash`, `smallvec`, `either`, `num-traits`, `itoa`, `zmij`, `serde_core` and others.
- Licences (from `cargo tree -e normal,build --prefix none --format "{p} | {l}"`): MIT only for `mlua`, `mlua-sys`, `lua-src`, `luajit-src` and `which`, `zmij`; `MIT OR Apache-2.0` for most others; `Unlicense OR MIT` (memchr); `Apache-2.0 OR MIT` (rustc-hash). Nothing outside MIT, Apache-2.0, BSD, ISC, Unlicense or Zlib.
- `luajit-src` is pulled in by mlua-sys's `vendored` feature as a build dependency. It is **not compiled** for Lua 5.4 (checked in the clean build's build directory: only `mlua-sys` runs its build script). It is still in the dependency graph and the source tree.

## F. API facts (evidence in brackets; RESULTS run 1)

1. **Module format:** runs. `M.execute(ctx, input)` is called by a runner in a coroutine. [A, D]
2. **`M.meta` with no `ctx`:** readable. The module's top level never needs a host function. Shown with the `no_ctx` profile (`name=demo version=1`) and with a plain `Lua::new()` plus an empty env table (`name=demo`). [F.meta_without_ctx_sandbox, F.meta_empty_env]
3. **Syntax errors:** `mlua::Error::SyntaxError { message, incomplete_input }`. The message carries the line: `[string "workflow.lua"]:3: unexpected symbol near '='` with `incomplete_input=false`. A missing `end` gives `...:4: 'end' expected (to close 'function' at line 2) near <eof>` with `incomplete_input=true` (useful for a REPL, not for `check`). **Without `set_name`, mlua uses the Rust source location as the chunk name** (`script-engine/lua54/src/battery.rs:604:3:`). Always set the name. Not tested: the `=` and `@` name prefixes (Lua's documented way to show a plain or file name); the format above is for a name without a prefix. [F.syntax_error.*]
4. **Error values:** `error({code=7})`, `error(function() ... end)` and an error object with `__tostring` all reach the host as `Error::RuntimeError(String)` with a traceback appended. Table and function values become strings with an address (`table: 0x...`); `__tostring` is called (`custom`). Structured errors do not survive the boundary. [F.error_value.*, A.error object]
5. **`ctx` enumeration:** `pairs(ctx)` yields nothing (proxy). The host keeps the names (`notify, now, count, block_recv, try`) for "valid names are" messages. [F.ctx_enumeration]
6. **Hook coverage of script-created coroutines:** covered by `set_global_hook` (mlua copies the global hook into each `lua_newthread`). **Not covered** by a hook set on the run's thread only (`Thread::set_hook`): the coroutine's hook is removed on its first tick and it spins unguarded. This reproduced (see the pitfall). [F.hook_covers_script_coroutine, F.per_thread_hook_pitfall]
7. **Type stubs for editors:** **not tested.** Lua 5.4 has no standard format. LuaLS `---@meta` files are the usual convention; unverified here.
8. **Library surface:** the base library always comes with `new_with` (`print`, `warn`, `load`, `dofile`, `collectgarbage`, `rawset` and the rest). Only the libraries requested are loaded; `os`, `io`, `package`, `debug` are absent. The removals in the production profile are enforced in the prelude, so a future mlua that adds globals needs a re-check (`--globals` lists them).

## Variants measured (why each guard exists)

| Variant | Verdict | Observed |
|---|---|---|
| **Naive:** hook raises an error after a trip, period N, no yield, no switch (`F.naive_error.single_pcall`) | escaped | `returned "survived false"`: the script catches the budget error and carries on |
| Naive, pcall loop (`F.naive_error.pcall_loop`) | escaped | did not finish in 3 s; the host cancel flag is ignored, every trip is caught by the loop's `pcall` |
| **Yield when possible, error otherwise, period N, no switch** (`F.no_switch.sort_try_loop`, nested in a `table.sort` comparator) | escaped | did not finish in 3 s; it spins because each trip lands inside the nested `try` |
| Same variant, simple sort comparator spin (`F.no_switch.sort_spin`) | contained | ends with `budget exceeded` |
| Same variant, raw `pcall` loop (`F.no_switch.pcall_loop_keep`) | contained | yields through `pcall` |
| **Per-thread hook** on the run's thread only, script creates a coroutine (`F.per_thread_hook_pitfall`) | escaped | did not finish in 3 s; ignored the cancel flag; the coroutine is unguarded |
| **Production** (global hook + every-instruction switch + yield) | contained | every adversarial case above ends with a budget reason |
| `__gc` finalizer spin, before the guard (observed in this session, the battery hung for 30 s and the cancel did not end it) | was an escape | Lua 5.4 sets `allowhook = 0` inside finalizers (`lgc.c`), so no hook fires there; fixed by rejecting `__gc` in `setmetatable`; now `blocked` |

The guard that matters is the **switch to a per-instruction hook after a trip** (`stop_mode` in `sandbox.rs`). After a trip, every thread that runs raises the stop on the next VM instruction, so the instruction after a `pcall` raises again, outside the `pcall`. Yielding is the clean exit when the thread is yieldable (no further Lua code runs; `pcall` cannot catch it). Yield alone is not enough: the no-switch variant hangs on the nested `try` case.

## Surprises and findings

1. **The instruction hook costs 70 to 90% on loop-heavy code, and does not depend on N.** Lua 5.4 keeps `trap` set while any hook mask is active, so `luaG_traceexec` runs before every VM instruction (`lvm.c`, `vmfetch`; `ldebug.c`, `luaG_traceexec`). The period only changes how often the hook body runs. ctx-bound code paid only about 20%. Options: accept it (workflows are ctx-bound), or count at the source level (instrument loop back-edges and calls). The second is extra work and risk; it is not measured here.
2. **mlua silently ignores `VmState::Yield` where the thread is not yieldable** (`process_status`: `if lua_isyieldable { lua_yield }` with no else). Returning `Yield` from a hook in a non-yieldable context would therefore keep the loop running. The sandbox checks `lua_isyieldable` itself before it yields.
3. **`ctx.try` must be Lua, not Rust.** A Rust `try` calls back into Lua from C, which is not yieldable, so a budget trip inside it could not yield.
4. **`__gc` cannot be budgeted in Lua 5.4** (hooks are off in finalizers). Rejected in `setmetatable`. Lua only honours `__gc` when it is present at `setmetatable` time, so the check is complete for that path.
5. **The wall-clock budget counts time blocked in `ctx.block_recv`.** An approval that waits for a human would trip it. The design should exclude blocked time (or give waits their own clock). Measured: the wall budget fires at 200 ms inside the block.
6. **Concatenation is not bounded per string.** `s = s .. s` is stopped only by the memory limit (`A.concat bomb`). A per-string check would need a VM-level hook or a wrapper for `..`, which Lua does not allow. The string budget covers `string.rep` only (and any other host function that builds strings).
7. **Error objects are strings.** A tables-as-errors protocol cannot cross the mlua boundary; use strings, or serialise to JSON in the script.
8. **`coroutine.close` runs `__close` non-yieldably** and the hook still terminates the run (`A.close: coroutine.close spin`). Pending `<close>` variables of an errored coroutine are never run.
9. **Native Lua limits are much higher than the budgets.** The stack reaches 999,988 frames before `stack overflow`. The memory limit alone would not stop a deep recursion on the Lua stack.
10. **Timing noise.** The box was shared (other workers compiling). Per-run medians moved by up to 40% between runs (for example well-behaved no-hook: 18.3 to 25.7 ms). Ranges are reported. Overheads were stable in ratio.

## Not done, or only partly done

- **Type stubs (F7):** not tested.
- **Process isolation:** not in scope (SPEC known limit); the VM runs in-process.
- **Peak memory:** only the hook-sampled peak (`peak_mem_sampled_bytes`). The allocator limit trips first in the memory tests, so the true peak is at most the limit.
- **Wall clock:** measured from VM creation; the design's per-run clock needs the blocked-time decision above.
- **Clean-build timing:** I used a fresh target directory in the scratchpad rather than `cargo clean -p`, so the other workers' cached artifacts were not removed.
- **Other platforms:** only Linux x86-64, glibc.
- **`=` and `@` chunk-name prefixes** for syntax-error file names: not tested.
- **Escape attempts beyond the battery:** `string.format` with huge width/precision, `coroutine.wrap` chains deeper than one level, and `__close` on a coroutine that is never resumed, were not exercised separately.

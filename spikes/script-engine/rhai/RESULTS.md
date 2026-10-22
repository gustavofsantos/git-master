# Spike S-D, Rhai: results

Engine: Rhai 1.27.0 (pure Rust, no C/C++ toolchain). The design wants Lua; this is the pure-Rust
alternative, evaluated on the same questions as the `luau/` and `lua54/` crates. Every number below
comes from the three full runs in `results/` (`rhai.jsonl`, `rhai-run2.jsonl`, `rhai-run3.jsonl`,
summary in `results/rhai-summary.txt`). Verdicts agree across the three runs for all 70 records (69 test names; the two `D.idle-rss` records share one name).

## Verdicts in short

| Must-have (SPEC) | Verdict | Evidence |
|---|---|---|
| No ambient authority (no os, io, load, require, debug, random, clock) | **Contained**, by absence | 13 capability probes all fail cleanly (section A). No file, process, clock or randomness function exists in the allow-listed packages. |
| Budgets, each with a distinct reason | **Contained** for instructions, string size, array size, stack depth, ctx calls, wall clock; **memory** only by a host hook | Section B. Map growth through index assignment is not checked by Rhai's size limit; the host hook is the only stop. |
| Termination is not catchable by the script | **Contained** | `try`/`catch`, `try_run`, and `catch` around `block_recv` all re-raise budget and cancel errors (section A). |
| Prompt cancellation (pure loop, host call, blocked `block_recv`, pcall loop, deep recursion) | **Contained** | Medians 0.016 to 0.10 ms, except the poll-only `block_recv` (2.9 to 3.2 ms, its 5 ms poll period) and the 900-level recursion (1.1 to 1.2 ms). All 900 trials (3 runs x 300) ended `cancelled` (section C). |
| Cheap fresh VM per run | **Fine** | Median 0.59 ms per run (engine 0.37 ms, compile of a 40-line module 0.10 ms). 8 threads x 1000 runs: no failures (section D). |
| Self-contained binary | **Fine** | Stripped binary 5.06 MB, no C or C++ compiler needed, no external program. Licence flags in section E. |
| Module shape `execute(ctx, input)` plus `META` | **Works** with two syntax changes (section F). `ctx.try` must be spelled `ctx.try_run`. |

Two findings change the design, not just the numbers:

1. Rhai's size limits are not on every mutation. `max_map_size` is not checked on `m[k] = v`,
   so a map grew to 20,000 entries against a 10,000 limit. Rhai checks sizes when a value is
   passed to a function or copied (for example `m.len()` or `let c = m`). A host memory budget is
   needed as a backstop (section B, `memory`).
2. The size check is recursive over the whole value, so each checked mutation costs O(total size).
   Filling a container is quadratic: a 10k-element array takes 44 ms, 20k takes 194 ms (probe
   numbers in section B). A 100k-element array would take about 5 s by extrapolation, so the wall-clock budget fires
   first unless the array limit is lower. `max_array` is set to 20,000 for this reason.

## What I built

- `src/alloc.rs`: a counting global allocator. Rhai has no byte limit, so the host counts the bytes
  live on the run's own thread (each run has a thread). The counters are thread-local, const-initialised
  and have no destructor.
- `src/sandbox.rs`: the host model. A fresh `Engine` per run:
  - `Engine::new_raw()` plus these packages only: `CorePackage` (includes `print`/`debug`, `Fn`/`call`,
    ranges), `LogicPackage`, `BasicMathPackage`, `BasicArrayPackage`, `BasicMapPackage`,
    `MoreStringPackage`. Not registered: `BasicTimePackage` (`timestamp`), `BitFieldPackage`,
    `BasicBlobPackage`. No module resolver.
  - `disable_symbol("eval")`; `on_print` and `on_debug` are no-ops (nothing reaches stdout).
  - The `ctx` type (`Ctx`, an `Arc` of per-run state) with methods `notify`, `now`, `count`,
    `block_recv`, `try_run`. Methods take `Ctx` by value, so they work on a constant binding.
  - `on_progress`: cancel flag on every call, memory on every call, wall clock every 1024 calls.
    It returns `Some(reason)` to terminate, which becomes `ErrorTerminated(reason)`, a system
    error that try/catch cannot catch.
  - `classify()` maps `ErrorTooManyOperations`, `ErrorStackOverflow`, `ErrorDataTooLarge` (by its
    message), `ErrorTerminated(reason)` and capability errors to reason labels.
- `src/battery_a.rs` to `battery_f.rs`: the batteries. `main.rs` dispatches them.
  `x-probe <file> [mem-MiB|off] [ops|off]` is an investigation helper that runs one file (used for
  the quadratic and map-limit probes below).

## Budgets (starting values, `Budgets::SPIKE`)

| Budget | Value | Reason label (on exit) | Mechanism |
|---|---|---|---|
| instructions | 50,000,000 operations | `instructions` ("Too many operations") | `set_max_operations` |
| wall clock | 2 s | `wall-clock` | progress hook (every 1024 calls) and inside `block_recv` |
| memory | 64 MiB live on the run thread | `memory` | counting allocator, checked on every progress call |
| string size | 1 MiB | `string-size` ("Length of string") | `set_max_string_size` |
| array size | 20,000 elements (recursive total) | `array-size` | `set_max_array_size` |
| map size | 10,000 entries | `map-size` | `set_max_map_size` (see finding 1) |
| stack depth | 64 call levels | `stack-depth` ("Stack overflow") | `set_max_call_levels` (Rhai default) |
| ctx calls | 10,000 per run | `ctx-calls` | counter in `Ctx`, `ErrorTerminated("ctx-calls")` |

The 16 MiB run-thread stack holds 900 call levels (battery C, deep recursion), so the 64-level
budget is far inside it. The smallest stack that works was not measured.

The well-behaved script (a 1e6-iteration loop of arithmetic and string work) counts 13,000,100
operations: about 3.8x headroom under the 50M budget.

## Lua to Rhai mapping

| SPEC item | Lua (design) | Rhai analogue used in this battery |
|---|---|---|
| `os` (time, clock, getenv, execute, exit) | `os.*` | none; `timestamp()` and `exec`/`getenv` probes are absent functions |
| `io` (open, read, popen) | `io.*` | none; `open()` probe is an absent function |
| `load`, `loadstring`, `dofile`, `loadfile` | load chunks | `eval` (disabled symbol); `import` (no resolver) |
| `require`, `package` | modules | `import` (no resolver) |
| `string.dump` | bytecode | none (no bytecode format; AST serde not enabled) |
| `debug.*` | debug library | `debug(x)` only formats; no hook or registry API. n/a |
| `collectgarbage` | GC control | none (reference counting) |
| metatables | `getmetatable`/`setmetatable` | none; property setters are not registered |
| `math.random` | randomness | `rand()`: feature not compiled |
| `_G`, `_ENV` | global table | none; `this` is unbound outside a method |
| `coroutine` | coroutines | none; `yield` is a reserved keyword |
| `pcall` | catch errors | `try`/`catch` (catches only non-system errors) |
| `error(value)` | any value | `throw value`: the value arrives at the host as `ErrorRuntime` |
| `ctx.try(fn)` | pcall-like | `ctx.try_run(fn)` (`try` cannot follow a dot, see finding 3) |
| `ctx.block_recv()` | blocking recv | same name, with the wall clock and cancel checked in its poll loop |
| `string.rep` bomb | | `"x".pad(10000000000, "x")` |
| `..` concatenation | | `+` (`"ab" + i.to_string()`) |

## A. Escape attempts

Verdicts are from `results/rhai.jsonl`. `blocked`: the capability is absent or fails cleanly.
`contained`: a budget ended the run with its reason. `n/a`: the concept does not exist in Rhai
(the probe is shown anyway). Every escape attempt is in the table; none reached host data.

| Test | Rhai analogue | Verdict | Observed outcome (exact message) |
|---|---|---|---|
| os.time/os.clock | `timestamp()` | blocked | `Function not found: timestamp ()` |
| os.getenv/os.execute | `getenv("HOME")` | blocked | `Function not found: getenv (&str | ImmutableString | String)` |
| os.execute | `exec("id")` | blocked | `Function not found: exec (...)` |
| io.open/io.read/io.popen | `open("/etc/hostname")` | blocked | `Function not found: open (...)` |
| load(string) | `eval("1 + 1")` | blocked | `reserved keyword 'eval' is disabled` (parse error) |
| loadstring/loadfile/dofile | `import "/etc/hostname" as m` | blocked | `Module not found: /etc/hostname` |
| require/package | `import "helper" as h` | blocked | `Module not found: helper` |
| string.dump | `"abc".dump()` | n/a | `Function not found: dump (...)` (no bytecode exists) |
| debug.getinfo/sethook/getregistry | `debug(input)` | n/a | returns `1`; output goes to the dropped `on_debug` sink |
| collectgarbage | `gc()` | blocked | `Function not found: gc ()` |
| getmetatable("").__index tampering | `s.len = 5` | blocked | `No writable property 'len' - a setter is not registered for type 'string'` |
| math.random/randomseed | `rand()` | blocked | `Function not found: rand ()` |
| setmetatable/rawset on globals | `ctx.notify = \|t\| {1}` | blocked | `No writable property 'notify' - a setter is not registered for type 'Ctx'` |
| _G/_ENV writes | `this` | n/a | `'this' not bound` |
| string.rep memory bomb | `"x".pad(10000000000, "x")` | contained | `string-size`: `Length of string too large` |
| table growth memory bomb | `a.push(1)` in `loop` | contained | `array-size`: `Size of array/BLOB too large` (after about 20k pushes, see finding 2) |
| map growth memory bomb | `m[k] = i` in `loop` | contained | `memory` (the size limit is not checked on index assignment, finding 1) |
| map bomb with large values | `m[k] = s`, `s` 512 KiB shared | contained | `memory` (same cause) |
| nested array memory bomb | `a = [a, a]` in `loop` | contained | `array-size`: `Size of array/BLOB too large` at about 14 steps. The recursive total is checked, so the nested bomb is bounded by the array limit |
| infinite loop | `loop {}` | contained | `instructions`: `Too many operations` |
| deep recursion (non-tail) | `fn r(n) { 1 + r(n + 1) }` | contained | `stack-depth`: `Stack overflow` |
| runaway string concatenation | `s += s` in `loop` | contained | `string-size`: `Length of string too large` |
| coroutine.wrap / yield | `yield 1;` | n/a | `'yield' is a reserved keyword` (parse error) |
| error(table) | `throw #{ code: 42, why: "x" }` | n/a | `ErrorRuntime`, payload `#{"code": 42, "why": "x"}` reaches the host |
| error(function) | `throw \|\| 1` | n/a | `ErrorRuntime`, payload `Fn*("anon$...")` |
| pcall swallows ops budget | `try { loop {} } catch {...}` | contained | `instructions`: not caught |
| pcall swallows ctx budget | `loop { try { f(ctx) } catch {} }`, `f` calls `ctx.count()` | contained | `ctx-calls`: `Script terminated`, not caught |
| pcall swallows stack budget | `try { r(0) } catch {...}` | contained | `stack-depth`: `Stack overflow`, not caught |
| pcall swallows string budget | `try { loop { s += s } } catch {}` | contained | `string-size`, not caught |
| ctx.try_run swallows ops budget | `ctx.try_run(\|\| loop {})` | contained | `instructions`, re-raised, not returned as an `error` outcome |
| blocked block_recv, no host cancel | `ctx.block_recv()` | contained | `Script terminated` at the 2 s wall clock (finding 4) |
| blocked block_recv inside try/catch | `try { ctx.block_recv() } catch { 1 }` | contained | `Script terminated` at the 2 s wall clock, not caught |
| pcall control | `let r = 0; try { throw "x" } catch (e) { r = e } r` | n/a | `"x"`: ordinary errors are caught (the control) |
| type of ctx | `type_of(ctx)` | n/a | `"Ctx"`: the type name only |
| iterate ctx | `for x in ctx {}` | blocked | `For loop expects iterable type` |
| ctx.__index / unknown property | `ctx.__index` | blocked | `Unknown property '__index' - a getter is not registered for type 'Ctx'` |
| ctx unknown method | `ctx.secret()` | blocked | `Function not found: secret (Ctx)` |
| Fn() reaching unregistered fn | `Fn("timestamp").call()` | blocked | `Function not found: timestamp ()` |
| Fn() reaching registered method | `Fn("notify").call(ctx, "x")` | n/a | returns `1`: allow-listed members are reachable by design |
| input mutation | `input.repo = "changed"` | n/a | `"changed"` inside the run; the host's map is a clone and is not changed |
| print/debug to stdout | `print("hi")` | n/a | returns `1`; `on_print` is a no-op, nothing reaches stdout |

**Tampering between runs**: run 1 tries `ctx.notify = |t| {1}`, `let len = |x| {0}`, `input.count = 999`
and `ctx.notify("...")`, each in its own `try`. Run 1 reports `attempts = 3`: the setter fails and the
other three succeed in their own scope. Run 2 (a fresh engine) returns `[3, 3, 2]` (`"abc".len()`,
`[1,2,3].len()`, and its own `ctx.count()`) with only its own note. Verdict: **contained**. Structural
reason: each run builds its own `Engine`, and no `Engine` or `Module` is shared, so nothing from run 1
can reach run 2.

**Lua-style concern to note for the design:** Rhai's `Engine` has no API to register functions from
script code, and the registered `ctx` functions are immutable after the engine is built. Sharing one
`Engine` between runs (an optimisation) would be safe for the same reason, but this was not tested.

## B. Budgets

Each budget case runs in its own child process (`b-one <case>`), so the process peak RSS (VmHWM)
belongs to that case alone. "Run peak" is the counting allocator's peak on the run thread.

| Case | Script | Reason | Elapsed (3 runs) | Run peak | Process VmHWM |
|---|---|---|---|---|---|
| instructions | `loop {}` | instructions | 193 / 209 / 253 ms | 0.49 MB | 7.2 MB |
| wall clock | `loop {}`, 500 ms, ops off | wall-clock | 500.0 ms | 0.49 MB | 7.2 MB |
| memory | map with a shared 512 KiB value, index assignment | memory | 1193 / 1374 / 1383 ms | 67.1 MB | 72.8 MB |
| nested array | `a = [a, a]` | array-size | 5.2 / 5.3 / 8.9 ms | 1.66 MB | 8.9 MB |
| string size | `s += s` | string-size | 2.0 / 2.1 / 2.9 ms | 2.75 MB | 8.5 MB |
| stack depth | `fn r(n) { 1 + r(n+1) }` | stack-depth | 1.0 / 1.4 / 1.5 ms | 0.50 MB | 7.5 MB |
| ctx calls | `loop { ctx.count(); }` | ctx-calls | 3.4 / 3.5 / 5.5 ms | 0.49 MB | 7.2 MB |

Process VmHWM includes the about 7 MB baseline of the test binary.

**Memory, measured.** There is no byte limit in Rhai's `Engine` API (`limits.rs` has operations, call
levels, variables, functions, modules, expression depth, string, array and map sizes). The closest
controls:

- The nested-array bomb is bounded by `max_array_size`, because Rhai counts the total recursively.
  It stopped at 1.66 MB peak.
- The string bomb is bounded per string by `max_string_size`. It stopped at 2.75 MB peak.
- Map growth through index assignment is **not** bounded by `max_map_size` (probe: 20,001 entries
  stored against a 10,000 limit, no error. The size error does appear when the map is passed to a
  function on each iteration, for example `m.len()` in the loop, which raised `Size of object map too
  large` (probe). The map bomb ran until the host's 64 MiB hook fired: peak 67.1 MB, process VmHWM 72.8 MB.
  The hook checks every call, so one operation can overshoot by one allocation.

Also measured (probe, not in the battery): a 512 KiB string is shared between map entries, so the
memory cost is the map nodes and keys, not the string.

**Cost of the size checks (probe numbers, `x-probe`, release, no hook):**

| Pushes into one array | Time |
|---|---|
| 2,000 | 3.7 ms |
| 5,000 | 13.1 ms |
| 10,000 | 44.3 ms |
| 20,000 | 193.9 ms |

Roughly quadratic (4.4x for 2x elements). The recursive size check runs on each checked mutation.

**Well-behaved script** (1e6 iterations of arithmetic and string work, 13,000,100 operations, completed
under all budgets; `budgets_tripped: false`): median of 9 interleaved runs per configuration, three
process runs. Hooks off (`Budgets::OFF`, no progress hook): 436 to 484 ms. Hooks on (`Budgets::SPIKE`,
progress hook installed): 468 to 482 ms. Overhead: -1.6%, +1.9%, +10.5% across the three process runs.
The hook's cost is within this box's noise, which came from other workers building on the same 4 cores.
A clean figure needs an idle machine.

## C. Cancellation latency

50 trials per scenario. The driver starts the run on its own thread, sleeps a pseudo-random 5 to 50 ms
(a fixed xorshift seed, so the points repeat), sets the cancel flag, and measures from that instant to
the run thread returning. For `block_recv` (wake sent), the driver also sends a wake value on the
channel. All 300 trials per run ended with `cancelled`; none completed on its own.

| Scenario | Median (3 runs) | Max over trials (3 runs) | Note |
|---|---|---|---|
| tight pure loop | 0.021 to 0.024 ms | 0.045 to 0.695 ms | hook checks the flag on every call |
| loop calling a cheap host fn (`ctx.now()`) | 0.018 to 0.020 ms | 0.034 to 1.278 ms | |
| blocked `ctx.block_recv()`, wake sent | 0.093 to 0.104 ms | 0.160 to 0.310 ms | the host sends a wake value |
| blocked `ctx.block_recv()`, poll only | 2.9 to 3.2 ms | 4.9 to 5.2 ms | polls with a 5 ms `recv_timeout`; the bound without a wake |
| script inside `try` in a loop | 0.016 to 0.019 ms | 0.054 to 0.418 ms | the termination is not caught |
| deep recursion (900 levels) then loop | 1.1 to 1.2 ms | 1.6 to 9.1 ms | unwinding 900 frames |

The maxima come from scheduler noise on the shared box (other workers were building); the medians are
the stable figures. Trial order and cancel points are fixed by the seed, so the runs are comparable.

## D. Cost of a fresh VM per run

Module: 40 lines (`META` plus `execute`, with `ctx.try_run`, `ctx.notify`, `ctx.now`, string
interpolation, a `for` loop, and a `throw`). Each cycle is: build `Engine` with packages and `Ctx`
registration, install the hook, compile, call `execute`, drop everything.

| Measure | Value (3 runs) |
|---|---|
| Engine build alone (median) | 360 to 378 us |
| Compile of the 40-line module (median) | 99 to 104 us |
| Full cycle, sequential, 1000 runs (median) | 593 to 655 us |
| Full cycle, sequential, p99 | 740 to 1120 us |
| Full cycle, max | 1.2 to 4.9 ms |
| Idle fresh VM, resident (engine only, child process) | +3.2 to +3.4 MB |
| Idle, engine plus compiled module | +4.1 to +4.3 MB |
| 8 threads x 1000 runs, wall | 1.27 to 1.47 s |
| 8 threads, per-run median | 592 to 620 us |
| 8 threads, per-run p99 | 5.7 to 8.7 ms |
| 8 threads, failures | 0 in every run |
| 8 threads, process VmHWM | 11.7 MB |

The tail under 8 threads is high (p99 about 14x the median) because the 8 threads share 4 cores with
other workers. The per-run median holds.

## E. Binary and build

- Stripped release binary (`CARGO_PROFILE_RELEASE_STRIP=true`, `CARGO_PROFILE_RELEASE_LTO=thin`, default
  opt-level 3 with no `opt-level = "z"`): **5,058,320 bytes**. This includes the test harness and the
  counting allocator; it is not a product binary.
- Clean build: `cargo clean --release -p spike-script-rhai -p rhai` then `cargo build --release -p spike-script-rhai`
  with the same env vars: **39.4 s wall** (one run, measured after the last code change; an earlier clean build
  of the same dependencies took 50.7 s). Measured on 4 cores shared with other workers.
- Toolchain: rustc and cargo 1.99.1. No C or C++ compiler is needed: no `cc` crate, no `-sys` crate in the tree.
- `cargo tree -p spike-script-rhai --depth 1`:
  `rhai v1.27.0` and `serde_json v1.0.154` (the spike's own dependency).
- Crates in the tree: 57 lines, 40 unique (including the spike crate). Versions:
  rhai 1.27.0, rhai_codegen 3.3.0, serde 1.0.229, serde_core 1.0.229, serde_derive 1.0.229, serde_json 1.0.154,
  ahash 0.8.12, const-random 0.1.18, const-random-macro 0.1.16, getrandom 0.2.17 and 0.3.4, once_cell 1.21.4,
  smallvec 1.16.3, smartstring 1.0.1, thin-vec 0.2.21, num-traits 0.2.19, memchr 2.8.3, libc 0.2.186,
  proc-macro2 1.0.107, quote 1.0.47, syn 2.0.119 and 3.0.9, unicode-ident 1.0.27, zerocopy 0.8.63,
  bitflags 2.13.2, cfg-if 1.0.5, autocfg 1.5.1, itoa 1.0.18, portable-atomic 1.15.0, static_assertions 1.1.0,
  version_check 0.9.5, zmij 1.0.23, crunchy 0.2.4, tiny-keccak 2.0.2.
- Licences (`cargo tree` with the licence field):
  - MIT, Apache-2.0, or dual: rhai, serde family, serde_json, ahash, const-random, getrandom, once_cell, smallvec,
    thin-vec, num-traits, memchr (Unlicense OR MIT), libc, proc-macro2, quote, syn, bitflags, cfg-if, autocfg,
    itoa, portable-atomic, static_assertions, version_check, zmij, crunchy (MIT), zerocopy (BSD-2-Clause OR Apache-2.0 OR MIT).
  - **Flagged, not on the SPEC's list:**
    - `smartstring 1.0.1`: **MPL-2.0+** (weak copyleft). It is a non-optional dependency of rhai
      (`[dependencies.smartstring]` has no `optional = true`), so it cannot be dropped by a feature.
    - `tiny-keccak 2.0.2`: **CC0-1.0**. It enters through rhai's default `ahash/runtime-rng` (via `const-random`).
    - `unicode-ident 1.0.27`: **(MIT OR Apache-2.0) AND Unicode-3.0**. Permissive, but not on the list.
    - `crunchy 0.2.4`: MIT, fine. `memchr`: Unlicense OR MIT, fine.
- Features used: `rhai` with `sync`, `metadata`, `internals`. `metadata` and `internals` together enable
  `Engine::definitions()` (the `.d.rhai` writer, section F). They add serde, serde_json and serde_derive to
  the tree (rhai's own deps). The lock change is additive: `spikes/Cargo.lock` now lists `serde`,
  `serde_json` as dependencies of `rhai`. Nothing was removed from the lock. This is outside
  `script-engine/rhai/` and should be known to whoever owns the spikes workspace.

## F. API facts the design needs

Each line is a probe with its evidence in `results/rhai.jsonl` (`F.*`).

1. **Module shape.** `fn execute(ctx, input) {...}` plus `const META = #{...};` at top level works. A
   module without `execute` fails with `Function not found: execute` (capability-missing). `return` is
   a keyword and works inside functions (`return 7;` gives 7); there is no `return M` at the top level,
   which is not needed.
2. **`M.meta` with no ctx and no host functions.** `read_meta()` builds `Engine::new_raw()` with no
   packages and no `Ctx` type, compiles, runs only the top level (`run_ast_with_scope`) and reads the
   `META` constant from the scope. It returns `#{"name": "sync-repo", ...}`. A top-level `ctx.notify(...)`
   fails with `Variable not found: ctx`, so the meta reader has no host access. The reader does run the
   top level, so `META` must be computable without a package. A `META` that calls a built-in function was
   not tested.
3. **Syntax errors.** `ParseError` is `(Box<ParseErrorType>, Position)`. `Position::line()` and
   `position()` give line and column, both `Option<usize>`. Probes:
   - `let x = ;` at line 2: `Unexpected ';'`, line 2, column 13 (correct).
   - unclosed block: `Expecting '}' to terminate this block`, line 3, column 1.
   - `let yield = 1;`: `'yield' is a reserved keyword`, line 2, column 9.
   - `eval(...)`: `reserved keyword 'eval' is disabled`, line 2, column 5.
   Nested `fn` inside a function is a parse error: `Function definitions must be at global level and cannot be inside a block or another function`.
   The `for _ in` loop variable is a parse error (`_` is not a name).
4. **`error()` values.** `throw #{code: 42, path: "a/b"}` reaches the host as `ErrorRuntime` with the map
   as payload (`#{"code": 42, "path": "a/b"}`); `throw || 1` gives a `FnPtr` payload. The host reads the
   payload with `EvalAltResult::ErrorRuntime(v, _)`. `Failure.payload` holds its debug form.
5. **Listing ctx's functions.** Via `engine.definitions().single_file()` (the `metadata` and `internals`
   features). Single-file output lists the five `Ctx` methods (`fn notify(_: Ctx, _: string)`,
   `fn now(_: Ctx)`, `fn count(_: Ctx)`, `fn block_recv(_: Ctx)`, `fn try_run(_: Ctx, _: Fn)`). The
   return types are printed as `EvalAltResult>>` (a rendering glitch in the writer, not verified as a
   bug). The unknown-name message is `Function not found: secret (Ctx)`, which names the type but not
   the valid names. The host has to build the "valid names are" list itself, from the definitions output
   or a static list.
6. **Type stubs for editors.** Rhai's definitions writer (`engine.definitions()`, `write_to_dir`) produces
   three `.d.rhai` files: `__builtin__.d.rhai`, `__builtin-operators__.d.rhai` and `__static__.d.rhai`. I did not verify whether any editor or language server reads them (no rhai-lsp or
   editor extension is in the registry), so **editor support is unverified**. The `.d.rhai` format itself is
   what the writer emits.
7. **`ctx` and `input` are parameters, not constants.** They arrive as arguments to `execute`. `input` is a
   parameter, so a script can assign to its fields inside the run (`input.repo = "changed"` succeeded). The
   host's map is a clone and is unaffected. Rhai has no way to freeze a value, so "read-only" here means
   "the host passes a copy". `ctx` is passed by value, so its methods work on the binding.

## Surprises and findings

1. **`ctx.try` is a syntax error.** `try` is a keyword, so `ctx.try(fn)` fails to parse
   (`Expecting name of a property`). The spike uses `ctx.try_run(fn)`. Any design that uses `try` as a
   method name in Lua must rename it for Rhai.
2. **Captured variables and the receiver of the same call are a data race in Rhai.**
   `let r = ctx.try_run(|| { scan(ctx, input) })` fails with `Data race detected on argument #1 of
   function 'try_run'`, because the closure captures `ctx` and `ctx` is the receiver. The fix that works:
   `let c = ctx; ctx.try_run(|| { scan(c, input) })`. Module authors will hit this; the design doc's examples
   would need the alias.
3. **Map size limit is not enforced on index assignment.** `m[k] = v` stores new entries without a size
   check. The check runs when the map is passed to a function (`m.len()`), copied (`let c = m`), and at
   other places. A map grew to 20,001 entries with `max_map_size = 10,000` and no error. The host's memory
   hook is the only backstop (section B). Size limits are a per-operation check, not a global invariant.
4. **`block_recv` could outlive the wall-clock budget.** The progress hook does not run during a host call,
   so a script blocked in `ctx.block_recv()` with no sender was not stopped by the budget (found by a probe
   that hung). Fixed: `CtxState` carries the run's deadline and `block_recv` checks it in its poll loop.
   Battery A now has three cases for this (no cancel, inside `try`/`catch`, inside `try_run`), all ending
   with `wall-clock`.
5. **Size checks are O(total size) per checked operation.** Filling an array to 20k elements costs 194 ms
   (quadratic). The array budget is set to 20k so that `array-size` fires before the 2 s wall clock.
6. **Nested totals are counted.** `calc_array_sizes` recurses, so `a = [a, a]` hits the array limit after
   about 14 steps. This is better than I expected, and it means nested containers are bounded by the array
   budget as long as each step is checked.
7. **`try`/`catch` cannot swallow system errors.** Budget and cancel errors are `ErrorTerminated`,
   `ErrorTooManyOperations`, `ErrorStackOverflow` or `ErrorDataTooLarge`. `is_catchable()` is false for all
   of them, and script-function calls pass them through unwrapped (`func/script.rs` checks
   `is_system_exception()` first). Every pcall-style test confirms this.
8. **A `try` as the last statement returns `()`.** `try { throw "x" } catch (e) { e }` evaluated to `()`
   when used as the final expression, so the control case assigns to a variable instead.
9. **`print`/`debug` are in `CorePackage`** (through `BasicStringPackage`), so they exist even though the
   spike does not want them. `on_print`/`on_debug` replace them with no-ops.
10. **`ctx.try_run` re-raises system errors.** Tested: an infinite loop inside `try_run` ends with
    `instructions`, and a thrown error comes back as an `error` outcome with the message.

## Syntax: what would change in the design (Rhai is not Lua)

The design's `Lua scripts` premise and its examples would change. Specifically:

- `local M = {}; function M.execute(ctx, input) ... end; return M` becomes
  `const META = #{ ... }; fn execute(ctx, input) { ... }`. The module object is gone.
- Tables are either arrays (`[..]`, 0-based) or object maps (`#{ key: value }`), with `#{}` for an empty map.
  Lua tables are both, and 1-based.
- There is no `local`; `let` declares a variable in the current block.
- `function` is `fn`, and functions must be top level (nested `fn` is a parse error).
- `..` is `+` for strings. `#s` is `s.len()`, bytes. There is no `#` operator.
- Loops: `for i in 0..n { }`, `while`, `loop { }` with `break` (all verified). `for _ in` is a parse error.
- `pcall(f)` is `try { f() } catch (e) { ... }`. Verified: `catch (e)` receives the thrown value of a `throw`
  (`"x"`). What `e` holds for a runtime error was not checked.
- String interpolation is backticks: `` `${a}:${b}` `` (verified in the 40-line module).
- Closures are `|x| { ... }` and `|| expr`. They capture by shared reference, which causes the data race
  described in finding 2.
- Method names: `try` cannot follow a dot (verified). Check any other name used after a dot against the keyword
  list before using it; only `try` was tested.

## Could not do, or not verified

- **Editor type-definition support**: the `.d.rhai` writer exists, but no editor consumer was checked.
- **Smallest stack**: the 16 MiB stack was chosen; the smallest that works was not measured.
- **Memory overshoot of one operation**: the hook checks memory on every call, but one `[a, a]` or one
  string concatenation allocates before the check. The peak (67.1 MB for a 64 MiB budget) is about
  right for this workload; a worst case of about 2x (one doubling) is the theoretical bound, not measured.
- **Hook overhead on an idle machine**: the figures above are from a shared box.
- **`x-probe` and the child subcommands** are investigation and measurement helpers, not part of the
  design. They are in `main.rs`.
- **Cargo.lock**: the lock in `spikes/` changed additively (see section E). Not reverted, because the
  `definitions()` writer needs `metadata`.

## Files

- Raw output: `results/rhai.jsonl` (run 1), `results/rhai-run2.jsonl`, `results/rhai-run3.jsonl`.
- Human summaries and spread: `results/rhai-summary.txt`.
- Source: `src/main.rs`, `src/alloc.rs`, `src/sandbox.rs`, `src/battery_a.rs` to `src/battery_f.rs`.
- Build: `cd /home/user/git-master/spikes && CARGO_TARGET_DIR=$HOME/.cache/gm-spikes/target CARGO_PROFILE_RELEASE_STRIP=true CARGO_PROFILE_RELEASE_LTO=thin cargo build --release -p spike-script-rhai`.
- Run: `$CARGO_TARGET_DIR/release/spike-script-rhai all` (or `a`, `b`, `c`, `d`, `f`).

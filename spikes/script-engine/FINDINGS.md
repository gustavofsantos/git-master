# Spike S-D: script engine for workflows (Q22)

Measured on 2026-10-22 and 2026-10-23 by the agent, in a cloud container: Linux x86_64, 4 vCPU, 15 GB, rustc 1.99.1, gcc/g++ 13.3, cmake 3.28. Three isolated crates ran the same battery (`SPEC.md`): `luau/` (mlua 0.11.6, Luau 0.709), `lua54/` (mlua 0.11.6, Lua 5.4.8 vendored) and `rhai/` (Rhai 1.27.0). Each crate has its raw output in `results/`; `lua54/RESULTS.md` and `rhai/RESULTS.md` carry the full tables, and `luau/results/luau-summary.txt` is the Luau equivalent. **Nothing was measured on macOS or Windows.** The first runs shared the machine with the other workers, so their tail timings are noisy; the Luau numbers below were re-measured on an idle machine.

## Verdict

**Luau via `mlua` passes every must-have and is the engine for D98 (decided as D104).** Lua 5.4 also passes, but only with guards the battery forced on it, and with a much larger hook cost. Rhai is contained too, but it is not Lua, it has no byte-level memory limit and it would change the language of the design and of every example.

| Must-have | Luau | Lua 5.4 | Rhai |
|---|---|---|---|
| Escape attempts (production sandbox) | 0 escaped in 52 probes plus the tamper test | 0 escaped in 68 cases plus the tamper test | 0 escaped in 13 capability probes plus the budget tests |
| A script cannot catch a budget error and go on | yes, with a latch (see below) | yes, but only with a per-instruction hook after the first trip | yes, no `try/catch` swallows a termination |
| Budgets, each with a distinct reason | all six | all six | five of six; memory only by a host allocator hook |
| Cancellation, median | 0.1 to 0.2 ms, all five loop types | 0.14 to 0.85 ms | 0.02 to 0.1 ms |
| Cancels a call blocked on a channel | yes, 0.2 ms | yes | yes |
| Fresh VM per run, median | 0.22 ms (p99 0.45 ms) | 0.37 to 0.47 ms | 0.59 ms |
| Instruction hook overhead, 1e6-iteration loop | 12% (idle machine) | 70% to 90% | not comparable |
| Stripped release binary | 2.9 MB (static C++ runtime) | 1.6 MB | 5.1 MB |
| Needs | g++ or clang++ | cc | nothing |
| It is Lua | yes | yes | no |

## What was tested, and how far it can be trusted

- **Luau and Lua 5.4:** every escape attempt of `SPEC.md` A ran against a fresh VM. The `escaped` verdicts in `lua54.jsonl` (4) are deliberate broken variants (a hook that only raises an error, no switch to a per-instruction hook, a per-thread hook), kept to show why each guard exists; I checked each one. The production sandbox has none. The Luau run, repeated on an idle machine with the statically linked build, gave the same verdicts as the first runs: 43 blocked, 27 contained, 32 n/a (reported only), 7 completed (well-behaved scripts).
- **Numbers I verified myself:** the 0-escape count and the verdict classes of both Lua engines (from their jsonl files), the binary sizes and the dynamic libraries (`ldd`), the Luau cancel, fresh-VM and hook-overhead figures (idle re-run), and the static `libstdc++` build. The other figures come from the workers' own runs; the raw files are in `results/` and three full runs agreed on every verdict.
- **One outlier:** a tight-loop cancel took 60 ms on the first of 50 trials on the idle run (thread start); the other 49 were under 5 ms.

## Luau: what the host must do

1. **Remove names.** `sandbox(true)` removes no names, only makes the library tables and `_G` read-only. The host removes 10: `os`, `debug`, `print`, `require`, `load`, `loadstring`, `collectgarbage`, `gcinfo`, `getfenv`/`setfenv`, `newproxy`; and `math.random`, `math.randomseed`, `string.dump`. `io`, `package`, `loadfile` and `dofile` are absent from Luau already.
2. **Latch the budget error.** A `pcall` can catch the first budget error, but the interrupt hook latches: the next call, return or loop back-edge raises again, and the host reports the latched reason, so the script cannot make progress. Memory errors need host wrappers for `pcall`, `xpcall` and `coroutine.resume` (these halve the `pcall` recursion depth, 99 against 198).
3. **Write `ctx.try` in Luau**, not in Rust: the Rust path loses table error values.
4. **String size** is enforced only for `string.rep` (a host wrapper, 16 MiB). Concatenation is capped only by the 64 MiB memory budget.
5. **The wall-clock budget must not count time blocked** in `approve`, `wait` or `exec`. In the spike it does (a blocked `block_recv` trips a 300 ms wall budget), and a human approval takes longer. The run's wall budget measures the script's own time; waiting for a person has its own timeout.
6. **Starting budget values** (not tuned): 20,000,000 interrupt points, 2 s wall clock of script time, 64 MiB memory, 16 MiB per string, 1000 `ctx` calls. The stack depth is Luau's own (20,000 Lua frames, 200 nested C calls) and is not configurable from the host.

## Build and shipping (Luau)

- **C++ is required.** Luau is C++17. The build needs `g++` or `clang++` (and `cc` for bundled SQLite, which the project already needs). On Linux the default link is dynamic against `libstdc++.so.6`, and the worker found `-static-libstdc++` had no effect. What works: `CXXSTDLIB=static=stdc++` plus `RUSTFLAGS="-L $(dirname $(g++ -print-file-name=libstdc++.a))"`. With them the binary depends only on glibc and `libgcc_s`, like the Lua 5.4 binary, at 2.9 MB stripped. The release workflow has to set both, which means a decision for packaging (D104).
- **macOS:** the system C++ runtime (`libc++`) is always present, so no equivalent is needed, but this build was **not measured**. CI has to prove it (a matrix job on `macos-latest`) before WS0 depends on it in a release.
- **Dependencies:** 28 crates, all permissive licenses (compatible with D103). Lua 5.4: 27 crates, all permissive. Rhai: three flags (`smartstring` MPL-2.0, `tiny-keccak` CC0, `unicode-ident` Unicode-3.0).
- **Clean build:** 30 to 40 s on 4 cores, for Luau, Lua 5.4 and Rhai alike.

## Why not the others

- **Lua 5.4.** Smaller binary, plain C. But it has no sandbox: it needs a manual allow list, and the battery forced four guards (each one reproduced a hang or an escape first): reject `__gc` in `setmetatable` (Lua disables hooks inside finalizers, which hung the run); after the first trip switch to a per-instruction hook in every thread; use a global hook, because a per-thread hook misses coroutines the script creates; write `ctx.try` in Lua. Its hook costs 70% to 90% on loop-heavy code because Lua 5.4 traps every instruction while a hook is set.
- **Rhai.** Contained, fast to cancel and pure Rust. But it is not Lua: `const META`, `fn execute`, `#{}` maps, 0-based arrays, no `local`, `try` is a keyword (so `ctx.try` would be `ctx.try_run`), and a closure that captures the same `ctx` it is called on hits a "Data race detected" error. It has no byte limit on memory: `max_map_size` is not checked on `m[k] = v`, so only a host counting allocator stops a map bomb (peak 72.8 MB RSS against a 64 MiB budget). The design, the docs and the examples all say Lua.

## Not tested

- macOS and Windows (build and run).
- Editor type stubs: Luau's `.d.luau` format is the planned `.api/` type file (D102), but `luau0-src` has no type checker, so the format was **not verified** against an editor. WS1 has to check it, or settle on `API.md` only.
- The `=` and `@` chunk-name prefixes in syntax errors (the format `file:line: message` for `gitmaster workflow check` was seen with a plain chunk name).
- A Luau VM running real GitMaster `Command`s: the spike's `ctx` is a fake.
- Soak: nothing ran longer than the battery (9,000 fresh VMs per engine, 8 threads, 0 failures).

## Known limit (unchanged from D99)

The VM runs in the same process. Its safety is that of its C++ implementation. This spike tested what a script can reach through the API. It did not fuzz Luau.

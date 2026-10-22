# Spike S-D: the measurement battery (the same for every engine)

Three crates, one per engine: `luau/` (mlua + Luau), `lua54/` (mlua + Lua 5.4, vendored), `rhai/` (Rhai).
Each crate is one binary, `src/main.rs` (split into modules if you like), that runs the battery below
and prints **one JSON object per line** to stdout (`{"test": "...", "verdict": "...", ...}`) plus a
human summary to stderr. Design: `design/workflow-scripts.md`, section "Sandbox (D99)". Roadmap: S-D.

The question being answered: can the engine host an **untrusted** workflow script with no ambient
authority, hard budgets and prompt cancellation, cheaply, in one self-contained binary?

## The host model every engine must implement

- A **fresh VM per run**. Globals the script can reach are a small allow list; everything else is absent.
  The environment must be read-only to the script (a script cannot change what the next run sees).
- One host object `ctx` with functions: `ctx.notify(text)` (records the text), `ctx.now()` (returns the
  host's fake clock, an integer), `ctx.block_recv()` (blocks the calling thread on a channel until the
  host sends a value, **or** until the run is cancelled; on cancel it must return an error that ends the
  run), `ctx.try(fn)` (calls `fn`, returns `{ outcome = "done", value = ... }` or `{ outcome = "error", message = ... }`),
  and `ctx.count()` (a host function that counts calls, to model the per-run limit on `ctx` calls).
- The script is a module: `local M = {} ; function M.execute(ctx, input) ... end ; return M`
  (Rhai: the equivalent, a function `execute(ctx, input)`; say how you mapped it).
  `input` is a read-only table/map of plain data.
- A run executes on **its own OS thread** (the host later runs one thread per run).
- Budgets, all enforced by the host, each ending the run with a distinct failure reason:
  instructions (or an equivalent counter), wall-clock time, memory bytes, string size (max bytes of one
  string a script can build), stack/recursion depth, and number of `ctx` calls.
  Pick **starting values** and report what you chose; the numbers will be tuned from your results.
- Cancellation: a shared flag (`AtomicBool`) or token. It must stop a pure-Lua loop (interrupt hook) and
  wake a blocked `ctx.block_recv()`.

## Tests (one JSON line each; `verdict` is `"blocked"`, `"contained"`, `"escaped"` or `"n/a"`)

A. **Escape attempts.** Each is a short script run in a fresh VM. Verdict `blocked` means the capability
is absent or errors cleanly. `escaped` means the script got real access (file contents, a process, the
clock, randomness, a loaded chunk, bytecode). Report, for each, the exact error message or output.
`os` (os.execute, os.getenv, os.time, os.clock, os.exit), `io` (io.open, io.popen, io.read),
`load`/`loadstring`/`dofile`/`loadfile`, `require`/`package`, `string.dump`, `debug` (debug.getinfo,
debug.sethook, debug.getregistry), `collectgarbage`, `getmetatable("").__index` tampering,
`math.random` and `math.randomseed`, `setmetatable`/`rawset` on the globals, `_G`/`_ENV` writes,
`string.rep` memory bomb, `table` growth memory bomb, infinite loop, deep recursion
(non-tail, 1e6 levels), runaway string concatenation, coroutine tricks (coroutine.wrap of an infinite
loop; a coroutine yielding across a host call), error objects that are tables or functions,
`pcall` swallowing a budget error (**important**: a script must not be able to catch the budget error
with pcall and carry on; report what happens), and a script that tries to reach other host data
through `ctx` (e.g. `getmetatable(ctx)`, iterating `ctx`, `ctx.__index`).
Rhai has no os/io/load by default: test the analogues that exist (eval, import, print/debug, Engine
fn access, `Dynamic` tricks, infinite loop, deep recursion, memory bomb, `max_*` limits) and mark the
rest `n/a` with a reason.
**Tampering between runs**: run script 1 that tries to modify a shared builtin (e.g. `string.format = ...`,
`ctx.notify = ...`, `table.insert = ...`, `_G.leak = 1`); then run script 2 in a new VM and report
whether any change is visible (it must not be).

B. **Budgets.** For each of instructions, wall clock, memory, string size, stack depth, ctx-call count:
a script that hits it. Report: did the run end, with what reason text, how long it took, peak memory
if you can read it. Also a **well-behaved script** (loop to 1e6 doing arithmetic and string work) to show
the budgets do not trip normally, and its wall time with and without the interrupt/instruction hook enabled
(the hook's overhead, in percent).

C. **Cancellation latency.** Cancel (flip the flag) at a random point (do 50 trials, report min, median,
p99, max in ms) for: a tight pure loop; a loop calling a cheap host function; a thread blocked in
`ctx.block_recv()`; a script inside `pcall` in a loop; a deep recursion. All must end promptly.

D. **Cost of a fresh VM per run.** Time to create a VM, install the sandbox and the `ctx` object, load
a ~40-line workflow module, call `execute`, and drop everything: report the median and p99 over 1000 runs,
and the resident memory of an idle fresh VM. Also: 8 runs in parallel threads, 1000 runs each, any problem?

E. **Binary and build.** Report the stripped release binary size (`strip`; build with `opt-level = "z"`
is NOT used, use the workspace/default release profile plus `strip = true`, `lto = "thin"` via
`CARGO_PROFILE_RELEASE_*` env vars) of your binary, a clean build time (`cargo build --release -p <crate>`
after `cargo clean -p <crate>`, vendored C/C++ included), the C/C++ toolchain it needs, and the list
of transitive crates it adds (`cargo tree -p <crate> --depth 1` and the count of `cargo tree -p <crate> | wc -l`)
and their licenses (`cargo tree` + each crate's `license` field; flag anything not MIT/Apache-2.0/BSD/ISC/Unlicense/Zlib).

F. **API facts the design needs** (answer in a short list, with the evidence): can the VM run the module
format above; can `M.meta` be read in a VM that has **no `ctx`** and no host functions at all; how does
the host get a line number and message from a syntax error (needed for `gitmaster workflow check`);
how are `error()` values with tables surfaced; can the host list the functions of `ctx` (to build the
"unknown name, valid names are: ..." message); is there a type-definition format for editors (Luau has
`.d.luau` / Rojo-style type stubs; Lua 5.4 has none standard; Rhai?).

## How to run and report

- Build with the shared target dir: `export CARGO_TARGET_DIR=$HOME/.cache/gm-spikes/target`.
- Do **not** touch files outside your own crate directory, except to read. Do not edit `spikes/Cargo.toml`,
  other crates, `design/`, `docs/` or the main workspace. Do not commit.
- Put the raw output in `results/<engine>.jsonl` and `results/<engine>-summary.txt` inside your crate directory.
- Write `RESULTS.md` in your crate directory: what you did, the table of results by test (with numbers),
  every escape attempt with its observed outcome, the API facts (F), anything surprising, anything you
  could not do and why. **Numbers must come from runs, never from memory.** If something fails to build or
  a test cannot be written, say so plainly instead of skipping it.
- Run the release binary at least 3 times; report the spread for timing tests.
- Pin versions you used (`cargo tree -p <crate> --depth 1`) in `RESULTS.md`.
- Your final message: five lines at most (verdict per must-have, binary size, anything blocking). The
  detail is in `RESULTS.md`.

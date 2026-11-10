# Architecture

Last updated: 2026-10-29. The status of each decision is in `design/decisions.md`.

## Overview (D08, decided)

One engine, two front ends (GPUI and MCP). Dependencies point inward (hexagonal architecture, ports and adapters).

```
gm-ui (GPUI)          gm-mcp (stdio JSON-RPC, own process)
      \                    /
            gm-core
   (engine, ports and their adapters:
    git subprocess, hooks, SQLite stores)

gm-schema: shared types (serde + JSON Schema) for UI, MCP and hooks
```

The core defines the traits (`GitPort` and the others), and the adapters live in `gm-core` too. D27 allows extracting them into `gm-git`, `gm-hooks` and `gm-store` when the core grows heavy; until then "the UI never imports the Git adapter" is a convention. The core is tested with fakes, without a real repository.

## Rules

1. **Commands in, events out.** `dispatch(Command { actor, ... })` returns an `OperationId`; the front ends subscribe to a stream of `Event`.
2. **One state machine per operation:** `Requested → Gating → AwaitingApproval → Executing → Done | Failed | Vetoed | Cancelled`. The UI and MCP are adapters of this contract. A human actor skips approval; an agent does not. `gitmaster mcp` is a second process with its own engine on the same repository (D135); it shares only the files in `.git/gitmaster/`, so an agent write that needs approval has no approver there until a later decision.
3. **One writer per repository (actors).** A mailbox serializes the mutations; reads come from immutable snapshots. libgit2's `Repository` is not `Sync`. The order of the operation log is the real execution order.
4. **Separate runtimes.** GPUI has its own executors. The engine runs on a dedicated tokio runtime, on threads of its own, and talks to the UI over channels. The UI never waits for the engine.
5. **Hybrid Git backend (D26):** libgit2 for objects, index, and in-memory merge/rebase; `git` subprocess for status in massive repositories (libgit2 does not use fsmonitor or the untracked cache) and for commit-graph at scale. Check the state of gitoxide before closing (Q6).
6. **Git is the source of truth.** The engine's state is derived and disposable. Only metadata lives outside Git: stack parents (git config), comments and snapshots (store). Every read revalidates fingerprints; a watcher (`notify`, with debounce) emits `RepoChanged`.
7. **One `gitmaster` binary** with subcommands: it opens the app by default; `gitmaster mcp` is the MCP server: a second process over stdio, no socket (D135).
8. **Extension by process**, not by dynamic library (the Rust ABI is not stable; security).

## Workflows (D60; direction D98 to D102)

> **Direction (2026-10-22, D98 to D102; built by WS2, D108).** Workflows were first a TOML state machine (WF0 to WF2). They are now Lua scripts in a sandbox, with `ctx` as the only effect boundary and ephemeral execution, and the TOML engine is removed. The full design is in `design/workflow-scripts.md`. This section describes the architecture as built.

- A workflow is a **Lua script** in one `.lua` file. It runs in a sandbox embedded in the binary. The engine is Luau through `mlua`, chosen by spike S-D (D104, Q22).
- The script receives two values: `ctx`, the only way it can act, and `input`, which is data only.
- **`ctx` is the single effect boundary.** Every call is an engine `Command` (D60) and a journal entry. The host decides approval (D101), not the script. An outcome other than `done` raises an error by default (fail-closed); `ctx.try` returns it to the script instead.
- The action catalog (`gm-core/src/script/catalog.rs`) generates `ctx`. Each action becomes a typed function with the same effect level and outcomes, named `ctx.<domain>.<verb>`.
- Each step dispatches an existing `Command`. A workflow has no logic path of its own.
- **Each run is an operation** (`OperationId`). It runs on its own thread, outside the UI. `ctx.approve` and `ctx.wait` block that thread on a channel. No coroutine crosses a host call.
- Events produced by a run carry the run's id and never fire triggers (no cascade).
- `exec` runs `argv` directly, never through a shell (D74).
- **Journal.** The host records each `ctx` call in `state.db`: sequence, function, resolved arguments, result, outcome and duration. The Runs view is built only from the journal. Output that the script writes with `ctx.notify` is marked "from the script".
- **Ephemeral execution (D100).** Closing the app ends the runs in progress. On the next launch, a run that did not finish is marked `interrupted`. An `apply` that started and has no result in the journal shows as interrupted and never runs again (D80, without resume). Waits, timeouts and `concurrency` queues live in memory. The journal is a history, not a recovery mechanism.
- Durable execution in the Temporal style stays open (Q23). The sandbox removes non-determinism, `ctx` is the only effect boundary, and the journal already records `(seq, function, arguments, result)`, so the door stays open.
- `HookPort` stays until the WF3 migration (D75): the `prepare-commit` hook still runs.

### What exists after WS2

The Lua code is in `gm-core/src/script/` (the port, the Luau adapter in `luau/`, the catalog and the generated `ctx` surface, the loader, `M.meta`, the review store, the run host, the journal, the templates, the `.api/` generator and the subcommands), with `workflow_session.rs` and `script_session.rs` in `gm-core/src/`. The TOML engine (`model`, `parse`, `expr`, `validate`, `machine`, `edit`, `dry_run`, `driver`, `store`, `flow_graph*`, `flow_inspector`), the Flow tab and the `.toml` fixtures are removed. A `.toml` file in the workflows directory is ignored with a visible warning. Two formats are never supported at the same time: two interpreters would double the surface.

## Threads (D119 to D129, planned)

Agent sessions are captured into `.git/gitmaster/threads.db`. Full design in `design/threads.md`.

- **Ingest is outside the engine.** A harness hook starts `gitmaster thread hook`, a one-shot process that redacts, normalizes and writes one SQLite transaction, then exits with status 0 (D124). It has no `OperationId` and is not a `Command`: it writes only GitMaster's own database and never touches the repository. The window learns of new data from a watcher on the database file.
- **Reads and edits are commands.** `ListThreads`, `SearchThreads`, `ReadThread` and the curation commands go through the engine like any other, so the UI and MCP share them (D08). An agent actor gets reads only.
- **Ports.** `ThreadStore` (SQLite adapter) and one `HarnessAdapter` per harness (payload parsing, session-log reading); the Claude Code plugin is source in the `git-master-ai` submodule, `ai/plugins/claude-code/`, not generated (D150, D194), which map the harness's shapes to canonical events (D129); nothing past the adapter knows which harness it was. The pure pieces (`redact`, `query`, normalization) hold the logic and are tested without SQLite or a window.
- **Redaction is a pure function in front of every write** (D121). Nothing downstream, including the spool and the log, ever sees unredacted text.

## Persistence

- Workflow files: outside the repository, in `~/.config/gitmaster/workflows/` (under `$XDG_CONFIG_HOME` when set) and in the directories of `gitmaster.workflowPath`. Nothing is read or written in the working tree (D61, revised by D98). The file holds only the definition (D79): nothing about execution, such as `enabled`, last run or approvals.
- Per repository, in `state.db` (in the Git common directory): runs, their journal (one entry per `ctx` call, plus outputs), and the retention rules below. It follows the repository and works with worktrees. The journal is history for the Runs view. It is not a recovery mechanism (D100).
- Per user, in `~/.local/share/gitmaster/gitmaster.db` (the app data directory): the review of each script version by its hash, with the automatic approval flag that WF3 uses. Enabling a workflow per repository is not stored yet (WF3).
- SQLite via `rusqlite` with bundled SQLite: compiled into the binary, no external dependency.
- Retention: 200 runs per workflow and per repository, pruned when a run starts. Outputs larger than 64 KiB are stored as hash, size and the first 4 KiB.
- Stack parents: git config of the repository.
- Operation log, review comments: SQLite in the Git common directory (`git rev-parse --git-common-dir`), in `gitmaster/state.db`. Works with worktrees.
- Snapshots: private refs inside `.git`.
- Threads: `threads.db` in the same directory (WAL, FTS5, `0600`), written by the hook process and by the engine, with a spool for events that cannot be written at once (D122, D124).
- The operation log is the audit trail and the base of undo (through ref snapshots). **It is not event sourcing**: rebuilding state by replay is a trap, because Git changes outside the app.
- The run journal of a workflow is the record the engine keeps of its own runs, not a reconstruction of the repository's state. Git remains the source of truth, and each effect revalidates fingerprints (D66).

## Tests

- Properties (`proptest`): staged plus unstaged equals the total change; restack is idempotent; a plan is invalidated when a ref changes.
- Determinism: the same sequence of commands on the same repository produces the same operation log.
- Core tested with fakes of the ports, without GPUI (fast test cycles).
- Workflow property: same workflow + same repository state + same inputs → same commands (D09, D66).
- Property: the transition function is deterministic and every run terminates (D78).
- Crash test: kill the engine between "entering X" and the outcome, restart, and check the handling of `interrupted` (D80).

## Crate structure (D27)

The workspace has `gm-schema`, `gm-core` (with ports and fakes), `gm-ui`, `gm-mcp` and the `gitmaster` binary. Extract `gm-git`, `gm-hooks` and `gm-store` when the core grows heavy. The core/UI split matters from day one: GPUI is heavy to compile, and the core must compile and test without it.

## Technical risks

- **Editable, virtualized text in GPUI:** needed for the conflict resolver's result panel and for the diff. Evaluate third-party libraries and their licenses (Q7).
- **In-memory rebase in libgit2:** confirm that it covers the case (Q6).
- **Status in a monorepo:** validate the subprocess approach with fsmonitor.
- **Graph lane layout:** test on a real large repository.

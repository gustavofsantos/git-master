# Workflow scripts: engineering design

> The user-facing documentation for workflows is in `docs/workflows/` and `docs/reference/workflow-api.md`. This file records the engineering rationale: why the design has this shape, and the rules the code must keep.

Last updated: 2026-10-22. The status of each decision is in `design/decisions.md`. D98 to D102 were decided by the author on 2026-10-22, in a planning session. Details marked "to be defined" depend on spike S-D or on the slice that builds them.

This file replaces, for what is not built yet, the TOML format and state machine sections of the earlier workflow design: the TOML workflow specification, available in git history at commit `5f88889` (`docs/10-workflows.md`). Those sections covered the format, the state machine, the expression language and validation. The rest of the earlier design (triggers, phases, actor and levels, concurrency, reconciliation, stacks as workflows, MCP) still holds, with the changes listed here. See [The earlier TOML design](#the-earlier-toml-design).

## Central idea (D98)

A workflow is a **Lua script** in a text file. It runs inside a sandbox embedded in the binary. The script receives a `ctx` object, which is the only way it can act, and an `input` value, which is only data.

The idea comes from durable execution systems such as Temporal: the flow is code in a programming language, and actions with effects cross a boundary that the host controls. The difference from Temporal is persistence: here, execution is ephemeral (D100).

Why replace the state machine of D78:

- It had grown a programming language: an expression evaluator, `limit`, `for_each`, `wait`, `set`, and a large validator to contain all of it.
- Small tasks (reading the JSON of an LLM, trimming a ticket key, walking branches) are hard in `${...}` and simple in Lua.
- The old rule "without a script language, the logic goes to `exec`" pushed logic into external programs that the user would have to install. That hurts "self-contained" more than an embedded VM does.

What does not change: the workflow only composes `Command`s of the engine (D60). Actions have a level of `read`, `propose` or `apply` (D64). What the workflow did is observed by the host, not reported by the script.

## Format and location (D98, revises D61)

- One workflow per `.lua` file, in `~/.config/gitmaster/workflows/` (macOS: `Library/Application Support/gitmaster/workflows/`) and in the directories of `gitmaster.workflowPath`. Nothing is read from a repository, and nothing is written to the working tree (D73 still holds).
- Directories whose names start with a dot (such as `.api/`, below) are not read as workflows.
- The module returns a table with `meta` and `execute`:

```lua
local M = {}

M.meta = {
  name = "branch-from-ticket",
  description = "Create a branch from a ticket key",
  trigger = { on = "manual" },
  concurrency = "queue",
  inputs = { ticket = { type = "string", prompt = "Ticket key" } },
}

function M.execute(ctx, input)
  local r = ctx.exec({ "ticket-branch-name", input.inputs.ticket }, { timeout = "30s" })
  if ctx.approve("Create branch " .. r.text .. "?") then
    ctx.branch.create({ name = r.text, checkout = true })
  end
end

return M
```

- `M.meta` replaces the TOML header: `name`, `description`, `trigger` (`on`, `match`, `key`), `concurrency` and `inputs`. It is read on load, in a sandbox without `ctx`, so the workflow and its triggers can be listed without running any logic.
- `input` gathers `inputs` (the user's values), `trigger` (the trigger payload) and `repo` (a read-only snapshot: `root`, `branch`, `head`, `staged_diff`). A large value comes as a file path, as in hooks.
- The file holds only the definition. Enablement per repository and approvals stay outside it, as in D79.

### What `execute` returns, by phase

| Phase | Contract |
|---|---|
| `manual` and events | The return value is ignored. Finishing without an error is `done`. |
| `before.<operation>` (gate) | `ctx.block(reason)` ends the run as `blocked` and vetoes the operation. An error also vetoes it (fail-closed). |
| `transform.<field>` | The returned value is the value of the field (`nil` = no suggestion). The reconciliation of `design/hooks-transitional.md` applies: if the user changed the field, the value becomes a suggestion. The `set` action no longer exists. |
| `after.<operation>` | Observation. Failures only produce a warning. |

`ctx.fail(reason)` ends the run as `failed`, with a reason.

## The `ctx` surface (D98, D99)

`ctx` is generated from the action catalog (`catalog.rs`). Each catalog action becomes a function with the same typed signature, the same level and the same outcomes. Names follow `ctx.<domain>.<verb>`. They are regular and predictable, because agents will write scripts (D102).

| Call | Catalog action | Level |
|---|---|---|
| `ctx.notify(text)` | `notify` | read |
| `ctx.approve(summary)` → `true`/`false` | `approve` | pauses for the human |
| `ctx.exec(argv, opts)` | `exec` | read, untrusted output |
| `ctx.commit.suggest_message(msg)`, `ctx.stage.all()`, `ctx.commit.create(...)`, `ctx.branch.create(...)`, `ctx.branch.checkout(...)` | same | propose or apply |
| `ctx.wait(event, { timeout = "24h" })` → `resumed`/`aborted`/`timeout` | `wait` | pauses until the event |
| `ctx.stack.*`, `ctx.restack.*`, `ctx.reparent.*`, `ctx.push.lease(...)`, `ctx.fetch()` | WF4, and when available | per the catalog |
| `ctx.now()`, `ctx.random()` | n/a | deterministic for the journal |
| `ctx.block(reason)`, `ctx.fail(reason)` | end of the run | n/a |
| `ctx.try(fn)` | n/a | returns the outcome instead of raising an error |

Rules:

- **An outcome other than `done` raises an error by default** (fail-closed). A `conflict`, `stale` or `rejected` does not pass silently, as in the rule "an outcome with no transition is an error" of D78. `ctx.try(function() return ctx.restack.apply(...) end)` returns `{ outcome = "conflict", ... }` so that the script can handle it.
- Calling an unknown name in `ctx` fails with the list of valid names at that level, so that an agent can correct itself from the message (D102).
- Each `ctx` call becomes a `Command` and is an entry in the journal. The host decides approval (D101), not the script.
- `exec` still runs without a shell: `argv` is a list, and each element is one element (D74).
- The `for_each` of the TOML design becomes Lua's `for`. A loop with `limit` becomes `while`. The termination limit comes from the budgets (D99).

### Examples

Gate:

```lua
local M = {}
M.meta = { name = "block-restack-on-protected", trigger = { on = "before.restack" } }

function M.execute(ctx, input)
  local root = input.trigger.root
  if root == "main" or root == "release" then
    ctx.block("Restack on " .. root .. " is blocked")
  end
end
return M
```

Transform, with an LLM command-line tool and a retry:

```lua
local M = {}
M.meta = { name = "commit-message-with-llm", trigger = { on = "transform.commit_message" } }

function M.execute(ctx, input)
  for _ = 1, 3 do
    local r = ctx.exec({ "llm-cli", "-p", "Write a conventional commit message for this diff. Reply with the message only." },
                       { stdin = input.repo.staged_diff, timeout = "60s" })
    if #r.text > 0 then return r.text end
  end
end

return M
```

Restack with a wait for a conflict:

```lua
local M = {}
M.meta = { name = "restack-children-on-parent-move", trigger = { on = "stack.parent_moved" }, concurrency = "replace" }

function M.execute(ctx, input)
  local plan = ctx.restack.plan({ root = input.trigger.parent })
  if plan.branch_count == 0 then return end
  local r = ctx.try(function() return ctx.restack.apply({ plan_id = plan.plan_id }) end)
  if r.outcome == "conflict" then
    local w = ctx.wait("restack.resumed", { timeout = "24h" })
    if w.outcome ~= "resumed" then return ctx.fail("conflict not resolved") end
  elseif r.outcome ~= "done" then
    return ctx.fail("restack " .. r.outcome)
  end
  ctx.notify("Restacked " .. plan.branch_count .. " branches on " .. input.trigger.parent)
end
return M
```

## Sandbox (D99)

**Invariant:** the script is untrusted logic, and the host is the only actor. The script has no authority of its own. Everything it does goes through `ctx`.

- Engine: a Lua VM compiled into the binary (self-contained). The engine is **Luau via `mlua`** (sandbox mode, interrupt hook, memory limit, hardened for untrusted code), chosen by spike S-D (D104, `spikes/script-engine/FINDINGS.md`). The alternatives measured were Lua 5.4 (needs a manual allow list and four guards) and Rhai (pure Rust, but not Lua).
- Removed from the environment: `io`, `os`, `debug`, `package`/`require`, `load`/`loadstring`/`dofile`, and `string.dump`. The sources of nondeterminism (`os.time`, `os.clock`, `math.random`) are removed too. `ctx.now()` and `ctx.random()` replace them, and they pass through the journal. The global environment is read-only and new on each run, so that one run does not affect the next.
- **Budgets per run:** instructions, total wall-clock time, memory, string size, stack depth, and the number of `ctx` calls. The number of calls replaces the 1000-transition ceiling of D78. Exceeding a budget ends the run as `failed`, with the reason. The values are to be defined in the spike.
- **Cancellation:** the VM's interrupt hook checks a signal. A blocked call (`approve`, `wait`, `exec`) is woken by the cancellation, and `exec` kills its process (D65).
- **Journal:** the host records each `ctx` call: sequence, function, resolved arguments, result, outcome and duration. The Runs tab (D84) is built only from it. What the script writes with `ctx.notify` or `ctx.log` appears marked as "from the script".
- **Known limit:** the VM runs in the same process. Its security is that of its C or C++ implementation. A separate process for the script would harden this, and it is outside v1. `exec` is, by design, an exit to programs the user owns. The sandbox protects the app and the repository from the script's logic, not from a program that the script itself calls (D101 covers that case).

## Ephemeral execution (D100)

- Each run runs in its own thread, outside the UI. `ctx.approve` and `ctx.wait` block that thread on a channel. No coroutine crosses host calls.
- Closing the app ends the runs in progress. At the next opening, a run that did not finish is marked `interrupted`. An `apply` that started and has no result in the journal shows as interrupted, and it never runs again (D80, without the resume).
- Waits and `timeout` deadlines live in memory. `workflow_wait` and `workflow_pending` leave SQLite. The `concurrency` queues (D65) also stay in memory.
- `state.db` stores the journal: the history of what ran, used by the Runs tab. Retention continues (200 runs per workflow and repository, with large outputs cut). The database is no longer a recovery mechanism.
- **The door stays open for Temporal-style durable execution** (Q23). The sandbox already removes nondeterminism, `ctx` is the only effect boundary, and the journal already records `(seq, function, arguments, result)`. Resuming would mean re-running the script from the start, feeding it the recorded results up to the end of the journal, and continuing live from there. Versioning of a script edited while runs are in progress, and persistent signals and timers, are left for that day.

## Approval and trust (D101, revises D64)

- Each version of a script is identified by the hash of its text. A hash that the human has not reviewed yet is **unreviewed**.
- On the first run of an unreviewed hash (manual or by event), the app shows the script and asks "Run this version?". Confirming marks the hash as reviewed. That is the moment when the human can grant automatic approval of event-driven `apply` actions: `approval = "auto"` becomes a UI choice, tied to the hash. Any edit, by the human or by an agent, returns the hash to "unreviewed". This refines D64(a): a human who presses Run on a script that an agent has just written has not seen what it does.
- Running a reviewed hash: `apply` actions follow D64 (a) to (c). A manual run executes them. An event pauses without automatic approval. An agent (MCP) always waits. `push.lease` always asks (D64(d)).
- **Rule D64(e) is replaced by a coarse rule:** after any `ctx.exec` in a run, every later `apply` action of that run asks for approval, and the dialog shows the resolved arguments. It is not possible to know statically where a value came from, so the human approves the concrete command.
- Reviewing a script approves the `exec` calls it contains. A run by an event or by an agent can only call `exec` if the hash is reviewed.

## Agents write scripts, outside MCP (D102)

The MCP keeps `workflow_list` and `workflow_run` (D71, WF5). There is no tool to create or edit scripts over MCP. An agent (for example, Claude Code) writes the `.lua` file into the user's workflows directory, like any text file. The hash-based trust rule (D101) makes sure the human sees the script before its first run. For the agent to get the connection to the app right, it needs current documentation and a way to test:

**Generated documentation.** The app writes to `<workflows directory>/.api/` at each start where the version has changed:

- `API.md`: the reference for `ctx`, generated from the catalog (signatures, levels, outcomes), plus the format of `M.meta`, the phases and the examples of this document.
- The type file to complete in the editor (for example `ctx.d.luau`; the format depends on the engine).
- Optionally, a short `SKILL.md` for agents that use skills.

Generating from the catalog keeps the documentation from drifting away from the code. The directory is in the user's config, not in a repository.

**Testing without the UI.** Three one-off subcommands, as `gitmaster mcp` already is (`design/architecture.md`, rule 7). They have no resident process and no state, so they are not a headless mode or a server (D02). This reading of D02 is the author's, and it is recorded in D102.

| Subcommand | Does |
|---|---|
| `gitmaster workflow check <file>` | Compiles in the sandbox, validates `M.meta`, and checks the trigger and the inputs. Output `file:line: message` (and `--json`). Non-zero exit code on error. |
| `gitmaster workflow dry-run <file> [--input k=v] [--repo <path>]` | Runs the script with a simulation `ctx`: real reads. `propose`, `apply` and `exec` are only recorded, with results of the same shape (D85, Q21: `exec` never runs). Prints the list of calls. What comes after a simulated value is an estimate, and it is marked as one. |
| `gitmaster workflow test <file> [fixture]` | Runs against a fake `ctx` with answers combined in a fixture (Lua). Checks the sequence of calls and the returned value. The fake `ctx` is the same one the `gm-core` tests use. |

Real effects only run from the UI, with the human as the actor. The CLI never does `apply`.

## Interface (revises D76)

The Workflows page stays: a list on the left, the detail on the right, with the **Script** and **Runs** tabs. The Flow tab (graph, inspector, structural editing) goes away.

- **Script:** the text of the file in the app's editor, with line diagnostics from `check` as you type, and a notice that saving makes the hash "unreviewed". Saving stays atomic, and it refuses when the file changed on disk. Deleting moves the file to the trash. Syntax highlighting uses the tree-sitter grammar of D92 (for Lua), if N4 already provides it.
- **Runs:** the view of D84, now with one entry per `ctx` call (function, resolved arguments, result, outcome, duration), built from the journal.
- **Dry run:** the same mechanism as the subcommand, on the text open in the editor.
- **Review:** the "Run this version?" dialog of D101.

## What survives from WF0 to WF2, and what goes

| Kept (adapted) | Removed |
|---|---|
| `store` (SQLite, retention; no waits or queues), `driver` (becomes the executor of `ctx`), `exec`, `loader` (`.lua` files, `gitmaster.workflowPath`), `workflow_session`, `catalog` (becomes the source of `ctx`), `templates` (rewritten in Lua), runs view, workflow commands and events, palette, approval | `model`, `parse`, `expr`, `validate`, `machine`, `edit`, `dry_run` (about 6.6 thousand lines in `gm-core`), `flow_graph`, `flow_graph_view`, `flow_inspector`, the Flow tab, the `.toml` fixtures, the `w-flow-editor` scenario |

There is no support for both formats at the same time: two interpreters would double the surface. After the switch, a `.toml` file in the workflows directory is ignored, with a visible warning. If the author has real TOML workflows, they are converted by hand once. The built-in templates are rewritten in Lua.

## The earlier TOML design

The TOML design was built in WF0 to WF2. It is replaced by D98. Its format, state machine, expression language and validation rules are no longer the design. Their full specification is in git history only, at commit `5f88889` (`docs/10-workflows.md`). The TOML engine is to be removed in WS2.

## Kept from the earlier design

The parts below of the earlier design still hold, with the changes noted.

### Triggers (D67, revised by D75 and D78)

| Trigger | Fires when |
|---|---|
| `manual` | the user chooses the workflow in the palette (or a key, if it declares one) |
| `repo.opened` | the repository opens |
| `refs.changed` | the watcher sees a change of ref (can be filtered by branch) |
| `commit.created`, `checkout.done`, `fetch.done`, `restack.done` | the operation finishes |
| `before.<operation>` | before the operation; it is a gate, and it vetoes with `ctx.block` (D75, D78) |
| `transform.<field>` | the value of a field is being built, such as `transform.commit_message`; the script's returned value is the field's value (D75, D78) |
| `after.<operation>` | after the operation; it is an observation, and failures only produce a warning (D75) |
| `stack.parent_moved` | derived: the tip of the parent differs from `stackParentBase` (`design/stacks-restack.md`) |
| `stack.parent_merged` | derived: the parent is gone or is already in `main` (`design/stacks-restack.md`) |
| `composer.opened` | the commit composer opens |

`match` uses the hook syntax (`branch:feat/*`, `design/hooks-transitional.md`). There is no time-based trigger in v1 (Q17, resolved).

Events that a run produces carry the ID of that run and never trigger a workflow. There are no cascades.

### Phases (D70, revised by D75, D78 and D98)

Workflows replace the hooks of GitMaster: the workflow model does the job of the hooks, and more. The extension points become trigger phases over an operation:

- `before.<operation>` (gate): the operation waits. The wait is asynchronous and pending, and it never blocks the UI (D05). The veto happens when the run calls `ctx.block(reason)`, and the reason is the message. (In the TOML design this was the `end.blocked` outcome.) A `before` run that ends in an error also vetoes the operation (fail-closed, as today's gates do).
- `transform.<field>` (transform): the value that the script returns is the value of the field (for example `transform.commit_message`). (In the TOML design this was the last `set` action.) Several transform workflows run in sequence, ordered by file name. The reconciliation of `design/hooks-transitional.md` applies. If the user changed the field meanwhile, the value becomes a suggestion, and it does not overwrite what the user typed.
- `after.<operation>` (observe): runs after the operation. Failures only produce a warning.

Initial operations: `commit`, `restack`, `branch_create` (`transform.branch_name`) and `conflict_resolve` (`transform.conflict_hunk`).

Rules and fixed points:

- Native Git hooks stay with Git (D18, D29), unchanged.
- The correlation ID of D06 becomes the run ID. The stdout response protocol of D07 applies inside `exec`.
- The hooks `pre-restack` and `post-restack` (`design/stacks-restack.md`) become `before.restack` and `after.restack`.
- **Migration.** The `prepare-commit` hook of slice 2 (in code, `gm-core/src/hook`) keeps working until the workflow `transform.commit_message` reaches parity, in WF3. After that, GitMaster stops reading the `[gitmaster "<hook>"]` sections of `.gitconfig`. A single action, "Convert hooks to workflows", writes the equivalent files into a user workflows directory.
- D17 (repository hooks require approval) has no effect with D73 after WF3. D04, D16 and D30 are superseded by D75 after WF3 (see `design/decisions.md`).

### Actor, levels and approval (D64, proposed; revised by D101)

Each run has the actor `workflow:<name>`, and it records who started it: `human`, `event:<name>` or `agent:<client>`. Each action has a level of effect, as in D20: `read`, `propose` or `apply`.

- **(a)** Manual run by the human: `apply` actions run. The human pressed Run and saw the list of steps.
- **(b)** Run by an event: `apply` actions pause in `AwaitingApproval`, unless the user granted automatic approval in the UI. Under D101, that grant is offered when the user reviews the hash, and it is tied to the file's `definition_hash`. Any edit changes the hash, and the approval stops applying (D79).
- **(c)** Run by the agent (MCP): `apply` actions always wait for the human. No override.
- **(d)** `push.lease` always asks, in every mode (D15).
- **(e)** Replaced by the coarse rule of D101: after any `ctx.exec` in a run, every later `apply` action of that run asks for approval, and the dialog shows the resolved arguments.

The `approve` action pauses with a summary for the human, in any mode. Its outcomes are `approved` and `rejected`.

### Concurrency

| `concurrency` | Behavior |
|---|---|
| `queue` (default) | at most one pending run; triggers merge into the one pending entry, which is held in memory (D100) |
| `replace` | cancels the running execution if it has not reached an `apply` action; otherwise it queues |
| `skip` | discards the trigger while an execution is active |

Exclusion comes from one active run per workflow, held in memory (D100). It does not come from a database transaction.

### Reconciliation (D66) and determinism (D09)

Each action records in the journal the fingerprint of its target: the SHAs of the refs, the index tree, the message hash, as hooks do (`design/hooks-transitional.md`). If the target changed since the inputs were computed, the action returns the outcome `stale`. The run fails with that outcome, unless the script handles it with `ctx.try`. Plans keep `plan_id` tied to the SHAs (D14).

Determinism (D09): the same script, with the same inputs and the same recorded results of earlier calls, produces the same sequence of `ctx` calls and the same `Command`s. `ctx.now()` and `ctx.random()` are journaled, and the sandbox removes the other sources of nondeterminism (D99).

### Journal in SQLite (D79, D80, adapted to D100)

#### Separation

The script file holds only the definition. No execution field (no `enabled`, no last run, no approvals) goes into the file. It is what is worth keeping in dotfiles.

Everything about execution is stored in SQLite, through `rusqlite` with SQLite bundled and compiled into the binary, to keep the app self-contained. There are two places, by scope:

- **Per repository**: the `state.db` that already exists in the Git common directory (`.git/gitmaster/state.db`, `design/architecture.md`), next to the operation log. It travels with the repository and works with worktrees. It stores the run journal. Waits and queues are in memory (D100).
- **Per user**: `~/.local/share/gitmaster/gitmaster.db` (the app's data directory). It stores which workflow is enabled in which repository, the automatic approvals by definition hash, and the cache of validated definitions.

#### Tables

Outline, with English names. It adapts the earlier sketch to D100. The names are to be settled in the slice that builds them. `workflow_call` is a placeholder name for the journal.

```text
workflow_run(id, workflow_name, definition_hash, definition_text, trigger_json,
             started_by, status, started_at, ended_at, end_reason)
workflow_call(run_id, seq, fn, args_json, result_json, outcome, started_at, duration)  -- append-only: the journal

-- per user
workflow_enabled(repo_common_dir, workflow_name)
workflow_approval(definition_hash, granted_at)
```

Changes from the earlier sketch:

- `current_state`, `workflow_transition`, `workflow_state_output`, `workflow_wait` and `workflow_pending` are removed. There are no states to store, and waits and queues are in memory (D100).
- `workflow_approval` loses `state_id`. Approval covers the hash (D101), not a state.

#### What it enables (D80, adapted to D100)

Three points of the earlier design do not apply to scripts: the pure transition function (there is no state machine), recovery by replaying the journal after a crash (under D100 the journal is history only), and waits and queues as rows (they are in memory). The points that still hold:

1. **Each run keeps the version it started with.** `definition_text` is saved at the start. Editing the file during a run does not change the run in progress. The Runs tab shows exactly what ran (observed, not reported).
2. **Approvals and enablement do not need to write the file.** The automatic approval is a row keyed by `definition_hash`. Any edit changes the hash, and the approval simply stops matching (D64(b), with no extra bookkeeping).
3. **The journal is the run history.** The host writes each `ctx` call to the journal (see Sandbox). The Runs tab (D84) is a query over it. Retention: the last 200 runs per workflow and per repository (configurable later, in Settings), pruned at the start of each run. Outputs larger than 64 KiB are kept as a hash, a size and the first 4 KiB. Diffs and LLM texts are not kept whole. Deleting a workflow keeps its runs until they are pruned.
4. **Dry run stays read-only.** It does real reads, and `propose`, `apply` and `exec` are only recorded (D76, D85, Q21).

Caveat to record: this is a **log of the execution**, kept by the engine. It is not event sourcing of the repository state. `design/architecture.md` warns against that, and the warning still holds. Git is still the source of truth, and every effect revalidates the fingerprints before it acts (D66).

### Stacks as workflows (D68, proposed)

The engine exposes the primitives as `Command`s: `stack.tree`, `stack.stale`, `restack.plan`, `restack.apply`, `restack.undo`, `reparent.plan`, `reparent.apply` and `push.lease`. It emits `stack.parent_moved` and `stack.parent_merged`.

Automatic stack update is not code. It comes as three built-in templates, off by default:

- "Restack children when parent moves"
- "Restack stale stacks after fetch"
- "Reparent to grandparent when parent is merged"

A workflow that calls `restack.apply` shows the same interactive flow as `design/stacks-restack.md`. In automatic runs, a conflict leads the run to a wait (`ctx.wait`), only for that run and the affected subtree (D14), and it notifies. It never resolves on its own.

### MCP (D71)

`workflow_list` (read) and `workflow_run(name, inputs)`, with `apply` actions following D64(c). This is slice 7 (WF5). Writing scripts is not an MCP tool (D102).

### Action catalog (v1, grows with the slices)

| Action | Level | Outcomes | Slice |
|---|---|---|---|
| `noop` | read | `done` | WF0 |
| `notify` | read | `done` | WF1 |
| `approve` | pauses for the human | `approved \| rejected` | WF1 |
| `exec` | read (untrusted output) | `done \| failed \| timeout` | WF1 |
| `commit.suggest_message` | propose | `done \| failed` | WF1 |
| `stage.all`, `commit.create` | apply | `done \| failed \| interrupted` | WF1 |
| `branch.create`, `branch.checkout` | apply | `done \| failed \| interrupted` | WF1 |
| `fetch` | apply | `done \| failed \| interrupted` | when Fetch exists |
| `stack.tree`, `stack.stale` | read | `done \| failed` | WF4 |
| `restack.plan`, `reparent.plan` | propose | `done \| failed` | WF4 |
| `restack.apply` | apply | `done \| conflict \| stale \| failed` | WF4 |
| `reparent.apply`, `restack.undo` | apply | `done \| failed \| interrupted` | WF4 |
| `push.lease` | apply, always asks | `done \| failed \| interrupted` | WF4 |
| `wait` (`ctx.wait`) | pauses until the event | `resumed \| aborted` (+ `timeout`) | WF3 |

Notes:

- The `set` row is removed. `set` belonged to the TOML design and no longer exists (D98). The `for_each` row is removed too: Lua's `for` replaces it.
- `noop` is read and has only the outcome `done`. In the TOML design it existed so that a state could only branch. Under D98 the script branches with plain Lua.
- `block` was removed (D77, revised by D78). Under D98 the veto is `ctx.block(reason)`.
- `interrupted` is the outcome of an `apply` action that was in progress when the engine stopped, and that has no recovery of its own (D80). Actions with their own recovery, such as the restack (D14), resume through their own flow and do not produce `interrupted`.

### LLM and exec (D69, revised by D74)

The app has no built-in LLM. There is no LLM call, no HTTP, and the app stores no API key (Q16). An LLM is called through `exec`, which runs a command-line program that the user already has, such as an LLM CLI or a script of their own. The app stays self-contained: the only required dependency is `git`.

Rules:

- `argv` is a list run directly, **never by a shell**. Each interpolated value becomes exactly one element of `argv`, and it is not split again. Repository content cannot inject flags or commands.
- `stdin` is an optional text template. Large values, such as the diff, go by `stdin`, not by `argv`.
- Output: the stdout text (without spaces at the edges) and the exit code. These are the field names of the earlier design. The examples above use `r.text`. With the earlier design's option `format = "json"`, stdout must be JSON and is available as parsed output. That is the old hooks contract, now optional.
- A non-zero exit code produces the outcome `failed`, and stderr is shown. `timeout` kills the process and produces `timeout`.
- The output is untrusted (D64(e), D69, `docs/agents/mcp-server.md` and `docs/agents/local-review.md`, prompt injection). It feeds only `propose` actions, such as `commit.suggest_message`, and the human decides. In `apply` actions, D64(e) applies, as revised by D101.

## Open questions

- **Q22. Script engine (resolved by D104).** Luau via `mlua` was chosen from the S-D numbers (`spikes/script-engine/FINDINGS.md`). Luau is C++, so CI and releases need a C++ compiler.
- **Q23. Durable execution.** Re-execution from the journal, as Temporal does, is postponed by D100. Reopen it when losing runs at app close becomes a problem.

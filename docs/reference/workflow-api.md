---
title: Workflow API
description: The ctx object a workflow script calls to act, with the arguments, results, effect levels and outcomes of each call, and the sandbox and budgets a script runs under.
status: partial
order: 40
---

A workflow is a Lua script. The script receives two values: `ctx`, the only way it can act in GitMaster, and `input`, which holds data only. Each call on `ctx` runs one GitMaster command, the same kind of command the window and the MCP server run, and each call is recorded in the run journal. For the file layout and how a run starts, see [Writing workflows](../workflows/writing-workflows.md). For the triggers and the rules of each phase, see [Triggers and phases](../workflows/triggers-and-phases.md).

## The module

A workflow file returns a table with `meta` and `execute`:

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

### `M.meta`

GitMaster reads `M.meta` when it loads the file, in a sandbox without `ctx`. It uses the table to list the workflow and its triggers. No script logic runs at that point.

| Field | Meaning |
|---|---|
| `name` | The name of the workflow. |
| `description` | One line that says what the workflow does. |
| `trigger` | What starts a run. `on` names the trigger. `match` narrows an event trigger to branches with a glob, such as `branch:feat/*`; see [Triggers and phases](../workflows/triggers-and-phases.md). `key` gives a manual trigger an optional keyboard shortcut. The valid trigger names are listed in [Triggers and phases](../workflows/triggers-and-phases.md). |
| `concurrency` | What happens when a trigger fires while a run is active. `queue` (the default) keeps at most one pending run, and new triggers merge into it. `replace` cancels the active run if it has not reached an applying action; otherwise the new trigger waits in the queue. `skip` discards the new trigger. |
| `inputs` | Named values that you type before a manual run. Each input has a `type`, such as `"string"`, and a `prompt` that the dialog shows. |

### `input`

| Field | Holds |
|---|---|
| `input.inputs` | The values you entered for the `inputs` declared in `M.meta`, by name. |
| `input.trigger` | The payload of the trigger. Its fields depend on the trigger. The examples read `root` from a `before.restack` trigger and `parent` from a `stack.parent_moved` trigger. |
| `input.repo` | A read-only snapshot of the repository when the run starts: `root`, `branch`, `head` and `staged_diff`. `staged_diff` is the diff text; a diff over 1 MiB is cut at a line break and ends with `[diff truncated at 1 MiB]`, and `staged_diff_path` is then the path of a file that holds the whole diff. |

## Effect levels

Every call has an effect level. The level tells you what the call can change.

> **Planned.** The agent rules of the apply level are not available yet.

| Level | Meaning |
|---|---|
| read | Observes the repository or runs a program. It changes nothing. |
| propose | Prepares a result for you to accept, such as a commit message or a plan. Nothing is applied. |
| apply | Changes the repository or the remote. A manual run applies these actions after you review the script. A run started by an event waits for your approval. A run started by an agent always waits for you. See [Approvals and security](../workflows/approvals-and-security.md). |
| pauses for a human | Stops the run until you answer in the app. |
| pauses until an event | Stops the run until the named event arrives or the timeout ends. |

## Calls

Calls with named arguments take one table, with the names shown in the Arguments column, such as `ctx.branch.create({ name = "fix/login", checkout = true })`. The calls `ctx.notify`, `ctx.approve`, `ctx.exec` and `ctx.wait` take their arguments positionally, as shown.

A dash (—) in the Result column means the call returns no fields that scripts rely on.

Outcomes are the values a call can end with, as listed in the action catalog. A value other than `done` raises an error unless the call runs inside `ctx.try`. The section Outcomes and errors below gives the rules.

### Interaction

| Call | Arguments | Result | Effect level | Outcomes |
|---|---|---|---|---|
| `ctx.notify(text)` | `text`: the line the notification shows. | — | read | `done`, `failed` |
| `ctx.approve(summary)` | `summary` (optional): what you are asked to approve. | `true` when you approve, `false` when you reject. | pauses for a human | `approved`, `rejected`, `failed` |
| `ctx.wait(event, opts)` | `event`: the event to wait for. `opts.timeout`: a duration such as `"24h"`; without it the wait lasts until the event or until you cancel. | A table whose `outcome` field is `resumed`, `aborted` or `timeout`. The call never raises an error, whatever the outcome. | pauses until an event | `resumed`, `aborted`, `timeout` |

The catalog declares one wait event: `restack.resumed`. It is used after a restack conflict, so that the run can continue once the conflict is resolved. The wait ends with `resumed` when you choose **Continue restack** after resolving the conflict, and with `aborted` when you cancel that resolution. Both carry `branch`, the branch that conflicted, and `commit`, the commit that was replayed. Waits exist only while the app is open.

### Process

| Call | Arguments | Result | Effect level | Outcomes |
|---|---|---|---|---|
| `ctx.exec(argv, opts)` | `argv`: a list of strings. `opts.stdin`: text for the program's standard input. `opts.timeout`: a duration such as `"60s"`, default `30s`. `opts.format`: `"json"`, or not set. | A table with `text`, `exit_code`, `stderr` and `truncated`. With `format = "json"`, the parsed JSON is also in the result, as `json`. | read (output is untrusted) | `done`, `failed`, `timeout` |

`ctx.exec` runs a program directly, never through a shell:

- `argv` is a list. Each element is one argument. A value placed in an element stays one argument, even when it contains spaces or flags, so repository content cannot add options or commands.
- `stdin` carries large text. Send a diff as `stdin`, not inside `argv`.
- `timeout` accepts a positive whole number followed by `s`, `m` or `h`, such as `"30s"`, `"2m"` or `"1h"`. The default is `30s`. When the time runs out, the program is killed and the call ends with `timeout`.
- `text` is the standard output with the spaces at both ends removed. `exit_code` is the exit status of the program.
- A non-zero exit ends the call with `failed`. The standard error of the program is shown.
- With `format = "json"`, the standard output must be JSON. The parsed value is in the result.
- Output from a program is untrusted. A propose action, such as `ctx.commit.suggest_message`, takes it only as a suggestion, and you decide. After any `ctx.exec` in a run, every later apply action asks you for approval and shows the resolved arguments.

```lua
local r = ctx.exec({ "llm-cli", "-p", "Write a conventional commit message for this diff. Reply with the message only." },
                   { stdin = input.repo.staged_diff, timeout = "60s" })
```

### Staging and commits

| Call | Arguments | Result | Effect level | Outcomes |
|---|---|---|---|---|
| `ctx.stage.all()` | None. | — | apply | `done`, `failed`, `interrupted` |
| `ctx.commit.suggest_message({ message })` | `message`: the suggested commit message, text. Required. | — | propose | `done`, `failed` |
| `ctx.commit.create({ message })` | `message`: the commit message, text. Required. | — | apply | `done`, `failed`, `interrupted` |

### Branches

| Call | Arguments | Result | Effect level | Outcomes |
|---|---|---|---|---|
| `ctx.branch.create({ name, checkout })` | `name`: the name of the new branch, text. Required. `checkout`: `true` to check out the new branch. Default `false`. | — | apply | `done`, `failed`, `interrupted` |
| `ctx.branch.checkout({ branch })` | `branch`: the branch to check out, text. Required. | — | apply | `done`, `failed`, `interrupted` |

### Remote

> **Planned.** `ctx.fetch` and `ctx.push.lease` are not available in the app yet.

| Call | Arguments | Result | Effect level | Outcomes |
|---|---|---|---|---|
| `ctx.fetch({ remote })` | `remote`: the remote to fetch, text. Optional, default `origin`. | — | apply | `done`, `failed`, `interrupted` |
| `ctx.push.lease({ branch, remote })` | `branch`: the branch to push, text. Required. `remote`: the remote to push to, text. Optional, default `origin`. | — | apply, always asks for your approval | `done`, `failed`, `interrupted` |

### Stacks

> **Planned.** `ctx.stack.tree` and `ctx.stack.stale` are not available in the app yet.

| Call | Arguments | Result | Effect level | Outcomes |
|---|---|---|---|---|
| `ctx.stack.tree()` | — | `branches`: a list of tables with `name`, `parent` and `stale`. | read | `done`, `failed` |
| `ctx.stack.stale()` | — | `branches`: a list of the names of the branches whose parent moved. | read | `done`, `failed` |

### Restack

> **Planned.** The restack calls are not available in the app yet.

| Call | Arguments | Result | Effect level | Outcomes |
|---|---|---|---|---|
| `ctx.restack.plan({ root })` | `root`: the branch at the top of the subtree to restack, text. Required. | `plan_id` and `branch_count`. | propose | `done`, `failed` |
| `ctx.restack.apply({ plan_id })` | `plan_id`: the id returned by `ctx.restack.plan`, text. Required. | — The outcome is available through `ctx.try`. | apply | `done`, `conflict`, `stale`, `failed` |
| `ctx.restack.undo({ plan_id })` | `plan_id`: the id of the restack to undo, text. Required. | — | apply | `done`, `failed`, `interrupted` |

A plan's `plan_id` is tied to the commits of the branches when the plan was made. If a branch has moved since then, `ctx.restack.apply` ends with `stale`. A restack conflict ends `ctx.restack.apply` with `conflict`. `ctx.restack.apply` has no `interrupted` outcome, because restack has its own recovery steps.

### Reparent

> **Planned.** The reparent calls are not available in the app yet.

| Call | Arguments | Result | Effect level | Outcomes |
|---|---|---|---|---|
| `ctx.reparent.plan({ branch, parent })` | `branch`: the branch to move, text. Required. `parent`: the branch that becomes its parent, text. Required. | `plan_id` and `branch_count`. | propose | `done`, `failed` |
| `ctx.reparent.apply({ plan_id })` | `plan_id`: the id returned by `ctx.reparent.plan`, text. Required. | — | apply | `done`, `failed`, `interrupted` |

### Helpers

These calls do not act on the repository, so they have no effect level or outcome.

| Call | Arguments | Result |
|---|---|---|
| `ctx.try(fn)` | `fn`: a function that makes one call on `ctx`. | A table whose `outcome` field is the outcome of that call. |
| `ctx.fail(reason)` | `reason`: text. | Ends the run as `failed` with the reason. |
| `ctx.block(reason)` | `reason`: text. | Ends the run as `blocked` with the reason. In a `before.<operation>` phase, it vetoes the operation. |
| `ctx.now()` | None. | The time of the call, as whole milliseconds since 1 January 1970 UTC. The value is recorded in the run journal. |
| `ctx.random()` | None. | A number from 0 up to, but not including, 1. A run draws from a sequence seeded for that run, and each value is recorded in the run journal. |

## Outcomes and errors

> **Planned.** The example below uses the restack calls, which are not available yet.

- A call that ends with an outcome other than `done` raises an error. The run stops there. An outcome such as `conflict` or `stale` never passes silently.
- `ctx.try(fn)` returns the outcome instead of raising. Use it when the script handles the outcome itself:

```lua
local plan = ctx.restack.plan({ root = input.trigger.parent })
local r = ctx.try(function() return ctx.restack.apply({ plan_id = plan.plan_id }) end)
if r.outcome == "conflict" then
  local w = ctx.wait("restack.resumed", { timeout = "24h" })
  if w.outcome ~= "resumed" then return ctx.fail("conflict not resolved") end
elseif r.outcome ~= "done" then
  return ctx.fail("restack " .. r.outcome)
end
```

- An applying action that was in progress when the app stopped, and has no recorded result, ends with `interrupted`. It never runs again.
- A call to a name that is not in `ctx` fails with the list of valid names at that level, so the caller can correct the name from the message.
- Every call is written to the run journal, with its sequence number, function, resolved arguments, result, outcome and duration. Approval decisions belong to GitMaster. The script does not decide them.

## Return value by phase

What `M.execute` returns depends on the trigger.

| Trigger | Return value | Effect |
|---|---|---|
| `manual` and events | Ignored. | Finishing without error ends the run as `done`. |
| `before.<operation>` | Ignored. | `ctx.block(reason)` ends the run as `blocked` and vetoes the operation. An error also vetoes it. |
| `transform.<field>` | The new value of the field. `nil` gives no suggestion. | The returned value becomes the field's value. If you changed the field in the meantime, the value becomes a suggestion and does not overwrite what you wrote. |
| `after.<operation>` | Ignored. | The run only observes the operation. A failure shows a warning. |

## Sandbox

The script runs in a Lua virtual machine that is part of the app.

- These libraries and functions are removed: `io`, `os`, `debug`, `package`, `require`, `load`, `loadstring`, `dofile` and `string.dump`.
- The sources of nondeterminism are removed as well: `os.time`, `os.clock` and `math.random`. Use `ctx.now()` and `ctx.random()` instead. Both are recorded in the journal.
- The global environment is read-only and is created new for each run. One run cannot change what the next run sees.
- The script has no authority of its own. Everything it does passes through `ctx`.
- The sandbox protects GitMaster and your repository from the logic of the script. It does not protect you from a program that the script launches. `ctx.exec` runs programs that you choose, and the approval rules apply to the calls that follow it. See [Approvals and security](../workflows/approvals-and-security.md).

Cancelling a run stops the script. A call that is waiting, such as `ctx.approve`, `ctx.wait` or `ctx.exec`, ends at once, and cancelling `ctx.exec` kills the program it started.

Waits and their timeouts are kept in memory. Closing the app ends a run in progress, and the next start shows that run as `interrupted`.

## Budgets

Each run has budgets for:

- instructions executed,
- total elapsed time,
- memory,
- string size,
- call depth,
- the number of `ctx` calls.

A run that exceeds a budget ends as `failed`, with the reason.

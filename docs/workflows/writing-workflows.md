---
title: Writing workflows
description: A tutorial on the shape of a workflow script, its metadata, the values it receives, the calls it makes through ctx, and how to check and test it before a run.
status: partial
order: 20
---

This tutorial takes a workflow from the first line to a tested file. Read [Workflows overview](overview.md) first for the model. Each worked example is a complete file that you save into your workflows folder.

## The shape of a module

Every workflow file is a Lua module with the same parts:

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
  -- the work of the workflow goes here
end

return M
```

- `local M = {}` creates the module table.
- `M.meta` describes the workflow. GitMaster reads it when it loads the file, without running `execute`, so keep logic out of it.
- `function M.execute(ctx, input)` is the code that runs. GitMaster calls it with the `ctx` object and the `input` values.
- `return M` returns the module to GitMaster.

## The metadata

| Field | Meaning |
|---|---|
| `name` | The name of the workflow. |
| `description` | One line that says what the workflow does. |
| `trigger` | When the workflow runs. `on` names the trigger, such as `manual`, a phase such as `before.restack`, or an event such as `stack.parent_moved`. `match` narrows the trigger with the hook syntax, for example `branch:feat/*`. `key` adds a keyboard shortcut to a manual workflow. |
| `concurrency` | What happens when the trigger fires during a run: `queue`, `replace` or `skip`. The default is `queue`. |
| `inputs` | Values the user enters before the run. Each input has a `type` and a `prompt`, which the dialog shows. |

> **Planned.** Only the `manual` trigger runs. The `match` filter and `concurrency` are not available yet, and so are the phase and event triggers.

The `concurrency` values work this way:

| Value | When the trigger fires during a run |
|---|---|
| `queue` | One run waits behind the active run. Further triggers merge into that waiting run. |
| `replace` | The active run is cancelled if it has not reached an apply action yet. Otherwise the new run waits in the queue. |
| `skip` | The trigger is discarded. |

For the full list of trigger names, see [Triggers and phases](triggers-and-phases.md).

## What `input` contains

- `input.inputs` holds the values the user entered, by name. In the example above, the ticket key is `input.inputs.ticket`.
- `input.repo` is a read-only snapshot of the repository, with `root`, `branch`, `head` and `staged_diff`.
- `input.trigger` holds the payload of the trigger. In a stack event, `input.trigger.parent` names the parent branch.

> **Planned.** The trigger payload comes with the triggers other than `manual`, which are not available yet.

`staged_diff` is the diff text. A diff over 1 MiB is cut at a line break and ends with `[diff truncated at 1 MiB]`, and `input.repo.staged_diff_path` is then the path of a file that holds the whole diff.

## What `execute` returns

What the function returns depends on the phase it runs in:

| Phase | What the return value means |
|---|---|
| `manual` and events | Ignored. Finishing without an error means the run is done. |
| `before.<operation>` (gate) | `ctx.block(reason)` ends the run as blocked and stops the operation. An error also stops the operation. |
| `transform.<field>` | The returned value becomes the value of the field. Return `nil` for no suggestion. |
| `after.<operation>` | Observed only. A failure produces a warning and nothing else. |

> **Planned.** The `before`, `transform` and `after` phases are not available yet. A manual run ignores the return value.

`ctx.fail(reason)` ends any run as failed, with the reason.

If you change a field while a `transform` runs, GitMaster does not overwrite your text. The returned value becomes a suggestion instead.

## Calls to `ctx`

Each call to `ctx` is an action of GitMaster. The action returns a result with an outcome. The outcome `done` means the action worked. Every other outcome, such as `conflict`, `stale`, `rejected` or `timeout`, raises an error. An error ends the run unless the script handles it.

`ctx.try(fn)` runs a function and returns the outcome instead of raising an error:

```lua
local r = ctx.try(function()
  return ctx.restack.apply({ plan_id = plan.plan_id })
end)
if r.outcome == "conflict" then
  -- the script decides what happens next
end
```

These rules also apply:

- `ctx.notify(text)` writes a message into the run's record, marked as coming from the script.
- A call to a `ctx` function that does not exist is an error. The message lists the valid names at that level, so you can correct the call from the message.
- Each `ctx` call is one entry in the run's record, with its resolved arguments, its result and its outcome.

## Running programs with `ctx.exec`

`ctx.exec(argv, options)` runs a program that you have installed on your computer.

- `argv` is a list. The first element is the program, and the rest are its arguments. GitMaster never passes the list to a shell. Each element stays one argument, even when it contains spaces, quotes or semicolons, so text from the repository cannot add flags or commands.
- `stdin` sends text to the program's standard input. Use it for large text, such as a diff. Do not put large text in `argv`.
- `timeout` sets a limit, such as `"30s"`. When the limit passes, GitMaster stops the process and the outcome is `timeout`.
- The result has `text`, which is the standard output with the surrounding whitespace removed, and `exit_code`. A non-zero exit code gives the outcome `failed`.

Output from a program is untrusted. A program can print anything, so treat `text` as data. Use it for suggestions that a person accepts, and check it before you act on it.

```lua
local r = ctx.exec({ "ticket-branch-name", input.inputs.ticket }, { timeout = "30s" })
```

The sandbox limits the script. It does not limit the program that `exec` starts. See [Approvals and security](approvals-and-security.md).

## Approvals from the script

`ctx.approve(summary)` shows the summary to you and pauses the run until you answer. It returns `true` or `false`.

```lua
if ctx.approve("Create branch " .. r.text .. "?") then
  ctx.branch.create({ name = r.text, checkout = true })
end
```

The script can ask for approval, but GitMaster decides what needs approval, and the script cannot skip those rules. After any `ctx.exec` in a run, every later apply action asks you again, and the dialog shows the resolved arguments. See [Approvals and security](approvals-and-security.md).

## Waiting for an event

`ctx.wait(event, { timeout = "24h" })` pauses the run until the named event happens. It returns `resumed`, `aborted` or `timeout`. A wait lasts only while GitMaster is open. Closing the app ends the run.

> **Planned.** No event resumes a wait yet. A wait ends only at its timeout or when you cancel the run.

## Loops and control flow

Conditions, loops and functions are ordinary Lua. A loop must end. GitMaster stops any run that exceeds its limits, so a loop that never ends is stopped rather than left running. The transform example below uses a bounded retry loop.

## Limits and what the sandbox removes

Each run has limits on:

- the number of instructions;
- total time;
- memory;
- string size;
- stack depth;
- the number of `ctx` calls.

A run that exceeds a limit ends as failed, with the reason. Cancel always works, even when the script is in a loop or waiting.

The sandbox removes these from the environment: `io`, `os`, `debug`, `package`, `require`, `load`, `loadstring`, `dofile` and `string.dump`. The clocks and random numbers are not available either. Use `ctx.now()` and `ctx.random()` instead. Both calls are recorded in the run.

The global environment is read-only, and each run starts with a fresh one, so nothing one run sets affects the next run.

## Worked examples

### Branch from a ticket

This is a manual workflow with one input. Run it from the command palette, with **Run Workflow…**.

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

The workflow runs a naming program with the ticket key, and the program's output becomes the branch name. It asks you to confirm the name. The branch is created and checked out only when you approve. Rejecting leaves the repository unchanged.

### Block restack on a protected branch

> **Planned.** The `before.restack` phase is not available yet, so this gate does not run.

This gate runs before every restack.

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

When the root of the restack is `main` or `release`, the script blocks the restack, and the reason is the message you see. In any other case the script finishes without blocking, and the restack goes ahead.

### Commit message from an LLM

> **Planned.** The `transform.commit_message` phase is not available yet, so this transform does not run.

This transform runs when GitMaster builds the commit message.

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

The staged diff goes to the command-line LLM on standard input. The script returns the first non-empty answer, and it tries up to three times. If every try is empty, the function returns nothing, which means no suggestion. Replace `llm-cli` with the tool you use.

### Restack children when the parent moves

> **Planned.** The `stack.parent_moved` trigger, `concurrency`, `ctx.wait` and the restack calls are not available yet, so this workflow does not run.

This event runs when the parent branch of a stack gets new commits. It uses `concurrency = "replace"`, so a newer event replaces this run if the run has not reached its apply step yet.

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

The script plans the restack and stops if no branch needs it. It applies the plan inside `ctx.try`, so a conflict does not raise an error. On a conflict, it waits up to 24 hours for the restack to be resumed. If the wait ends in anything other than `resumed`, the run fails with a reason. Any other outcome also fails the run. On success, it sends a notification.

## Check, dry-run and test while you write

Three command-line commands help you test a file without the app's UI. They run only when you start them, so they are not a server.

| Command | What it does |
|---|---|
| `gitmaster workflow check <file>` | Compiles the file, validates `M.meta`, and checks the trigger and the inputs. Each error prints as `file:line: message`. Add `--json` for machine-readable output. The exit code is non-zero on an error. |
| `gitmaster workflow dry-run <file> [--input k=v] [--repo <path>]` | Runs the script with a simulated `ctx`. Reads are real. Other actions are recorded, not run. Prints the list of calls. |
| `gitmaster workflow test <file> [fixture]` | Runs the script against a fake `ctx` that returns canned responses from a Lua fixture. Checks the sequence of calls and the returned value. |

The command line never applies a change. Real effects run only from the app, with you as the actor.

The app also writes generated reference files into a `.api` folder inside your workflows folder. They list every `ctx` function, with its arguments and outcomes. Point your editor at them to get completion for `ctx`. See [Workflow API](../reference/workflow-api.md).

## Related pages

- [Workflows overview](overview.md): the model, where files live, and the Runs tab.
- [Triggers and phases](triggers-and-phases.md): the events, operations and phases a workflow can run on.
- [Approvals and security](approvals-and-security.md): review, approvals and the sandbox.
- [Workflow API](../reference/workflow-api.md): every `ctx` function, with its arguments and outcomes.

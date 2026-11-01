---
title: Approvals and security
description: Who can start a workflow run, which actions need your approval, how script review by hash works, and the limits of the sandbox.
status: partial
order: 40
---

Workflows can change your repository, so GitMaster decides what a run may do on its own and what needs you. This page covers who starts a run, the effect level of each action, when you approve, how a script is reviewed, and what the sandbox protects. A script is untrusted logic. It has no authority of its own: GitMaster is the only actor, and every action the script takes goes through a `ctx` call that GitMaster carries out.

## Who started a run

Each run records who started it. The Runs tab shows the starter for every run.

> **Planned.** Agent runs are not available yet, because nothing starts them. The other runs are started by you or by an event.

| Starter | Shown as | Started by |
|---|---|---|
| Human | `human` | You, from the command palette, or with the workflow's key. |
| Event | `event:<name>` | A trigger, for example `event:commit.created`. |
| Agent | `agent:<client>` | An AI agent through the `workflow_run` tool of the [MCP server](../agents/mcp-server.md). `<client>` is the name of the MCP client. |

## Effect levels

Every action has one effect level. The level tells you whether the action can change anything.

> **Planned.** The `fetch`, `push.lease` and stack calls are listed by their level here. They are not available yet.

| Level | Meaning | Examples |
|---|---|---|
| `read` | Only reads state. Runs without asking. | `ctx.notify`, `ctx.stack.tree`, `ctx.exec` (its output is untrusted) |
| `propose` | Prepares a change for you to accept. Nothing is applied. | `ctx.commit.suggest_message`, `ctx.restack.plan` |
| `apply` | Changes the repository or the remote. Subject to the rules below. | `ctx.stage.all`, `ctx.commit.create`, `ctx.branch.create`, `ctx.branch.checkout`, `ctx.fetch`, `ctx.restack.apply`, `ctx.push.lease` |

`ctx.approve(summary)` pauses the run and shows you the summary. It returns `true` or `false`. It pauses in every mode.

## Rules for applying actions

> **Planned.** The agent column and the `ctx.push.lease` row are not available yet.

The table shows what happens to an `apply` action, depending on who started the run.

| Applying action | Manual run | Event run | Agent run |
|---|---|---|---|
| An `apply` action | Runs, after you review the script. | Pauses for your approval, unless you granted automatic approval for this version of the script. | Always waits for you. |
| `ctx.push.lease` | Asks. | Asks. | Asks. |
| Any `apply` action after a `ctx.exec` in the same run | Asks, and shows the resolved arguments. | Asks, and shows the resolved arguments. | Asks, and shows the resolved arguments. |

No setting overrides the agent rule. `ctx.push.lease` asks in every mode.

A script can take a value from the output of a program and pass it to a change. GitMaster cannot tell from the script where such a value came from. So after any `ctx.exec` in a run, every later applying action asks you, and the dialog shows the arguments as they resolve. You approve the concrete command, not the script's intent.

## Review by hash

> **Planned.** The review rules of agent runs are not available yet.

GitMaster identifies each version of a script by the hash of its text.

1. A version you have not reviewed is **unreviewed**.
2. The first run of an unreviewed version shows you the script and asks **Run this version?**.
3. Confirm to mark that version as reviewed. When an event starts a version you have not reviewed, no dialog interrupts you. The run waits behind a notice, *name wants to run*, with **Review**. Reviewing runs it for that event. Dismissing the notice cancels the run.
4. Any edit to the script, by you or by an agent, makes it unreviewed again. Automatic approval is tied to the hash, so an edit ends it.

Reviewing a version also approves the `ctx.exec` calls that the script contains. Event runs and agent runs call `ctx.exec` only for a reviewed version.

> **Note.** A script that an agent writes to your workflows folder is unreviewed until you review it. Its first manual run asks you before it does anything.

## The sandbox

A script runs in a Lua virtual machine embedded in GitMaster. It receives two values:

- `ctx`, the only way to act.
- `input`, which holds data only: the values you typed, the trigger payload and a read-only snapshot of the repository.

### What the sandbox removes

These are not available to a script:

- The libraries `io`, `os`, `debug`, `package` and `require`.
- The functions `load`, `loadstring`, `dofile` and `string.dump`.
- The sources of nondeterminism `os.time`, `os.clock` and `math.random`. Use `ctx.now()` and `ctx.random()` instead. Their results are recorded in the run journal, so a run can be read back exactly.

The global environment is read-only and is created new for each run, so one run cannot change what the next run sees.

### Budgets

Each run has budgets for instructions, elapsed time, memory, string size, call depth and the number of `ctx` calls. A run that exceeds a budget ends as `failed`, and the reason is shown.

### Cancellation

You can cancel a run at any time, with **Cancel Workflow Run** in the command palette. Cancelling stops the script. A call that is waiting, such as `ctx.approve` or `ctx.exec`, ends at once. Cancelling `ctx.exec` also kills the program it started. A cancelled run ends as `Cancelled`.

### Closing the app

Closing GitMaster ends the runs in progress. When GitMaster starts again:

- A run that did not finish is shown as `interrupted`.
- An applying action that started but has no recorded result is shown as `interrupted`. It never runs again.

Waits and their timeouts are kept in memory, so they do not survive a restart.

## Run journal

GitMaster records every `ctx` call in a run journal. For each call it records the sequence number, the function, the resolved arguments, the result, the outcome and the duration. The Runs tab is built only from this journal. It shows what the run did, not what the script says it did.

- Text that the script writes with `ctx.notify` is marked as coming from the script.
- Each Git command with a side effect that a run performs appears in the Git commands panel, as the same action started from the window does.
- The journal is kept for each repository, in `.git/gitmaster/state.db`. GitMaster keeps the last 200 runs for each workflow and repository. Large outputs are cut short.

## Output of programs

`ctx.exec` runs a program that you name. Its arguments are a list, and the command never goes through a shell. Each value you pass becomes exactly one argument, so repository content cannot add flags or commands.

Output from a program is untrusted. It can suggest a commit message or fill a field, and you decide what happens next. Applying actions that follow a `ctx.exec` ask you, with the resolved arguments shown.

## Limits of the sandbox

- The sandbox protects GitMaster and your repository from the logic of the script. It does not protect you from a program that the script launches. `ctx.exec` runs programs you choose, by design. That is why a run that calls it needs a reviewed script.
- The script engine runs inside the GitMaster process. Its safety depends on the implementation of the engine, as well as on the rules on this page.

See also [Triggers and phases](triggers-and-phases.md) and [Writing workflows](writing-workflows.md).

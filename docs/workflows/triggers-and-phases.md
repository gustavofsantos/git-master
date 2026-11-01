---
title: Triggers and phases
description: Which events start a workflow, how the match filter narrows them, and what the before, transform and after phases do.
status: partial
order: 30
---

A workflow runs when one of its triggers fires. A trigger is either an event in GitMaster, such as a commit being created or a branch being checked out, or your own request from the command palette. Some operations also offer three phases for a workflow: **before** the operation, where a workflow can veto it; **transform**, where a workflow fills in the value of a field; and **after** the operation, where a workflow only observes. This page lists the triggers, the `match` filter and the phases. For the script API, see [Writing workflows](writing-workflows.md) and the [Workflow API](../reference/workflow-api.md).

## Triggers

You set a trigger in the `trigger` table of `M.meta`:

```lua
M.meta = {
  name = "notify-on-feature-change",
  trigger = { on = "refs.changed", match = "branch:feat/*" },
}
```

> **Planned.** Only `manual` is available in the app. The other triggers in the table below, and the `match` filter in the example, are not available yet.

| Trigger | Fires when |
|---|---|
| `manual` | You choose the workflow in the command palette, or press its key if it declares one. |
| `repo.opened` | A repository opens. |
| `refs.changed` | The file watcher sees a Git reference change. Filter it by branch with `match`. |
| `commit.created` | A commit finishes. |
| `checkout.done` | A checkout finishes. |
| `fetch.done` | A fetch finishes. |
| `restack.done` | A restack finishes. |
| `before.<operation>` | Before the operation starts. The workflow is a gate. |
| `transform.<field>` | The value of a field is being built, such as `transform.commit_message`. |
| `after.<operation>` | After the operation finishes. The workflow only observes. |
| `stack.parent_moved` | The tip of a branch's parent no longer matches the point the branch was based on. It fires when the branch becomes stale, once per change, with `branch`, `parent`, `base` and `parent_tip` in the payload. |
| `stack.parent_merged` | The parent branch is gone, or it is already merged into `main`. It fires once when that becomes true, with `branch` and `parent` in the payload. |
| `composer.opened` | The empty message box of the commit composer gets focus while something is staged. |

The stack events compare the stacks with what GitMaster saw last. The state a repository has when it opens is the starting point, so opening a repository with a stale branch does not fire `stack.parent_moved`.

### No time-based triggers

No trigger runs on a clock or after a delay. The `timeout` of `ctx.wait` ends that wait with the outcome `timeout`. It does not start a run.

### The match filter

> **Planned.** The `match` filter is not available yet.

Use `match` to narrow a trigger to branches. The syntax is `branch:` followed by a glob pattern, where `*` matches any run of characters, `/` included. `branch:feat/*` matches the branches whose names start with `feat/`. A trigger without `match` is not filtered.

### Events from a run do not trigger workflows

> **Planned.** Event triggers are not available yet, so this rule has no effect yet.

Events that a run produces carry the ID of that run and never trigger a workflow. A commit that a workflow creates does not start a `commit.created` workflow, so a workflow cannot start another one through the events it causes. There are no cascades. For the same reason, an operation a run performs runs no phase: a commit made with `ctx.commit.create` is not gated by `before.commit` workflows, and runs no `transform` or `after` workflow. The run's own applying actions follow the rules in [Approvals and security](approvals-and-security.md).

Workflows run only in the app. The MCP server, `gitmaster mcp`, fires no trigger and runs no phase. When an agent changes a branch, the app's file watcher sees it and fires `refs.changed`.

### Debounce

> **Planned.** Debounce is not available yet, because the file watcher triggers are not.

Triggers that come from the file watcher, such as `refs.changed`, are debounced. A run starts once the changes stop arriving, not once for each file change.

## Concurrency

> **Planned.** Concurrency is not available yet. Every manual run starts when you start it.

Set `concurrency` in `M.meta`. The default is `queue`.

| Mode | When a trigger arrives while a run is active |
|---|---|
| `queue` (default) | The new run waits as the one pending run. Further triggers merge into that pending run. |
| `replace` | The active run is cancelled if it has not reached an applying action. Otherwise the new run waits as the pending run. |
| `skip` | The trigger is dropped. |

An applying action is an action at the `apply` level. See [Approvals and security](approvals-and-security.md).

## Phases

> **Planned.** The before, transform and after phases are not available yet. Only `manual` runs.

The phases apply to the operations listed in the table of operations and fields, further down this page. Each phase is a workflow whose trigger is the phase name, such as `before.restack`.

### Before: gates

A `before.<operation>` workflow is a gate. The operation stays pending while the gate runs, and the window stays responsive.

- Call `ctx.block(reason)` to veto the operation. The reason is the message you see.
- An error, or a call to `ctx.fail`, also vetoes the operation. A gate fails closed: if it does not finish cleanly, the operation does not run.
- If no gate blocks, the operation continues.
- When several gates apply to an operation, they run one after another, ordered by file name. The first one that blocks stops the operation, and the rest do not run.
- A gate you have not reviewed does not run, and it stops the operation: the message names the workflow and asks you to review it. A gate fails closed.
- You can cancel the operation while its gates run. The gates are cancelled with it.

This gate blocks a restack when its root branch is `main` or `release`:

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

### Transform: field values

A `transform.<field>` workflow builds the value of a field. The value the workflow returns becomes the value of the field. Return `nil` to suggest nothing.

- When several transform workflows apply to a field, they run in sequence, ordered by file name.
- If you change the field while a transform runs, the returned value does not overwrite your text. It appears as a suggestion with **Replace** and **Discard** buttons. Choose **Replace** to apply it, or **Discard** to keep your text.

This transform asks a command-line LLM tool for a commit message, up to three times. It returns the first non-empty answer:

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

If all three answers are empty, the script returns nothing, and no suggestion is made. The output of the command is untrusted. See [Approvals and security](approvals-and-security.md).

### After: observers

An `after.<operation>` workflow runs after the operation finishes. It returns nothing. If it fails, GitMaster shows a warning, and the operation is not affected. After workflows run in parallel, and the payload of `input.trigger` carries what the operation produced, such as the new commit.

## Operations and fields

> **Planned.** The phases of these operations are not available yet.

| Operation | Phases | Field for `transform` |
|---|---|---|
| `commit` | `before.commit`, `after.commit` | `transform.commit_message`: the message in the composer |
| `restack` | `before.restack`, `after.restack` | None |
| `branch_create` | `before.branch_create`, `after.branch_create` | `transform.branch_name`: the name of the new branch |
| `conflict_resolve` | `before.conflict_resolve`, `after.conflict_resolve` | `transform.conflict_hunk`: a resolution suggestion for one conflict hunk |

See also [Branches](../guide/branches.md), [Stacks](../guide/stacks.md) and [Conflicts](../guide/conflicts.md).

## Reconciliation of effects

> **Planned.** The `stale` outcome is returned by the restack calls, which are not available yet.

An applying action checks its target before it acts. The target is the set of references, the index, or the commit message that the script used to compute the action. If the target changed after the inputs were computed, the action ends with the outcome `stale`.

A run stops with an error when an action ends with any outcome other than `done`, unless the script handles that outcome. To read the outcome instead, wrap the call with `ctx.try`. See [Writing workflows](writing-workflows.md).

## Native Git hooks

Native Git hooks, such as `pre-commit`, are not workflows. They still belong to Git. GitMaster does not run or wrap them. When a hook rejects a commit, its output is the error that `git commit` reports.

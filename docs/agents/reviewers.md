---
title: Review workflows and scorecards
description: Run reviewers, which are workflows that start your own command-line agent, keep a scorecard for each branch and block a push with a workflow that reads the scorecards.
status: planned
order: 30
---

> **Planned.** Review workflows, scorecards and the review comment labels are not available in the app yet.

A reviewer is a [workflow](../workflows/overview.md) that starts an agent on your machine to review a change. The agent publishes its result through the [MCP server](mcp-server.md) as a **scorecard**: one row with a status, a short summary and review comments that point at lines of the change. GitMaster calls no model and stores no key. The agent runs in your own session, with your model and your quota.

## What a reviewer is

A reviewer is a Lua workflow with `review = true` and a manual trigger:

```lua
M.meta = {
  name = "security",
  description = "Looks for security problems in the change",
  trigger = { on = "manual" },
  review = true,
}
```

Inside the script, `ctx.exec` runs your agent in non-interactive mode, and the prompt tells it to publish with `scorecard_publish`. Two templates ship to copy and edit: `review-invariants.lua`, for rules that every change must keep, and `review-for-task.lua`, for one task. An agent can write a reviewer too. Such a file does not run until you approve that exact version in **Run this version?**.

Reviewers live in two folders. Nothing is written into the working tree:

- `~/.config/gitmaster/workflows/` holds generic reviewers, which run in every repository. Switch one off for one repository with **Enabled in this repository** on the Workflows page.
- `.git/gitmaster/workflows/` holds reviewers for one task in one repository.

## Run a review

Open **Review** and choose **Run review…**. Pick one or more of the workflows marked `review = true`. Each pick starts an ordinary manual run, under the workflow's `concurrency` rule. Nothing starts a review on its own: no commit, push or other event does.

## Scorecards

A scorecard is one result of one review. It holds the branch, the commit it judged (`head`), a status (**Pass**, **Needs work** or **Fail**), a summary with a label, decorations and a subject, and a body. It also records the agent, its model and the run it came from.

The Review page lists the scorecards in a table, with the last five results of each reviewer as trend dots and the number of open blocking comments. A scorecard whose commit is no longer the tip of its branch is **Stale**. It stays visible, and GitMaster keeps the last 20 results for each reviewer and branch. Text that an agent wrote is marked **Agent-written** and redacted when it is stored.

## Labels and decorations

Summaries and review comments use conventional-comment labels: `praise`, `nitpick`, `suggestion`, `issue`, `todo`, `question`, `thought`, `chore` and `note`. Decorations add detail: `blocking`, `non-blocking`, `if-minor`, and up to three more words such as `security`.

A label sets the kind and severity of a comment when they are not given. `question` is a question. `praise`, `note` and `thought` are notes. The others are flags. `issue`, `suggestion` and `todo` are medium severity, `nitpick` and `chore` are low, and `blocking` is high. Only you resolve or dismiss a comment.

## Block a push

A push is blocked only by a workflow that you write or approve. This `before.push` workflow reads the scorecards with `ctx.scorecards` and calls `ctx.block` when the rule is not met:

```lua
local M = {}

M.meta = {
  name = "block-unsafe-pushes",
  description = "Block a push while the security review does not pass for this commit",
  trigger = { on = "before.push" },
}

function M.execute(ctx, input)
  local rows = ctx.scorecards({ name = "security", branch = input.repo.branch })
  for _, row in ipairs(rows) do
    if row.head == input.repo.head then
      if row.status ~= "pass" then
        return ctx.block("security review: " .. row.status)
      end
      if row.blocking_open > 0 then
        return ctx.block(row.blocking_open .. " blocking comments are open")
      end
      return
    end
  end
  return ctx.block("no security scorecard for this commit")
end

return M
```

Calling `ctx.block` stops the push, and so does an error in the script. The workflow runs only the version you approved. A change to the file needs your approval again.

To push anyway, switch the workflow off for this repository with **Enabled in this repository**. GitMaster has no override, and no reason field.

## Safety

- A scorecard is one agent's opinion. A `pass` is what the agent reported, not a check that GitMaster made.
- Two guarantees remain. Only you resolve or dismiss a review comment. A blocking workflow runs only the version you approved.
- A `before.push` workflow stops only pushes made in GitMaster. A `git push` in a terminal is not checked.
- The agent loads the checkout's own agent configuration: hooks, MCP servers, skills, commands and plugins. On a branch with commits that are not yours, make the reviewer ask first with `ctx.approve`, and stop when you say no.

## See also

- [Local review](local-review.md): comment on a diff yourself and have an agent work through the comments.
- [MCP tools](../reference/mcp-tools.md): the scorecard and comment tools.
- [Workflow API](../reference/workflow-api.md): `ctx.scorecards`, `ctx.change` and `ctx.block`.
- [Approvals and security](../workflows/approvals-and-security.md): how GitMaster decides what a workflow may do.

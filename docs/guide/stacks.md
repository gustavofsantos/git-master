---
title: Stacks and restack
description: Branch stacks, the By stack sidebar view and history graph, and restack with conflict handling, undo, push and automation.
status: partial
order: 80
---

A stack is a chain of branches in which each branch builds on its parent. A typical stack has a base branch, an API branch and a UI branch on top of the base, and an end-to-end branch on top of the UI branch. When the base changes, restack replays each branch on the new tip of its parent, so the whole stack stays current. If you never give a branch a parent, GitMaster works as a normal Git client and the stack views stay out of your way.

## How a stack is shaped

Each branch has at most one parent and any number of children. Every stack is a tree rooted at `main`:

```text
main
└ ledger-base
  ├ ledger-api
  └ ledger-ui
    └ ledger-e2e
```

A branch cannot have two parents, so a diamond shape is not supported. To combine two branches that both feed one piece of work, create an integration branch that merges both, then make the integration branch the child of one of them.

## Where the parent is stored

GitMaster records the parent in the `.git/config` file of the repository, outside the working tree. Each branch stores two values:

| Key | Value |
|---|---|
| `branch.<name>.stackParent` | The name of the parent branch. |
| `branch.<name>.stackParentBase` | The commit of the parent that the branch's own commits start from. |

```ini
[branch "ledger-api"]
	stackParent = ledger-base
	stackParentBase = <commit>
```

Each value is a single value, not a list. The values stay in your local clone. They never appear in a commit or in a file of the working tree.

GitMaster writes these values with `git config`, and each write shows in the command panel. The recorded base is set as follows:

| Change | Recorded base |
|---|---|
| **New Child Branch** | The tip of the parent, where the new branch starts. |
| **Set Parent…** | The merge base of the branch and its new parent. |
| **Reparent to Grandparent** | Unchanged. The branch's own commits still start at the same commit, so restack replays only those. |
| **Remove From Stack** | Both values are removed. |

When `git branch -d` deletes a branch, Git removes that branch's own values too. Its children keep their values, and their parent shows as gone.

## Derived states

GitMaster compares the stored values with the repository and derives the state of each branch that has a parent. The comparison runs off the UI thread, and the sidebar shows **Calculating…** until it finishes.

| State | When it applies | What you see |
|---|---|---|
| Stale | The tip of the parent is no longer the commit in `stackParentBase`, because the parent has new commits or was rewritten. | The child shows an amber dot, and the parent shows a restack indicator. |
| Diverged base | The recorded base no longer marks where the branch's own commits start: it is not in the history of the branch, or the branch already contains newer commits of its parent. This happens after a rebase or a merge made outside GitMaster. | The branch shows the **diverged** tag, from the first time the repository opens. |
| Parent gone | The parent branch no longer exists. | The branch shows the **parent gone** tag, and its menu offers **Reparent to Grandparent**. |
| Parent merged | The changes of the parent are already in the root of the stack. GitMaster detects this by patch ID, so a squash merge and a rebase merge count, as well as a normal merge. | The branch shows the **parent merged** tag, and its menu offers **Reparent to Grandparent**. |

When a branch is in more than one state, the tag shows the first that applies in this order: parent gone, parent merged, diverged base, stale.

**Reparent to Grandparent** makes the parent of the old parent the new parent. When the old parent is gone and its values went with it, the new parent is the default branch: the branch of `origin/HEAD`, else the first of `main`, `master`, `trunk` and `develop` that exists. You confirm before anything changes, and the branch's commits do not move until you restack.

GitMaster compares patch IDs against the most recent 2,000 commits of the root that the parent's branch does not have. A parent merged further back than that is not detected.

A parent value that points back at its own child, directly or through other branches, is a loop that Git cannot express as a tree. GitMaster shows the branches of a loop under **No stack**, with the **parent loop** tag, until you set or remove one of the parents.
## The By stack view

The sidebar has two views of your branches: **By stack** and **By name**. **By name** groups branches by prefix, such as `feature/`, as described in [Branches](branches.md).

**By stack** shows an indented tree of dependencies, in two sections:

- **Stacks** lists each tree, with its root branch at the top.
- **No stack** lists the branches that have no parent and no children.

The sidebar shows these indicators:

| Indicator | Where | Meaning |
|---|---|---|
| **restack N** | On a parent | N branches below it need a restack: its stale children and every branch under them. The indicator sits on the parent, because the parent is the branch that changed. |
| **↑N** | On a branch | The branch has N commits that its parent does not have. The count is against the parent, not against `origin`. |
| Amber dot | On a child | The child is stale. |

Hover a tag or an indicator to read what it means.

The list is virtualized, so a repository with hundreds of branches stays responsive.

## Set a parent

Every action below is in the branch menu of the sidebar and in the command palette. Each one changes only the two values above. No commit moves until you restack.

- **New Child Branch** creates a branch at the tip of this branch, with this branch as its parent. The dialog is the **New Branch** dialog, with the parent shown and the start point fixed.
- **Set Parent…** makes another local branch the parent of this branch. Pick the parent from the list. GitMaster refuses the branch itself, a branch under it (that would make a loop), a remote branch, and a branch that shares no history with it.
- **Remove From Stack** removes the parent of this branch. Its children keep it as their parent, so it becomes the root of their stack.
- **Reparent to Grandparent** is offered when the parent is gone or merged, as described in [Derived states](#derived-states).

### Drag to set a parent

In the **By stack** view, drag a branch onto another branch. The branch you drop it on becomes the parent of the dragged branch, as with **Set Parent…**. While you drag, the row under the pointer is highlighted when it can be the parent, and the status bar says what the drop does. A row that cannot be the parent is dimmed, and the status bar says why: the branch itself, a branch under it, or its parent already.

## Stacks in the history graph

The [History](history.md) page draws stacks in its graph:

- A child branch that starts from a commit other than the tip of its parent has a dashed amber edge, with the badge **N commits behind parent**.
- Each commit belongs to the branch whose range, from the recorded base to the branch tip, contains it, and its node takes that branch's color. GitMaster does not guess from branch names.
- When HEAD is in a stack, the default scope is the stack of HEAD plus the default branch, shown by the chip **Scope: current stack**. Choose **All refs** in the scope pill to show every reference.
- Each branch has a color derived from its name, and the color stays the same across sessions. The dashed edge and the badge also show the state, so color is never the only signal.

### Commits and Stacks modes

> **Planned.** The **Commits | Stacks** switch is not available yet.

The graph has two modes. **Commits** shows the graph of commits. **Stacks** shows one node per branch, with its commits collapsed or expanded.

## Restack

> **Planned.** Restack is not available in the app yet.

Restack brings a stack up to date. It walks the tree from the top down, so parents come before their children. For each branch, it replays the branch's own commits onto the new tip of its parent. When the branch is done, its base moves to that tip, and the walk continues with the next branch.

### How restack works

- **The plan is exact.** Before anything runs, GitMaster builds a plan. The plan replays each commit in order, with the same algorithm that runs the restack, so it shows exactly which commits conflict. It never reports a probable conflict.
- **A plan is tied to the commits it reads.** A plan refers to the commit IDs of the branches it touches. If any of those branches moves, the plan is invalid, and you build a new one.
- **The restack runs in memory.** It does not touch your working copy, so your files and staged changes stay as they are while it runs.
- **Undo is atomic.** Before a restack starts, GitMaster records the commit of every branch it touches. **Undo restack** restores all of them in one step. Each restack is recorded in the operation log.
- **The state survives.** GitMaster saves the state of a restack as it runs. Closing the app or a crash does not lose it.
- **A conflict pauses only its subtree.** The branch with the conflict and its children wait. Independent sibling branches keep going.
- **Recorded resolutions are reused.** GitMaster enables Git's rerere. The same conflict reappears on each restack, and its recorded resolution is reapplied in the same way. See [Resolving conflicts](conflicts.md).
- **A dirty working copy can block the restack.** If your working copy has uncommitted changes and the current branch is part of the plan, the restack does not start. A banner explains the block. Commit your changes, then start the restack again.
- **Push is a separate step.** GitMaster never pushes restacked branches on its own. **Push with lease** pushes with `--force-with-lease`, so it fails when the remote branch has changed since your last fetch.

### The four steps

1. **Plan.** Review the ordered list of branches, how many commits each one replays, and what depends on what. Start the restack from here.
2. **Running.** Each branch shows its progress. The work happens in memory.
3. **Conflict.** The branch pauses. Choose one of the actions:
   - **Resolve conflict** opens the resolver for the branch. See [Resolving conflicts](conflicts.md).
   - **Skip branch and children** leaves that branch and its children out of the restack.
   - **Abort all** stops the whole restack.
4. **Done.** Choose **Push with lease** to push the restacked branches, or **Undo restack** to restore every branch the restack moved.

## Automate stack updates

> **Planned.** The stack events and the stack workflow templates are not available yet.

Automation is built with workflows, not with a restack setting. Three built-in templates are available, and all three are off by default. Turn on each one per repository:

- **Restack children when parent moves** runs on the `stack.parent_moved` event.
- **Restack stale stacks after fetch** runs after a fetch.
- **Reparent to grandparent when parent is merged** runs on the `stack.parent_merged` event.

A workflow that restacks shows the same four steps as the manual restack. On a conflict, the run pauses only that run and its subtree, notifies you, and waits. It never resolves a conflict by itself. A push in a workflow always asks for your approval. The restack apply step follows the approval rules of the workflow, as described in [Approvals and security](../workflows/approvals-and-security.md).

To write your own stack workflows, see the [Workflows overview](../workflows/overview.md) and [Triggers and phases](../workflows/triggers-and-phases.md).

## Restack with agents

> **Planned.** The stack tools are not available through MCP yet.

Agents use the same stack primitives through MCP. An agent reads the stack with `stack_tree`, builds a plan with `restack_plan`, and asks to apply the plan with `restack_apply`. The apply accepts only a plan ID, so you approve exactly the plan that runs. The apply always waits for your approval in the app. See the [MCP server](../agents/mcp-server.md) page.

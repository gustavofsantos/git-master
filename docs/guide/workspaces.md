---
title: Workspaces
description: Group branches of several repositories into one workspace, a Git repository whose submodules are worktrees of your own clones, to change and test them together with an agent.
status: planned
order: 55
---

> **Planned.** Workspaces are not available in the app yet.

A workspace brings branches of several repositories together in one folder, so you can change them and test them as one system. Use one when a change touches more than one service: each service has its branch, and the workspace holds them side by side, for you and for your agent.

A workspace is a Git repository of its own. Each repository in it is a submodule, and each submodule is a [worktree](worktrees.md) of the clone you already have. Nothing is cloned again: a workspace shares the `.git` folder of each of your clones, so it costs little more disk than the files it checks out, and a commit made in a workspace is on the branch in your clone at once.

## What a workspace holds

```text
~/workspaces/checkout-v2/      the workspace repository
  .gitmodules                  one entry per repository
  service-a/                   worktree of ~/src/service-a, on feat/checkout-v2
  service-b/                   worktree of ~/src/service-b, on feat/checkout-v2
  service-c/                   worktree of ~/src/service-c, on feat/new-pricing
```

| Part | What it is |
|---|---|
| The workspace repository | A Git repository that GitMaster creates. It records which commit of each repository the workspace points to, and it holds your own files for the integration: notes, scripts, the configuration of a local environment. |
| A member | One repository of the workspace, checked out on one branch. Its folder is a linked worktree of your clone, registered in the workspace repository as a submodule. |

The workspace repository is yours to write in. Your service repositories are not touched: GitMaster writes nothing into their working trees, and adds nothing to them but the worktree records Git keeps in their `.git` folder.

## Durable and ephemeral workspaces

A workspace is **durable** or **ephemeral**. A durable workspace stays until you remove it. An ephemeral workspace is removed when you close it.

Removing a workspace, of either kind, follows the rules of [removing a worktree](worktrees.md) for each member. It is blocked when a member has changes that are not committed, when a member is on a detached HEAD with commits no local or remote branch holds, or when the workspace repository has changes of its own, and the dialog lists what would be lost. Commits on a member's branch are never lost: they are on the branches of your clones.

## Create a workspace

1. Run **New Workspace…** from the command palette, or click **+** in the **Workspaces** section of the sidebar.
2. Name the workspace.
3. Add a member for each repository: choose the repository and its branch. A branch that does not exist yet is created from the repository's base branch, such as `main`. **Add by branch name** gives one branch name to every repository you choose, and creates it where it is missing.
4. Choose **Durable** or **Ephemeral**.
5. Check the folder and click **Create workspace**.

GitMaster suggests the folder `<root>/<name>`. The root is `~/workspaces` unless you set `gitmaster.workspace.root` (see [Configuration](../reference/configuration.md#git-configuration-keys)).

A branch can be checked out in one worktree at a time. When a member's branch is already checked out in your clone or in another worktree, the dialog says where, and offers **Go to that worktree**.

## Work in a workspace

A workspace opens in its own window, like any repository. The Working Copy shows the workspace repository, with one row per member that says its branch and whether it has changes or commits the workspace does not point to yet. Open a member to see its own Working Copy, history and branches.

- [Code navigation](code-navigation.md) searches every member: Quick Open lists the files of all members, and text search looks inside them.
- The diff of a member shows the commits it gained, as a list, and opens each one.
- Each member keeps its branch checked out. When a Git command leaves a member on a detached HEAD, its row is marked **detached**, and **Check Out Branch** puts it back on its branch.

### Record a combination

The workspace repository records the commit of every member. Record the combination you tested, "these commits of A, B and C work together", with **Record Combination** in the command palette. It stages the new commit of every member that has one, and commits in the workspace repository with a message that lists each member, its branch and its commit:

```text
Record combination

billing   feat/pay-v2  4f2a9c1
checkout  feat/pay-v2  9be03d7
```

Your own files in the workspace repository, such as notes or scripts, are not staged by it: commit them as in any repository. **Record Combination** is not available when no member has a new commit, or when a member is on a detached HEAD.

The combinations follow each other on one branch, `main`. The workspace's [History](history.md) is the list of the combinations you recorded, and each one opens to the commits of each member.

## Run an agent in a workspace

Start the agent in the folder of the workspace, for example in a [terminal](terminals.md) of the workspace window. The agent sees every member and can change them all.

With the [agent integration](../agents/agent-integration.md) installed, the session is recorded as a [thread](threads.md) of the workspace. The thread is stored once, in the workspace repository, and each edit and each commit in it names the member it was made in. See [Threads in a workspace](threads.md#threads-in-a-workspace). The [MCP tools](../reference/mcp-tools.md) take the member a call is about, and the review loop covers every member.

The local environment the agent sets up, such as containers that run the services, is the agent's and yours: GitMaster does not start or stop it. Its files belong in the workspace repository, so they are versioned with the combination they run.

## Remove a workspace

Run **Remove Workspace** from the palette or the menu of its row. GitMaster removes each member's worktree from your clone, then the workspace folder, after the checks described in [Durable and ephemeral workspaces](#durable-and-ephemeral-workspaces). The branches stay in your clones.

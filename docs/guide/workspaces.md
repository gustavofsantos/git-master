---
title: Workspaces
description: Group branches of several repositories into one workspace, a Git repository whose submodules are worktrees of your own clones, to change and test them together with an agent.
status: partial
order: 55
---

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

> **Planned.** The app does not yet remove an ephemeral workspace when you close it. `gitmaster workspace remove` removes either kind; see [Command line](../reference/cli.md#remove-a-workspace).

A workspace is **durable** or **ephemeral**. A durable workspace stays until you remove it. An ephemeral workspace is removed when you close it.

Removing a workspace, of either kind, follows the rules of [removing a worktree](worktrees.md) for each member. It is blocked when a member has changes that are not committed, when a member is on a detached HEAD with commits no local or remote branch holds, or when the workspace repository has changes of its own, and the dialog lists what would be lost. Commits on a member's branch are never lost: they are on the branches of your clones.

## The Workspaces section

The **Workspaces** section of the sidebar lists the workspaces under the workspace root, by name, in every window. A **here** badge marks the workspace the window shows, and an **ephemeral** badge marks an ephemeral one. Click a row to open that workspace in its own window.

The **+** in the section header opens **New Workspace…**. The menu of a row has **Open Workspace**, **Reveal in File Manager** and **Remove Workspace**. The section starts folded, like **Branches** and **Worktrees**: click its header to open it; see [Branches](branches.md#fold-the-sections).

## Create a workspace

1. Run **New Workspace…** from the command palette, or click **+** in the **Workspaces** section of the sidebar.
2. Name the workspace.
3. Add a member for each repository: choose the repository and type its branch. The repository list offers the repository of the window, your recent repositories and **Choose Folder…**. The branch field suggests the repository's branches as you type. **Branch for all** fills the same branch into every member.
4. Choose **Durable** or **Ephemeral**.
5. Check the folder and click **Create workspace**.

A branch that does not exist yet is created from the repository's base branch, such as `main`; a branch that exists only on a remote is created to track it. Each member row says which will happen before you create the workspace.

GitMaster suggests the folder `<root>/<name>`. The root is `~/workspaces` unless you set `gitmaster.workspace.root` (see [Configuration](../reference/configuration.md#git-configuration-keys)).

Every member is checked before anything is created. When one cannot be added, for example because its branch is checked out in another worktree, the dialog says which and where, offers **Go to that worktree**, and creates nothing. The new workspace opens in its own window.

## Work in a workspace

A workspace opens in its own window, like any repository. The Working Copy shows the workspace repository, with one row per member that says its branch and whether it has changes or commits the workspace does not point to yet. Open a member to see its own Working Copy, history and branches.

- [Code navigation](code-navigation.md) searches every member: Quick Open lists the files of all members, and text search looks inside them.
- The diff of a member shows the commits it gained, as a list, and opens each one.
- Each member keeps its branch checked out. When a Git command leaves a member on a detached HEAD, its row is marked **detached**, and **Check Out Branch** puts it back on its branch.

When the detached HEAD holds commits that no local or remote branch has, **Check Out Branch** refuses and says how many, so they are not left behind: put them on a branch first, from the member's own window.

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

With the [agent integration](../agents/agent-integration.md) installed, the session is recorded as a [thread](threads.md) of the workspace. The thread is stored once, in the workspace repository, and each edit and each commit in it names the member it was made in. See [Threads in a workspace](threads.md#threads-in-a-workspace). The [MCP tools](../reference/mcp-tools.md#tools-in-a-workspace) take the member a call is about by its path, such as `billing/src/main.rs`.

The [local review](../agents/local-review.md) loop covers the members too: comment on a line of a member's file in the **Code** page, and GitMaster measures the agent's change to it as for any file.

The local environment the agent sets up, such as containers that run the services, is the agent's and yours: GitMaster does not start or stop it. Its files belong in the workspace repository, so they are versioned with the combination they run.

## Remove a workspace

1. Choose **Remove Workspace** in the menu of its row in the **Workspaces** section, or in the command palette and pick the workspace with **Tab**.
2. Read the dialog. It checks every member and the workspace repository, and lists what would stop the removal.
3. To remove it anyway, tick the confirmation checkbox. The checkbox names what will be discarded. The remove button stays disabled until you tick it.
4. Click **Remove workspace**.

GitMaster removes each member's worktree from your clone, then the workspace folder. The branches stay in your clones. The workspace that the window shows cannot be removed from that window: close it, or remove it from another window.

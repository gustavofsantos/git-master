---
title: First steps
description: Start GitMaster, open a repository, learn the window, choose a theme and make your first commit.
status: partial
order: 20
---

GitMaster opens one repository in each window. This page shows how to start it and open a repository, tours the window, and walks you through a first commit.

## Start GitMaster

Start GitMaster from the application menu on Linux, or from the Applications folder on macOS. From a terminal, pass a path to open a specific repository:

```sh
gitmaster ~/src/my-project
```

| Command | What opens |
|---|---|
| `gitmaster` | The repository in the current directory, if the directory is inside one. Otherwise, the most recent repository that still exists. If there is none, the welcome screen. |
| `gitmaster PATH` | The repository at `PATH`. A relative path is taken from the current directory. If `PATH` is not a repository, the Git error appears in the window. |
| `gitmaster --version` | Prints the version and exits. |

`gitmaster` accepts at most one path. With more arguments, it prints the usage and exits with status 2. For the full command-line reference, see [CLI](../reference/cli.md).

## The welcome screen

The welcome screen appears when GitMaster starts without a repository: no path was given, the current directory is not a repository, and no recent repository still exists. It shows the GitMaster name, a short description, the **Open a repository** button and, when there are any, the list of **Recent repositories**.

Recent repositories are kept in the app data directory. A recent repository whose folder no longer exists is not offered. If the folder you pick is not a repository, the Git error appears on the welcome screen itself. No dialog blocks the window.

## Open a repository

1. On the welcome screen, click **Open a repository**. The native folder picker opens.
2. Select the repository folder.

To open a repository from the recent list, click its entry.

GitMaster shows one repository per window. Choosing another repository in a window replaces the one shown there. A worktree opens in a new window of its own. See [Worktrees](../guide/worktrees.md).

## The window

The window has four areas: the top bar across the top, the sidebar on the left, the main area in the centre, and the status bar at the bottom.

### Sidebar

The sidebar lists the pages of the repository, then its stacks, branches, worktrees and workspaces. Its footer holds the stashes, the reflog, the workflows and the settings.

| Entry | What it opens | Status |
|---|---|---|
| **Inbox** | What needs you: conflicts, approvals and the work that agents return. The badge counts what needs you and what came back. See [Inbox](../guide/inbox.md). | Implemented |
| **Changes** | Changed files, the diff of a file, and the commit composer. The badge shows the number of changed files. The page also hosts the conflict resolver. See [Working Copy](../guide/working-copy.md). | Implemented |
| **Review** | Review comments on your local changes, for an agent to handle. The badge shows the number of open comments, and it is hidden when there are none. See [Local review](../agents/local-review.md). | Implemented |
| **History** | The commit history of the current branch. See [History](../guide/history.md). | Implemented |
| **Code** | The Code page, with two tabs: **Files**, a folder tree and a read-only file viewer, and **Search**, the text search. See [Code navigation](../guide/code-navigation.md). | Implemented |
| **Agents** | Your agent threads, memories and the labels of the repository, in the **Threads**, **Memories** and **Labels** tabs. The badge shows the number of threads. See [Threads](../guide/threads.md) and [Memories](../guide/memories.md). | Implemented |

The **STACK** section lists the stacks of the repository as trees of parents, with their marks. It is open by default, and a click on its header folds it. It appears only while the stacks preview feature is on. Drag a branch onto another branch to set its parent, and drop a file on a branch row to move it. See [Stacks and restack](../guide/stacks.md).

The **BRANCHES · <repo name>** section lists local branches, then remote branches, grouped by prefix such as `feature/`. A filter box above the list shows `N of M`. The current branch carries a **HEAD** badge, and the **+** button creates a branch. The section is folded by default. See [Branches](../guide/branches.md).

The **WORKTREES** section lists the worktrees of the repository, with the main checkout first. Rows mark the worktree **here**, missing or locked. The **+** button creates a worktree, and the **×** button removes one. The section is folded by default. See [Worktrees](../guide/worktrees.md).

The **WORKSPACES** section lists your workspaces. It is folded by default. See [Workspaces](../guide/workspaces.md).

The footer holds **Stashes**, with the number of stashed changes, **Reflog**, **Workflows** and **Settings**. Stashes and reflog are described on the [Branches](../guide/branches.md#reflog-and-stashes) page.

> **Planned.** **Settings** is not available yet: its entry is disabled and shows **Not available yet**.

Drag the edge of the sidebar to resize it. Its width stays between 200 px and 40% of the window. Double-click the edge to return it to 240 px.

### Main area

The main area shows the page you opened from the sidebar, such as Changes, History or Code, and the diffs and file views that open from them.

### Top bar

The top bar has three zones.

On the left, the repository name is over **on <branch> ▾**. Click the repository name to open the repository menu. Click the branch to open the branch picker. The path is not shown in the top bar.

In the middle, when you select something, the top bar shows **Selected**, the kind and the name of the selection, and up to three actions for it. The selection can be a file on Changes or Code, a commit on History, a thread or a label on Agents, a comment on Review, a stash or a reflog entry. The primary action comes first and is highlighted. The actions are the ones of the item's context menu, in the same order, without the actions that are disabled right now, the destructive ones and the ones that ask first. The **⋯** button opens the command palette with only the actions of the selection. See [Command palette](../guide/command-palette.md#actions-for-the-selection).

On the right, one sync button shows the state of the current branch. Its ▾ opens a menu with **Fetch**, **Pull** and **Push with lease**.

| Button label | Click runs |
|---|---|
| **Up to date** | **Fetch** |
| **Push ↑N** | **Push with lease** |
| **Pull ↓N** | **Pull** |
| **Pull ↓N ↑M** | **Pull**, first, when the branch has diverged from its upstream |
| **Publish** | **Push with lease**, which sets the upstream when the repository has exactly one remote |
| **Fetching…**, **Pulling…**, **Pushing…** | Shown while the command runs |

The commands are described on the [Branches](../guide/branches.md#fetch-pull-and-push-with-lease) page.

GitMaster also fetches in the background every 10 minutes, while the current branch has an upstream and no remote command runs. A background fetch shows no notice. If it fails, the error appears only in the Git commands panel.

When branches of the current branch's stack are behind their parent, the top bar shows **Restack N behind** in amber. It opens the restack dialog. It is part of the stacks preview feature; see [Stacks and restack](../guide/stacks.md).

### Status bar

The status bar sits at the bottom of the window. It shows notices and, while you drag a file to move it, what a drop will do. It also holds the **Git commands** button.

When the last restack can be undone, the status bar offers **Undo restack**. Press `Ctrl+Z` (`⌘Z` on macOS) to run **Undo Last Operation**. The shortcut works while the file list has focus, not while you type in a text field.

GitMaster refreshes the view by itself when you change files, commit, or switch branches outside the app. You do not need to refresh by hand. If the system does not let GitMaster watch the files, the status bar says **Watching is limited; refreshing when the window gains focus.** once. The view then refreshes when the window gets focus.

### Git commands panel

Click **Git commands** in the status bar to open the panel. It lists each Git command that changed something: stage, unstage, commit, checkout, fetch, pull, push, and branch and worktree operations. Each entry shows the program, its arguments, the exit code, the duration, stdout and stderr. The button turns red while the newest command failed.

Read-only commands, such as those that load the status, a diff or the history, are not listed. The panel keeps the last 1000 entries in memory only, and it shows the newest 200 of them. Closing GitMaster clears them.

## Command palette

Press `Ctrl+K` (`⌘K` on macOS) to open the command palette. Type part of an action, a worktree or a branch name, and the palette filters the list as you type. Press `Enter` to run the selected item. Press `Tab` to choose the branch or file the action needs. Press `Esc` to leave the argument step or close the palette. An action that cannot run now is listed dimmed, with the reason in place of its shortcut. See [Command palette](../guide/command-palette.md).

## Choose a theme

GitMaster has a dark theme and a light theme, and follows your system appearance by default. Set the `GITMASTER_THEME` environment variable to `dark`, `light` or `system` before you start the app:

```sh
GITMASTER_THEME=light gitmaster ~/src/my-project
```

> **Planned.** A theme setting in the app's interface is not available yet. The environment variable is the only way to choose the theme.

## Your first commit

This walkthrough commits one changed file.

1. Open a repository and change a file in it with your editor.
2. Open **Changes**. The file appears in the list, under the row of its folder, with a state letter and its name: `M` for modified, `A` for added, `D` for deleted and `?` for new. The header shows the count, such as "1 changed".
3. Select the file to see its diff. The diff header shows the path and the number of added and removed lines, such as `+5 −1`.
4. Stage the file. Click its box, select it and press `Space`, or right-click it and choose **Stage File**. The box becomes checked once the whole file is staged. A dash means only part of it is staged.
5. Type a message in the composer at the top of the column, above the file list.
6. Click the commit button, labelled **Commit on** followed by the branch name, for example **Commit on main**.

The commit runs your repository's Git hooks, such as `pre-commit`, as `git commit` does. The commit then appears in the Git commands panel. Open [History](../guide/history.md) to see it at the top of the list.

To commit part of a file, stage a single chunk or selected lines from the diff instead. See [Diff and staging](../guide/diff-and-staging.md).

## Where to go next

- [Working Copy](../guide/working-copy.md) for the file list and the composer.
- [Diff and staging](../guide/diff-and-staging.md) for chunks, lines and the Staged and Unstaged views.
- [Branches](../guide/branches.md) for switching, creating and deleting branches.
- [Command palette](../guide/command-palette.md) for every action in one list.

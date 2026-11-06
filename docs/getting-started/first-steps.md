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

The window has four areas: the sidebar on the left, the main area in the centre, the top bar above them, and the status bar at the bottom.

### Sidebar

The sidebar lists the places in the repository and its branches and worktrees.

| Entry | What it opens |
|---|---|
| **Working Copy** | Changed files, the diff of a file, and the commit composer. The header shows the number of changed files. |
| **History** | The commit history of the current branch. See [History](../guide/history.md). |
| **Code** | The Code page: a folder tree and a read-only file viewer. See [Code navigation](../guide/code-navigation.md). |
| **Search** | The Search page, which searches the text of the repository. See [Code navigation](../guide/code-navigation.md). |
| **Threads** | Your recorded agent sessions. See [Threads](../guide/threads.md). |
| **Workflows** | The workflows page. See [Workflows overview](../workflows/overview.md). |
| **Stashes** | Stashed changes. |
| **Reflog** | The reference log. |
| **Labels** | The labels of threads, files and commits. |
| **Local review** | Review comments on your local changes, for an agent to handle. See [Local review](../agents/local-review.md). |

Below the entries, the **Branches** section shows your branches in one of two views. **By name** shows local and remote branches in a tree grouped by prefix, such as `feature/`. **By stack** shows the local branches as a tree of parents. It is a preview feature; see [Stacks](../guide/stacks.md). The current branch carries a **HEAD** badge. A search box above the tree narrows the list. Click the header of the **Repository**, **Branches** or **Worktrees** section to fold it. See [Branches](../guide/branches.md).

Below the branches, the **Worktrees** section lists the worktrees of the repository, with the main checkout first. See [Worktrees](../guide/worktrees.md).

Drag the edge of the sidebar to resize it. Its width stays between 200 px and 40% of the window. Double-click the edge to return it to 240 px.

### Main area

The main area shows the page you opened from the sidebar, such as the Working Copy, the history or the code, and the diffs and file views that open from them.

### Top bar

The top bar shows the repository name. Under it is a line with the branch and the path, for example `main · ~/src/my-project`.

| Control | What it does |
|---|---|
| **Fetch** | Runs `git fetch --all --prune`. |
| **Pull** | Runs `git pull --ff-only`. Pull only fast-forwards. It never merges or rebases. |
| **Push with lease** | Pushes the current branch only, using a lease so it does not overwrite changes you have not seen. When the branch has no upstream yet and the repository has exactly one remote, the push also sets that remote as the upstream. |
| **Restack** | Restacks the stack of the current branch. A preview feature; see [Stacks](../guide/stacks.md). |

### Status bar

The status bar sits at the bottom of the window. It holds the **Git commands** button. While you drag a file to move it, the status bar also shows what a drop will do.

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
2. Open **Working Copy**. The file appears in the list with a state letter and its path: `M` for modified, `A` for added, `D` for deleted and `?` for new. The header shows the count, such as "1 changed".
3. Select the file to see its diff. The diff header shows the path and the number of added and removed lines, such as `+5 −1`.
4. Stage the file. Select it and press `Space`, or right-click it and choose **Stage File**. A small mark appears next to the state letter once the file has staged changes.
5. Type a message in the composer at the bottom of the column.
6. Click the commit button, labelled **Commit on** followed by the branch name, for example **Commit on main**.

The commit runs your repository's Git hooks, such as `pre-commit`, as `git commit` does. The commit then appears in the Git commands panel. Open [History](../guide/history.md) to see it at the top of the list.

To commit part of a file, stage a single chunk or selected lines from the diff instead. See [Diff and staging](../guide/diff-and-staging.md).

## Where to go next

- [Working Copy](../guide/working-copy.md) for the file list and the composer.
- [Diff and staging](../guide/diff-and-staging.md) for chunks, lines and the Staged and Unstaged views.
- [Branches](../guide/branches.md) for switching, creating and deleting branches.
- [Command palette](../guide/command-palette.md) for every action in one list.

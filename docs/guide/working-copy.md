---
title: Working Copy
description: The changed-files list, staging files, discarding changes, moving a file to another branch or worktree, the commit composer, automatic refresh and the Git commands panel.
status: partial
order: 10
---

The Working Copy shows the changes in your repository that are not committed yet. Use it to choose what goes into the next commit, write the commit message and commit.

## Changed files

The header reads **Working Copy** and shows how many files changed, for example **6 changed**. Each changed file takes one row, with a state letter and its full path:

| Letter | State |
| --- | --- |
| `M` | Modified |
| `A` | Added |
| `D` | Deleted |
| `?` | Untracked: a new file that Git does not track yet |

A subtle mark next to the letter shows that the file has something staged.

Select a file to see its diff. The diff is described in [Diff and staging](diff-and-staging.md).

## Submodules

A submodule takes one row, with the folder icon and its path. The row shows the branch the submodule has checked out, or **detached**, and marks what changed:

| Mark | Meaning |
| --- | --- |
| **N new commits** | The submodule's `HEAD` has commits that the repository does not record yet. |
| **N behind** | The repository records commits that the submodule's `HEAD` does not have, for example after you pulled the repository. |
| **modified** | Files tracked by the submodule have changes. |
| **untracked** | The submodule has new files that its `.gitignore` does not hide. |

Select the row to see the commits the submodule gained, newest first, each with its subject, author and date, instead of a diff. Staging the row records the submodule's new commit in the repository, like staging a file. Changes made inside the submodule are committed in the submodule itself: choose **Open Submodule** in the row's menu, or double-click the row, to open it in its own window.

In a [workspace](workspaces.md), each member is a submodule, so the Working Copy of a workspace lists the members whose state changed.

## Stage and unstage a file

Staged changes go into the next commit. Stage a whole file when all of its changes belong in the commit:

1. Select the file in the list.
2. Press `Space` to stage it. On a staged file, `Space` unstages it.

You can also right-click the file, or select it and press `Shift+F10`, and choose from the file menu. The menu lists:

| Item | What it does |
| --- | --- |
| **Stage File** or **Unstage File** | Stages the file, or unstages it. Only the one that applies is enabled. |
| **Discard Changes** | Throws away the unstaged changes of the file. See [Discard changes](#discard-changes). |
| **Discard All Changes** | Throws away the staged and the unstaged changes of the file. |
| **View File** | Opens the file in the read-only viewer of the [Code navigation](code-navigation.md) page. |
| **Move File to Branch…** and **Move File to Worktree…** | Move the file's changes to another branch or worktree. See [Move a file to a branch or a worktree](#move-a-file-to-a-branch-or-a-worktree). |
| **Show Threads for File** | Lists the agent threads that edited the file. |
| **Copy File Path** | Copies the path of the file. |
| **Reveal in File Manager** | Shows the file in your file manager. |

For a conflicted file, **Resolve Conflict** is the first item. It is a preview feature; see [Resolving conflicts](conflicts.md).

A disabled item shows the reason in place of its shortcut. **Stage File** and **Unstage File** are disabled when there is nothing to do.

To stage part of a file, see [Diff and staging](diff-and-staging.md).

## Commit

The composer sits at the bottom of the Working Copy. It holds the commit message and the **Commit on <branch>** button, where `<branch>` is the branch you are on.

1. Stage the changes you want in the commit. Only staged changes are committed.
2. Type the commit message in the composer.
3. Click **Commit on <branch>**, for example **Commit on main**.

Git's own hooks run during the commit, such as `pre-commit`. If a hook stops the commit, its output appears as the error of the commit. The same output is recorded in the Git commands panel, described below.

### Commit message suggestions

If a workflow on the `transform.commit_message` phase is loaded, the composer shows a **Suggest message** button. The workflow runs when you click it. It also runs by itself when the message box is empty, has focus, and something is staged. Staging a change alone does not start it, because a workflow may call a paid tool.

A suggestion never overwrites the text you typed. It appears above the message box, with **Replace** to take it and **Discard** to keep your text. See the [workflows overview](../workflows/overview.md).

## Automatic refresh

The list never goes stale, so you do not need to refresh it. GitMaster updates the list when:

- The window gets focus.
- Git's state changes in `.git`, such as `HEAD`, the index or the branch references.
- A file in the working tree changes. Files that `.gitignore` ignores, such as `target/` or `node_modules/`, do not count.

Changes made outside GitMaster, for example in a terminal or an editor, appear here without any action. The **Refresh** action, bound to `r` while the list has focus, is still available if you want to force an update.

Reading the status runs in the background, so the window stays responsive. A small indicator appears only when the read takes longer than about 300 ms.

If the operating system stops GitMaster from watching more folders, or the working tree is too large to watch, the list refreshes only when the window gets focus. The status bar shows **Watching is limited; refreshing when the window gains focus.**

## Discard changes

Discarding throws changes away for good, so GitMaster always asks first. Three actions discard changes:

| Action | Where | What it discards |
| --- | --- | --- |
| **Discard Changes** | File menu, command palette | The unstaged changes of one file. Its staged changes stay. |
| **Discard All Changes** | File menu, command palette | The staged and the unstaged changes of one file. |
| **Discard Chunk** | Diff view, on a chunk's header line | The changes of one chunk. See [Diff and staging](diff-and-staging.md#stage-and-unstage-chunks). |

To discard the unstaged changes of a file:

1. Right-click the file in the Working Copy, or select it and press `Shift+F10`.
2. Choose **Discard Changes**.
3. Read the dialog. Its title names the file, such as **Discard changes to src/lib.rs?**, and its lines say what will be lost.
4. Click **Discard changes** to confirm, or press `Esc` to keep the file as it is.

The last line of every discard dialog is **This cannot be undone.**

A discard is refused, and its menu item is disabled, when there is nothing to throw away. **Discard Changes** says **Nothing unstaged to discard** for a file with only staged changes. **Discard All Changes** says **Nothing to discard** for a clean file. A conflicted file must be resolved first.

## Large repositories

> **Planned.** The behavior for very large repositories described here is not available in the app yet.

On a very large repository, the status is read as a stream, and the list shows a scanning state until the read finishes. The list draws only the rows you can see, so long lists stay smooth. GitMaster does not turn on `core.fsmonitor` or `core.untrackedCache` without your consent. Sparse checkout is supported.

## Move a file to a branch or a worktree

You can move the changes of one file out of the Working Copy and into another branch or another worktree, without switching to it. The file leaves the Working Copy, with its staged, unstaged and untracked state together, and the change is recorded where you drop it.

To move a file, drag its row from the changed-files list and drop it on a row of the sidebar. The file name follows the pointer while you drag. You move one file at a time.

Each row of the sidebar answers while you drag over it. A row that accepts the file is highlighted. A row that cannot accept it is dimmed, and the pointer shows that dropping is not allowed. The status bar says what a drop will do, for example **Commit to `<branch>`** or **Move to `<worktree>`**, or why it will be refused.

The branch you are on and the worktree marked **here** never accept a file.

> **Planned.** The hint is shown in the status bar while the file is dragged, not beside the row yet. The popover opens beside the sidebar, not on its row yet. In the **By stack** view the Branches header is not a drop target yet.

### Move to a worktree

Drop the file on a row of the **Worktrees** section. The change is applied to the same path in that worktree, and nothing is committed. The move runs at once, with no confirmation, and the status bar tells you it is done and offers **Open Worktree**.

In the target worktree the change arrives unstaged. A new file arrives untracked. The index of the target worktree is not touched.

The target worktree may have changes of its own. GitMaster merges your change into the file as it is on disk in that worktree, the way Git merges two sets of changes to one file. The move is refused when:

- The merge has conflicts. The hint says the change conflicts, and nothing is written.
- The file is binary and differs from the version your change started from.
- A new file would land on a file that already exists there with other content.
- The path is a symbolic link, or leaves the worktree.
- The folder of the worktree is missing.

A locked worktree accepts a file, because the lock only protects it from removal.

If the target file changes between the check and the write, the move is refused. Drop the file again.

### Move to a branch

Drop the file on a branch row of the **Branches** section. A popover opens on that row with the commit message, filled with the file name. Press `Enter` to move, or `Esc` to cancel.

GitMaster creates a commit on that branch with your change and leaves the checked-out branch, the index and the rest of the Working Copy alone. It does not run Git hooks such as `pre-commit`, because it does not check the branch out. The popover says so. You stay on your current branch.

The move is refused when:

- Your change conflicts with the tip of the target branch. The hint lists the files in conflict, and nothing changes.
- The branch is checked out in another worktree. Move the file to that worktree instead.
- The branch moved while the popover was open. Drop the file again.

### Move to a new branch or a new worktree

- Drop the file on the **Worktrees** header, or on its **+** button, to create a worktree for it. The dialog of [New Worktree](worktrees.md#create-a-worktree) opens with a new branch, and the file moves when the worktree exists.
- Drop the file on the **Branches** header, or on a prefix group such as `feature/`, to create a branch for it. The popover asks for the branch name, which starts with the prefix, and for the commit message. The new branch starts at the commit you are on.

### What stays in the Working Copy

After a successful move the file is gone from the Working Copy. GitMaster checks the file before it removes it, and when the file changed since the move began, it keeps that file and the status bar says so. If any step fails, your change is not lost: it stays in the Working Copy, in the target, or in both.

The move is recorded in the [Git commands panel](#git-commands-panel).

### From the keyboard

**Move File to Branch…** and **Move File to Worktree…** are in the file menu and in the [command palette](command-palette.md). Select a file, run the action, and press **Tab** to choose the target. They have no default shortcut. Every rule above applies to them.

### Requirements

Moving a file needs Git 2.38 or newer. With an older Git, the drop is refused and the reason reads **Requires Git 2.38 or newer**. The rest of the app does not need 2.38.

## Reviewed marker

> **Planned.** Marking a file as reviewed is not available in the app yet.

A reviewed mark is a reminder that you read a file's changes. It is independent of staging: marking a file as reviewed does not stage it, and staging does not mark it. The mark clears by itself when the change in the file changes. It is a reminder, never an approval.

## Generated files and lockfiles

> **Planned.** Collapsing generated files and lockfiles in the list is not available in the app yet.

Generated files and lockfiles are collapsed in the list. GitMaster reads the rules for generated files from `.gitattributes` and never writes to it.

## Commit menus

> **Planned.** Context menus for commits are not available in the app yet.

## Git commands panel

Open the **Git commands** panel from the status bar. The panel is read-only. It lists each Git command that changed something: stage, unstage, commit, checkout, fetch, pull, push, and worktree and branch operations. Reads, such as status, diffs and history, are not listed.

Each entry shows the program, its arguments, the exit code, the duration, the standard output and the standard error. The output expands when you open it. The status bar button turns red while the newest command failed. The list is kept in memory and holds the last 1000 entries.

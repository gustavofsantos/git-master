---
title: Worktrees
description: Work on several branches at once with worktrees, each in its own window. The Worktrees section, creating, opening, removing safely, locking, pruning and the actions for them.
status: partial
order: 50
---

A worktree is a second folder of the same repository, with its own checked-out branch. You can work on two branches at the same time, each in its own window, without switching branches in one folder. The **Worktrees** section of the sidebar lists them all.

## The Worktrees section

The **Worktrees** section sits under the branch tree in the sidebar. The main checkout, the folder you opened the repository from, is always the first row. The other worktrees follow it. Each row shows the worktree's branch, or a detached HEAD.

Each worktree gets a color from its name, so the same worktree keeps the same color in every session.

Badges mark the state of a row:

| Badge | Meaning |
|---|---|
| **here** | The worktree that the window you are in shows. |
| **missing** | Its folder is gone. Git can prune its record. |
| **locked** | Git has the worktree locked. Removing it is blocked by default. |

The **+** button in the section header starts a new worktree. On a linked worktree row, a **×** appears when you point at the row. It removes that worktree with the same checks as **Remove Worktree**.

## Create a worktree

1. Choose **Create Worktree** in the menu of a branch. The dialog opens with that branch chosen. Or choose **New Worktree…** from the **+** button or the command palette. The dialog opens empty.
2. Choose how the worktree starts. The dialog has three modes:
   - An existing branch.
   - A new branch, created from a commit.
   - A detached HEAD, at a commit with no branch.
3. Check the folder. GitMaster suggests `../<repo>-<branch>`, a sibling of the repository. The suggestion follows the branch name until you type a different folder.
4. Create the worktree. The command appears in the **Git commands** panel.

The branch field suggests local branches as you type.

### The folder

> **Planned.** Setting the base folder for new worktrees with `gitmaster.worktree.root` is not available yet.

GitMaster follows these rules for the folder:

- The folder must not exist. GitMaster refuses an existing folder, even an empty one, with the same message Git gives.
- Any other error from Git appears in the dialog exactly as Git wrote it.

### A branch that another worktree uses

Git refuses to check out a branch that a worktree already uses. The dialog shows Git's message and the button **Open that worktree**. Choose it to open that worktree in its own window instead.

## Open a worktree in its own window

1. Double-click a worktree row. It opens in a new window.
2. Each worktree has its own window. The window you came from does not wait for it and stays as it is.

Double-clicking the **here** row does nothing. A row whose folder is missing shows a message in the status bar instead.

**Open Worktree** does the same from the row menu and from the command palette.

## Remove a worktree

1. Right-click the row and choose **Remove Worktree**. Or point at a linked worktree row and choose **×**. In the command palette, choose **Remove Worktree** and pick the worktree with **Tab**.
2. Read the dialog. It checks the worktree and lists what would stop the removal.
3. If a blocker can be overridden, tick the confirmation checkbox. The checkbox names what will be discarded. The remove button stays disabled until you tick it.
4. Remove the worktree.

GitMaster checks the worktree again at the moment of removal, so the dialog never acts on an old answer.

| Situation | Can the checkbox override it? |
|---|---|
| Staged, unstaged or untracked files | Yes |
| The worktree is locked | Yes |
| Commits that no other local or remote branch has, or commits ahead of the upstream | Yes |
| The main checkout | No |
| The worktree that the window you are in shows | No |
| The folder is missing | No. Use **Prune Worktrees** instead. |

GitMaster removes the folder with `git worktree remove`. It never deletes the folder by hand. Files that Git ignores inside the folder are removed with it.

### Delete the branch too

> **Planned.** Deleting the branch together with its worktree is not available yet.

## Lock and unlock

Choose **Lock Worktree** or **Unlock Worktree** in the row menu, whichever applies. The command palette has both as well.

A locked worktree is blocked from removal until you unlock it or tick the confirmation checkbox. Pruning leaves it out. If Git refuses the lock or the unlock, for example because the worktree is already locked, Git's message appears in the status bar.

## Prune worktrees

Pruning clears the records of worktrees whose folders are gone.

1. On the row of a **missing** worktree, choose **Prune Worktrees**. You can also choose it in the command palette.
2. The dialog lists each stale record with the reason Git gives. Nothing has changed yet.
3. Confirm to remove the records.

Pruning removes Git's records only. It never touches a folder. A locked worktree is left out of the list. If the list changed after the dialog showed it, the prune is refused. Open the dialog again to see the current list.

## Branch menu for a worktree branch

When a branch is checked out in a linked worktree, its branch menu offers worktree actions in place of the usual ones:

- **Open Worktree**
- **Reveal Worktree in File Manager**
- **Copy Worktree Path**
- **New Child Branch**
- **Restack This Stack**
- **Remove Worktree**, in red

**Checkout Branch**, **Rename Branch**, **Delete Branch** and **Create Worktree** are not in this menu, because the branch already has a worktree. The main checkout keeps the normal branch menu.

### Stack actions

> **Planned.** **New Child Branch** and **Restack This Stack** are not available yet.

## Open in a terminal

> **Planned.** **Open in Terminal** is not available yet.

## Worktrees and agents

> **Planned.** A tag in the Worktrees section that marks the worktrees an agent uses is not available yet.

## Worktree actions in the command palette

Open the palette with `Ctrl+K` (`⌘K` on macOS). For the actions that need a worktree, press **Tab** and pick one.

| Action | How the palette runs it |
|---|---|
| **New Worktree…** | Opens the create dialog, empty. |
| **Create Worktree** | Press **Tab** and pick a branch. |
| **Open Worktree** | Press **Tab** and pick a worktree. |
| **Reveal Worktree in File Manager** | Press **Tab** and pick a worktree. |
| **Copy Worktree Path** | Press **Tab** and pick a worktree. |
| **Remove Worktree** | Press **Tab** and pick a worktree. Opens the remove dialog. |
| **Lock Worktree** and **Unlock Worktree** | Press **Tab** and pick a worktree. |
| **Prune Worktrees** | Opens the prune dialog. |

The rules of [Command palette](command-palette.md) apply to every step.

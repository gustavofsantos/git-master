---
title: Branches
description: The branch sidebar, the Mine and All filter, switching, creating and deleting branches, and the Fetch, Pull and Push with lease commands.
status: partial
order: 40
---

The sidebar lists the local and remote branches of the repository. Use it to check out a branch, filter the list, and see which branch HEAD points at. Create and delete branches from the branch menu, the command palette or the sidebar. The top bar holds the remote commands: **Fetch**, **Pull** and **Push with lease**.

## The branch sidebar

The **By name** view groups branches by their prefix. A branch named `feature/login` is listed under the `feature/` group. Local and remote branches are both listed. The branch that HEAD points at has a **HEAD** badge.

Long names keep their end visible. The last 16 characters always stay on screen, and the start of the name shrinks with an ellipsis when the row is narrow. A name longer than 24 characters shows its full text in a tooltip when you hover over it.

The worktrees of the repository are listed in their own section below the branches. For that section, see [Worktrees](worktrees.md).

### Resize the sidebar

1. Drag the edge of the sidebar to change its width.
2. The sidebar stays between 200 px and 40% of the window width. The main area keeps at least 320 px.
3. Double-click the edge to return the sidebar to 240 px.

The width is not kept between runs.

## Filter the branches

The filter sits above the branch list. It has three parts:

- The **Mine** and **All** switch chooses the scope.
- The search box narrows the list by name.
- The count on the right shows `N of M`: the branches shown, and the total.

### Mine and All

**Mine** shows the branches that belong to you:

- The current branch, always.
- Each branch whose tip commit has your `user.email` as its author email. The comparison ignores letter case.
- Each local branch whose author cannot be read.

Remote branches without a readable author are hidden under **Mine**.

**All** shows every branch.

Until you choose a scope, the scope is **Mine** when Git has `user.email`, and **All** when it does not. Your choice is saved in `settings.json`, so it stays after a restart. Without `user.email`, the scope is **All** and the command palette action **Show Only My Branches** is disabled, with the reason.

### Search

The search matches the letters of the full branch name in order, so `flg` finds `feature/login`. The search works together with the scope. The search text is not saved between runs.

When nothing matches, the sidebar says **No branches match** and offers a link that shows all branches.

The palette actions **Show Only My Branches** and **Show All Branches** change the scope. The action that would change nothing is disabled, and its reason says so.

The branch picker in the top bar and the command palette always list every branch, whatever the filter.

## Switch to another branch

Use any of these ways:

1. Open the branch picker in the top bar, type to filter, and choose a branch.
2. Click a branch in the **By name** view of the sidebar.
3. Run **Checkout Branch** from the command palette (`Ctrl+K`, `⌘K` on macOS), then press `Tab` and choose the branch.

Choosing a remote branch that has no local copy creates a local branch that tracks the remote one.

You cannot check out the current branch again. Its entry says **This is the current branch**.

GitMaster does not stash or carry your uncommitted changes across a checkout. If Git refuses the checkout, for example because of uncommitted changes, Git's message appears in the panel and as a notification. Commit or discard the changes, then try again.

## Create a branch

1. Run **New Branch…** from the command palette, from the branch menu, or with the New Branch button in the sidebar.
2. Type the **New branch name**. The form shows the reason when the name is not valid. A branch name follows Git's rules: it cannot contain spaces or `..`, and no part of it can end with `.lock`.
3. Optionally, type the **Branch, tag or commit** to start from. When it is empty, the new branch starts from HEAD. The branch menu fills this field with the branch you chose.
4. Tick **Check out the new branch** to switch to the new branch as well. The dialog shows the Git command it will run: `git branch` with the name and the start point, or `git checkout -b` when the box is ticked.
5. Click **Create branch**. If Git refuses, its message stays in the dialog.

## Delete a branch

1. Run **Delete Branch** from the branch menu. The item is red, because it asks for confirmation.
2. The dialog asks **Delete branch** followed by the name of the branch, with a question mark. It says that Git deletes the branch only when its commits are merged into HEAD or its upstream.
3. Click **Delete branch**. GitMaster runs `git branch -d`.

When Git refuses because the branch has commits that are not merged, the dialog shows Git's message and a checkbox, **Discard commits that exist only on this branch**. The delete button stays off until you tick the checkbox. Then GitMaster deletes the branch with `-D`, which discards those commits.

You cannot delete:

- The branch that is checked out.
- A branch that is checked out in another worktree.

The Delete action covers local branches only.

## Delete merged branches

Run **Delete Merged Branches…** from the command palette. This command always works in two steps.

1. The dialog lists the local branches merged into the base branch, under the line **Merged into** and the name of the base.
2. Click the button that shows the count, such as **Delete 3 branches**. GitMaster deletes only the listed branches, one by one, with `git branch -d`.

The base branch is chosen in this order:

1. The local branch that `origin/HEAD` points at.
2. The first of `main`, `master`, `trunk` and `develop` that exists.
3. The current branch.

The list never includes:

- The base branch.
- The current branch.
- A branch that is checked out in another worktree.
- A protected name: `main`, `master`, `trunk`, `develop`, `dev`, `staging`, `production` or `release`.

A branch appears only when it is merged into the base branch and into HEAD. A branch that Git refuses stays, and the other branches are still deleted. The result says how many were deleted and how many Git refused.

If the branches changed after the list was shown, nothing is deleted. The list is read again, and a notice says so.

> **Note.** A branch merged by squash or by rebase is not detected as merged, because Git cannot see it as merged. It stays in the list.

## Fetch, Pull and Push with lease

The three buttons in the top bar run the remote commands. Each command appears in the Git commands panel. Only one remote command runs at a time, and while one runs, its button shows the progress, such as **Fetching…** or **Pulling…**.

| Button | Runs | Rules |
| --- | --- | --- |
| **Fetch** | `git fetch --all --prune` | Fetches every remote. It removes the remote-tracking branches that no longer exist on their remote. |
| **Pull** | `git pull --ff-only` | Only moves the branch forward. It never creates a merge commit and never rebases. Refuses when HEAD is detached, and when the branch has no upstream. If the branch cannot move forward, Git's message is shown as it is. |
| **Push with lease** | Pushes the checked-out branch | Only the checked-out branch can be pushed. Git refuses the push when the remote branch has changed since your last fetch. The push does not follow tags. |

For **Push with lease**, the upstream decides where the branch goes:

- When the branch has an upstream, the push goes there.
- When the branch has no upstream and the repository has exactly one remote, the push goes to that remote, and the branch's upstream is set to it.
- When the branch has no upstream and the repository has no remote or several remotes, the push is refused. GitMaster does not guess a remote.

## The branch menu

Right-click a branch in the sidebar to open its menu.

| Item | What it does |
| --- | --- |
| **Checkout Branch** | Checks out the branch. |
| **New Branch…** | Opens the New Branch dialog, with this branch as the start point. |
| **Create Worktree** | Opens the Create Worktree dialog with this branch. |
| **Push With Lease** | Pushes the branch with lease. It works for the checked-out branch only. |
| **Copy Branch Name** | Copies the name of the branch. |
| **Delete Branch** | Deletes the branch, after confirmation. The item is red. |

A disabled item shows its reason in place of its shortcut. Items that delete work are red and ask for confirmation first. For the shortcuts of each item, see [Keyboard shortcuts](../reference/keyboard-shortcuts.md).

A branch that is checked out in a linked worktree has a different menu, for working with that worktree. See [Worktrees](worktrees.md).

## Stack actions

> **Planned.** New Child Branch, Restack This Stack and Rename Branch are not available yet.

The branch menu shows **New Child Branch**, **Restack This Stack** and **Rename Branch**. New Child Branch creates a branch whose parent is this branch. Restack This Stack brings the branches of the stack that contains this branch up to date with their parents. Rename Branch changes the name of the branch. For stacks, see [Stacks](stacks.md).

## By stack view

> **Planned.** The By stack view is not available yet.

The **By stack** view shows the branches as a tree of parents, under a **Stacks** section and a **No stack** section. A branch whose parent has moved shows an amber dot. A branch with commits beyond its parent shows their number, such as `↑3`. A parent that has branches to restack shows the action with their number, such as `restack 3`. For stacks, see [Stacks](stacks.md).

## Set a parent by dragging

> **Planned.** Dragging a branch onto another branch to set its parent is not available yet.

In the **By stack** view, drag a branch onto another branch. The branch you drop it on becomes the parent of the dragged branch. For how a parent is set and what it changes, see [Stacks](stacks.md).

## Move a file to a branch

> **Planned.** Moving a file to a branch is not available yet.

Drag a changed file from the Working Copy and drop it on a branch row. A popover asks for the commit message, and the change is committed on that branch. You stay on your current branch, and the file leaves the Working Copy. Drop the file on the **Branches** header or on a prefix group to create a new branch for it. The rules are on the [Working Copy](working-copy.md#move-a-file-to-a-branch-or-a-worktree) page.

## Reflog and stashes

The **Workspace** section of the sidebar lists **Stashes** and **Reflog**. Stashes lists the changes you have saved aside. Reflog lists the past positions of HEAD and of the branches.

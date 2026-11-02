---
title: Branches
description: The branch sidebar, the branch search, switching, creating and deleting branches, and the Fetch, Pull and Push with lease commands.
status: partial
order: 40
---

The sidebar lists the local and remote branches of the repository. Use it to see which branch HEAD points at, to search the list, and to open a branch's menu. Check out, create and delete branches from the branch menu, the branch picker in the top bar or the command palette. The top bar holds the remote commands: **Fetch**, **Pull** and **Push with lease**.

## The branch sidebar

The **By name** view groups branches by their prefix. A branch named `feature/login` is listed under the `feature/` group. Local and remote branches are both listed. The branch that HEAD points at has a **HEAD** badge.

Long names keep their end visible. The last 16 characters always stay on screen, and the start of the name shrinks with an ellipsis when the row is narrow. A name longer than 24 characters shows its full text in a tooltip when you hover over it.

The worktrees of the repository are listed in their own section below the branches. For that section, see [Worktrees](worktrees.md).

### Resize the sidebar

1. Drag the edge of the sidebar to change its width.
2. The sidebar stays between 200 px and 40% of the window width. The main area keeps at least 320 px.

The sidebar starts at 240 px. Its width is not kept between runs.

## Fold the sections

The sidebar has three sections: **Workspace**, **Branches** and **Worktrees**. Click a section's header to fold it, and click it again to open it. A caret before the title shows the state: it points down when the section is open and to the right when it is folded. A folded section shows its header only, so the sections you do not need leave room for the others. The **+** at the end of the Branches and Worktrees headers still works while the section is folded.

All sections are open when the window starts. Which ones are folded is not kept between runs.

## Search the branches

The list shows every local and remote branch of the repository. The search box above the list narrows it by name, and the count on the right shows `N of M`: the branches shown, and the total.

The search matches the letters of the full branch name in order, so `flg` finds `feature/login`. The search text is not saved between runs.

When nothing matches, the sidebar says **No branches match** and offers a link, **Clear the search**.

The branch picker in the top bar and the command palette always list every branch, whatever the search.

## Switch to another branch

Use any of these ways:

1. Open the branch picker in the top bar, type to filter, and choose a branch.
2. Right-click a branch in the sidebar and choose **Checkout Branch**.
3. Run **Checkout Branch** from the command palette (`Ctrl+K`, `⌘K` on macOS), then press `Tab` and choose the branch.

A left click on a branch in the sidebar does not check it out. On the History page, the same click shows that branch's commits instead. See [Scope of the list](history.md#scope-of-the-list).

Choosing a remote branch that has no local copy creates a local branch that tracks the remote one.

You cannot check out the current branch again. Its entry says **This is the current branch**.

GitMaster does not stash or carry your uncommitted changes across a checkout. If Git refuses the checkout, for example because of uncommitted changes, Git's message appears in the panel and as a notification. Commit or discard the changes, then try again.

## Create a branch

1. Run **New Branch…** from the command palette, from the branch menu, or with the **+** button in the Branches header.
2. Type the **New branch name**. The form shows the reason when the name is not valid. A branch name follows Git's rules: it cannot contain spaces or `..`, and no part of it can end with `.lock`.
3. Optionally, type the **Branch, tag or commit** to start from. When it is empty, the new branch starts from HEAD. The branch menu fills this field with the branch you chose.
4. Tick **Check out the new branch** to switch to the new branch as well. The dialog shows the Git command it will run: `git branch` with the name and the start point, or `git checkout -b` when the box is ticked.
5. Click **Create branch**. If Git refuses, its message stays in the dialog.

### Example: a branch for a fix

1. Right-click `main` in the sidebar and choose **New Branch…**. The dialog opens with `main` in **Branch, tag or commit**.
2. Type `fix/login-timeout` in **New branch name**.
3. Tick **Check out the new branch**.
4. Click **Create branch**. The HEAD badge moves to `fix/login-timeout`.

## Delete a branch

1. Right-click the branch and choose **Delete Branch**. The item is red, because it asks for confirmation.
2. The dialog is titled **Delete branch** followed by the name of the branch, with a question mark. It says that Git deletes the branch only when its commits are merged into HEAD or its upstream.
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
| **Push with lease** | `git push --no-follow-tags --force-with-lease` | Pushes the checked-out branch only. Git refuses the push when the remote branch has changed since your last fetch. The push does not follow tags. |

For **Push with lease**, the upstream decides where the branch goes:

- When the branch has an upstream, the push goes there.
- When the branch has no upstream and the repository has exactly one remote, the push goes to that remote, and the branch's upstream is set to it.
- When the branch has no upstream and the repository has no remote or several remotes, the push is refused. GitMaster does not guess a remote.

## The branch menu

Right-click a branch in the sidebar to open its menu. The items appear in this order, split into groups by separators.

| Item | What it does |
| --- | --- |
| **Checkout Branch** | Checks out the branch. |
| **New Branch…** | Opens the New Branch dialog, with this branch as the start point. |
| **Create Worktree** | Opens the Create Worktree dialog with this branch. See [Worktrees](worktrees.md). |
| **New Child Branch**, **Set Parent…**, **Remove From Stack**, **Reparent to Grandparent**, **Restack This Stack** | Stack actions, described in [Stack actions](#stack-actions). |
| **Push With Lease** | Pushes the branch with lease. It works for the checked-out branch only. |
| **Copy Branch Name** | Copies the name of the branch. |
| **Rename Branch** | Listed, disabled. See [Rename a branch](#rename-a-branch). |
| **Delete Branch** | Deletes the branch, after confirmation. The item is red. |

A disabled item shows its reason in place of its shortcut. Items that delete work are red and ask for confirmation first. For the shortcuts of each item, see [Keyboard shortcuts](../reference/keyboard-shortcuts.md).

A branch that is checked out in a linked worktree has a different menu, for working with that worktree. See [Worktrees](worktrees.md#branch-menu-for-a-worktree-branch).

## Stack actions

Stacks are a preview feature. They are hidden in the release build. Start GitMaster with `GITMASTER_PREVIEW=stacks` to turn them on. See [Stacks and restack](stacks.md).

The branch menu has these stack actions:

- **New Child Branch** creates a branch whose parent is this branch.
- **Set Parent…** picks another local branch as the parent.
- **Remove From Stack** removes the parent of this branch.
- **Reparent to Grandparent** is enabled when the parent is gone or merged.
- **Restack This Stack** brings the branches of the stack up to date with their parents.

## Rename a branch

> **Planned.** Rename Branch is not available yet. Its item is listed in the branch menu, disabled.

## By stack view

> **Note.** The **By stack** view is part of the Stacks preview feature. Without it, the sidebar shows only **By name**.

The **By stack** view shows the local branches as a tree of parents, under a **Stacks** section and a **No stack** section. Remote branches are listed in the **By name** view only. A branch whose parent has moved shows an amber dot. A branch with commits beyond its parent shows their number, such as `↑3`. A parent that has branches to restack shows the number of branches, such as `restack 3`. For stacks, see [Stacks and restack](stacks.md).

## Set a parent by dragging

> **Note.** Dragging to set a parent is part of the Stacks preview feature.

In the **By stack** view, drag a branch onto another branch. The branch you drop it on becomes the parent of the dragged branch. A row that cannot be the parent is dimmed while you drag, and the status bar says why. For how a parent is set and what it changes, see [Stacks and restack](stacks.md).

## Move a file to a branch

Drag a changed file from the Working Copy and drop it on a branch row. A popover asks for the commit message, and the change is committed on that branch. You stay on your current branch, and the file leaves the Working Copy. Drop the file on the **Branches** header or on a prefix group to create a new branch for it. The rules are on the [Working Copy](working-copy.md#move-a-file-to-a-branch-or-a-worktree) page.

## Reflog and stashes

The **Workspace** section of the sidebar lists **Stashes** and **Reflog**. Stashes lists the changes you have saved aside. Reflog lists the past positions of HEAD and of the branches.

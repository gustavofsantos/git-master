---
title: Command palette
description: Run any action by name from the command palette, opened with Ctrl+K, and use the context menus that run the same actions.
status: partial
order: 70
---

The command palette runs any action by name. Open it with `Ctrl+K` (`⌘K` on macOS). Every action is defined once, in one registry, and reached from three places: the keyboard, the menus and the palette. A key, a menu item and a palette row run the same action under the same rules.

## Open and search

1. Press `Ctrl+K` (`⌘K` on macOS).
2. Type part of a name. Matching is fuzzy, and the matched part of each result is highlighted.
3. Press the arrow keys to move through the results, and **Enter** to run the highlighted row.

The palette shows how many results match your text.

## Sections

The results are grouped in three sections:

| Section | What it lists |
|---|---|
| **Actions** | Every action of the app. Each row shows its keyboard shortcut on the right, when it has one. |
| **Worktrees** | Your worktrees, matched by name. |
| **Branches** | Local and remote branches. This section appears only after you type. |

A section appears only when it has results.

## Choose a target

Some actions need a target: a branch, a file or a worktree. To pick it from the palette:

1. Find the action, and press **Tab**. The palette lists the targets of that kind.
2. Find the target you want, and press **Enter**.

Press **Esc** to leave the target step. Press **Esc** again to close the palette.

## Disabled actions

An action that cannot run right now stays in the list, dimmed. Its reason takes the place of its shortcut, for example `Nothing is staged.` A disabled action does not run.

The same rule applies to the keyboard shortcut and to the menu item for the action.

## Context menus

A context menu lists the actions for the thing you point at. Open one in one of these ways:

- Right-click a branch in the sidebar.
- Right-click a file in the Working Copy list.
- Right-click a worktree row in the Worktrees section.
- Press `Shift+F10` to open the menu of the selected file. Press **Esc** to close it.

The rules are the same for every menu:

- Items come from the same registry as the palette. Each item shows its shortcut on the right.
- Items are grouped, with a separator between groups.
- A disabled item shows its reason in place of its shortcut. If the item has no shortcut, the reason is the item's note. If it has a shortcut, the reason appears as a tooltip when you point at the item.
- A red item is destructive. When it discards work, the menu asks first, with a question and two buttons. When it opens a dialog, the dialog asks instead.

### Branch menu

> **Planned.** The branch menu cannot be opened from the keyboard yet, and **Restack This Stack** and **Rename Branch** are not available yet.

The branch menu has **Checkout Branch**, **New Child Branch**, **Set Parent…**, **Remove From Stack**, **Reparent to Grandparent**, **Create Worktree**, **Restack This Stack**, **Push With Lease**, **Copy Branch Name**, **Rename Branch** and **Delete Branch**. **Delete Branch** is in red.

For a branch that is checked out in a linked worktree, the menu shows the worktree actions instead. See [Worktrees](worktrees.md).

### File menu

The file menu has **Stage File** or **Unstage File**, whichever applies, **Copy File Path**, **Reveal in File Manager** and **View File**.

#### Move a file

The file menu also has **Move File to Branch…** and **Move File to Worktree…**. Both are in the command palette too, where you press **Tab** to pick the target. They have no default shortcut. Dragging the file onto the sidebar does the same. See [Working Copy](working-copy.md#move-a-file-to-a-branch-or-a-worktree).

#### Discard changes

> **Planned.** **Discard Changes** in the file menu is not available yet.

### Worktree menu

The worktree menu has **Open Worktree**, **Reveal Worktree in File Manager**, **Copy Worktree Path**, **Lock Worktree** or **Unlock Worktree**, and **Remove Worktree** in red. On the row of a missing worktree, it has **Prune Worktrees**.

### Commit menus

> **Planned.** Context menus for commits are not available yet.

## Every action from the keyboard

Every action can be reached from the keyboard. Actions with a shortcut run from it. Actions without one run from the command palette, opened with `Ctrl+K`. The [keyboard shortcuts](../reference/keyboard-shortcuts.md) page lists every shortcut.

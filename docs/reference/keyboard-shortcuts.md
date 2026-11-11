---
title: Keyboard shortcuts
description: Every keyboard shortcut in GitMaster, grouped by area, with its keys on Linux and Windows and on macOS, and whether it works in the app yet.
status: partial
order: 10
---

This page lists the keyboard shortcuts of GitMaster by area. Each row shows the keys on Linux and Windows, then on macOS. Where the macOS key is `⌘`, the Linux and Windows key is `Ctrl`.

The **Status** column shows which keys work in the app. Each row is either implemented, or not available yet.

Actions without a shortcut on this page can be run from the [command palette](../guide/command-palette.md).

## Global

| Action | Linux and Windows | macOS | Status |
|---|---|---|---|
| Open the command palette | `Ctrl+K` | `⌘K` | Implemented |
| Quit | `Ctrl+Q` | `⌘Q` | Implemented |
| Open the context menu of the selected file | `Shift+F10` | `Shift+F10` | Implemented |
| Refresh | `R` | `R` | Implemented |
| Undo Last Operation, which undoes the last restack | `Ctrl+Z` | `⌘Z` | Implemented |

`R` does not work while you type in the message box or in a diff. `Ctrl+Z` works while the file list has focus, not while you type in a text field.

## Working Copy list

| Action | Linux and Windows | macOS | Status |
|---|---|---|---|
| Stage or unstage the selected file | `Space` | `Space` | Implemented |
| Commit, from the commit message box | `Ctrl+Enter` | `⌘↵` | Implemented |

## Diff in Working Copy

| Action | Linux and Windows | macOS | Status |
|---|---|---|---|
| Move to the previous changed line | `Up` | `Up` | Implemented |
| Move to the next changed line | `Down` | `Down` | Implemented |
| Move to the previous hunk | `Ctrl+Shift+B` | `⇧⌘B` | Implemented |
| Move to the next hunk | `Ctrl+Shift+N` | `⇧⌘N` | Implemented |
| Mark or unmark the line under the cursor | `Space` | `Space` | Implemented |
| Start a visual selection at the cursor | `Ctrl+Shift+V` | `⇧⌘V` | Implemented |
| Stage or unstage the chunk under the cursor | `Ctrl+Shift+A` | `⇧⌘A` | Implemented |
| Stage the selected lines | `Ctrl+Shift+S` | `⇧⌘S` | Implemented |
| Unstage the selected lines | `Ctrl+Shift+I` | `⇧⌘I` | Implemented |
| Clear the selection | `Esc` | `Esc` | Implemented |
| Comment on the selected lines | `Ctrl+Shift+M` | `⇧⌘M` | Implemented |
| Open the file at the first line in view | `Ctrl+Shift+O` | `⇧⌘O` | Planned |

## Commit diff (stacked)

| Action | Linux and Windows | macOS | Status |
|---|---|---|---|
| Scroll one line up or down | `Up`, `Down` | `Up`, `Down` | Implemented |
| Scroll one page up or down | `PageUp`, `PageDown` | `PageUp`, `PageDown` | Implemented |
| Move to the next file | `Ctrl+Shift+N` | `⇧⌘N` | Implemented |
| Move to the previous file | `Ctrl+Shift+P` | `⇧⌘P` | Implemented |
| Comment on the selected lines | `Ctrl+Shift+M` | `⇧⌘M` | Implemented |
| Open the file at the first line in view | `Ctrl+Shift+O` | `⇧⌘O` | Planned |
| Go back to the commit | `Esc` | `Esc` | Implemented |

Next file and previous file wrap around at the first and the last file.

## Branch menu

> **Planned.** Some rows here have no key bound yet, and some of their actions are not available yet.

| Action | Linux and Windows | macOS | Status |
|---|---|---|---|
| Checkout Branch, in the branch picker at the top of the window | `Enter` | `Enter` | Implemented |
| Copy Branch Name, for the checked-out branch | `Alt+Ctrl+C` | `⌥⌘C` | Implemented |
| Create Worktree | `Alt+Ctrl+W` | `⌥⌘W` | Planned |
| Push With Lease | `Ctrl+Shift+P` | `⇧⌘P` | Planned |
| New Child Branch | `Ctrl+B` | `⌘B` | Planned |
| Restack This Stack | `Ctrl+Shift+R` | `⇧⌘R` | Planned |
| Rename Branch | `F2` | `F2` | Planned |
| Delete Branch | `Backspace` | `⌫` | Planned |

**Restack This Stack** and the other stack actions belong to stacks, a preview feature. In the stacked commit diff, `Ctrl+Shift+P` moves to the previous file. The branch menu in the sidebar cannot be opened from the keyboard yet, so its **Checkout Branch** item has no key there.

## Search and Quick Open

| Action | Linux and Windows | macOS | Status |
|---|---|---|---|
| Quick Open, find a file in the worktree | `Ctrl+P` | `⌘P` | Implemented |
| Search Text, search the repository | `Ctrl+Shift+F` | `⇧⌘F` | Implemented |

## Code health

These keys work in the hotspot list of the **Health** tab, while the list has focus. Click the list or a row to focus it.

| Action | Linux and Windows | macOS | Status |
|---|---|---|---|
| Select the next or the previous file | `Down`, `Up` | `Down`, `Up` | Implemented |
| Select the first or the last file | `Home`, `End` | `Home`, `End` | Implemented |
| Select the file a page down or up | `PageDown`, `PageUp` | `PageDown`, `PageUp` | Implemented |
| Open the selected file on the Files tab | `Enter` | `Enter` | Implemented |

## Workflows

A manual workflow can declare its own shortcut with `key` in the `trigger` table of `M.meta`. See [Writing workflows](../workflows/writing-workflows.md).

| Action | Linux and Windows | macOS | Status |
|---|---|---|---|
| Save Workflow | `Ctrl+S` | `⌘S` | Implemented |

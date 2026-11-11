---
title: Diff and staging
description: Reading diffs, staging and unstaging chunks and single lines, the Unified and Split layouts, the stacked commit diff and the keyboard shortcuts of the diff views.
status: partial
order: 20
---

The diff shows the changes of one file. Read it on the side you choose, stage or unstage whole chunks or single lines, and pick one of two layouts. The same view shows the changes in the Working Copy and the changes of a commit opened from [History](history.md).

## Staged and Unstaged

The **Staged | Unstaged** selector in the diff header chooses the side you look at:

- **Staged** shows the changes already staged for the next commit.
- **Unstaged** shows the changes you have not staged yet.

The header is one row. It shows the file path, then the totals, such as `+5 −1 · 2 chunks`. On the right it has the comment button, the **Staged | Unstaged** selector and the **Unified | Split** selector.

## Stage and unstage chunks

A chunk is a block of changed lines. Each chunk starts with a `@@` header line, and its buttons sit on that line:

- **Discard Chunk** throws away the chunk's changes. Click it, and the header asks **Discard this chunk?** with **Discard** and **Cancel** buttons. Only the Unstaged side can discard. On the Staged side the button is dimmed and does nothing.
- **Stage Chunk** stages the chunk. On the Staged side, the button reads **Unstage Chunk** and unstages it.

To stage a chunk:

1. Select the file in the [Working Copy](working-copy.md).
2. Open the Unstaged side.
3. Click **Stage Chunk** on the chunk's header line.

## Stage single lines

Stage part of a chunk by choosing lines:

1. Open the Unstaged side of the file.
2. Choose lines in one of two ways. Tick the checkbox in the gutter of each line, or press `Ctrl+Shift+V` (`⇧⌘V` on macOS) to start a visual selection at the cursor, then move the cursor.
3. Press `Ctrl+Shift+S` (`⇧⌘S` on macOS) to stage the selected lines.

On the Staged side, press `Ctrl+Shift+I` (`⇧⌘I` on macOS) to unstage the selected lines.

### Half-staged pairs

A removed line and the added line that replaces it form a pair. If you stage only one side of a pair, the index can end up without the old line and without the new one, and the file may not compile. When your selection splits a pair, the view shows the warning **Half-staged −/+ pair: the index would lose both the old and the new line, and may not compile.** The warning has a **Select the pair** button. It adds the missing side of every split pair to your selection, so the staging takes whole pairs. To stage the selection as it is, press the same key a second time: `Ctrl+Shift+S` (`⇧⌘S` on macOS) to stage, or `Ctrl+Shift+I` (`⇧⌘I` on macOS) to unstage.

## Staging uses the exact diff

Staging always works on the exact diff that Git computes for the file. The lines you stage and the lines you leave out always add up to the whole change: the staged part plus the unstaged part equals the full change.

## Ignoring whitespace

> **Planned.** Ignoring whitespace in the view is not available in the app yet.

Ignoring whitespace only filters what the diff shows. It never changes what gets staged, because staging still uses the exact diff.

## Diff algorithm

The diff uses Git's `histogram` algorithm, the one that `git diff --histogram` uses.

## Unified and Split layouts

Use the **Unified | Split** selector in the diff header to switch layouts. **Unified** is the default.

- **Unified** shows removed and added lines one after another, in one column.
- **Split** shows the old side on the left and the new side on the right. The two sides are paired one row at a time: each row holds one line of the old side and one line of the new side. Where a side has no line for a row, that side stays empty, with no line number and no marker. Each side's gutter marks only its own line.

Each line shows its line number. Both layouts scroll sideways (in Split, both sides together): use a horizontal wheel or a trackpad gesture.

The layout you choose is saved for your user account as `gitmaster.diffLayout` in `~/.gitconfig`. The Working Copy diff and the commit diff share the choice, so changing it in one changes both. A repository can override it with `git config gitmaster.diffLayout split`. If the key is missing or has another value, Unified is used. See [Configuration](../reference/configuration.md#preferences).

## Open the file

Every diff view has an **Open File** button in the header of a file: the Working Copy diff, the stacked diff of a commit, and the diffs of the **Branch** and **Compare** tabs described in [Compare changes](compare-changes.md). Click it, or press `Ctrl+Shift+O` (`⇧⌘O` on macOS), to read the whole file where you are looking.

The file opens in the read-only viewer of the [Code](code-navigation.md#view-file) page, with the first line you see in the diff marked and shown near the top. The line is the one of the new side of the diff. In a stacked diff, the file is the one at the top of the view, and each file header has its own button. When the first row you see has no line on the new side, such as a removed line, the nearest line on the new side after it is used, or the last one before it when there is none.

The viewer shows the file as it is on disk. This has two consequences:

- In the **Unstaged** diff the line numbers are those of the file on disk. In the **Staged** diff, and in the diff of a commit or of a revision, they are the numbers of that version. When the file changed since, the marked line can be a few lines off.
- A file the diff deletes is not on disk. The button is disabled and says **The file is deleted in the working tree.** A binary file has no lines to show, and its button is disabled too.

## Binary files

A binary file shows the notice **Binary file (n/a)** in the Working Copy diff, instead of lines. In the stacked diff of a commit, the notice is **Binary file: its contents are not shown.**

## Stacked diff of a commit

Open a commit from [History](history.md), then click a file in its file list. GitMaster shows the diff of every file in the commit in one continuous view, in the order of the file list, and scrolls to the file you clicked.

Each file has a header with its status letter, its path (a renamed file shows `old → new`), and its `+` and `−` counts. The header above the files shows **N files** and the **Unified | Split** selector, which applies to every file.

A file with nothing to show as lines has a notice in place of the lines. This applies to a binary file, a renamed file with no text changes, a file whose mode changed, and an empty file.

Two limits apply:

- A file with more than 5,000 lines shows the line `and N more lines, not shown.`
- After the first 200 files, the last line reads `and N more files, not shown.`

The stacked commit diff is read-only. It has no stage or discard buttons. Press `Esc` to return to the commit.

## Word-level highlighting

> **Planned.** Highlighting the changed words inside a line is not available in the app yet.

The changed words inside a line are highlighted on the visible chunks.

## Moved code

> **Planned.** Detecting moved code in the diff is not available in the app yet.

Lines that moved from one place to another are marked as moved, not as removed and added.

## Syntax highlighting

> **Planned.** Syntax highlighting in diffs is not available in the app yet.

Diff lines are colored by the syntax of their language.

## Keyboard in the diff views

The diff views have no single-letter shortcuts. Every letter shortcut uses `Ctrl+Shift` (`⇧⌘` on macOS). The [keyboard shortcuts reference](../reference/keyboard-shortcuts.md) lists every shortcut in the app.

### Working Copy diff

| Shortcut | Action |
| --- | --- |
| Up, Down | Move the cursor to the previous and next changed line. |
| `Ctrl+Shift+N`, `Ctrl+Shift+B` (`⇧⌘N`, `⇧⌘B` on macOS) | Next and previous hunk. |
| `Space` | Select or clear the line at the cursor. |
| `Ctrl+Shift+V` (`⇧⌘V` on macOS) | Start a visual selection at the cursor. |
| `Ctrl+Shift+A` (`⇧⌘A` on macOS) | Stage or unstage the chunk at the cursor. |
| `Ctrl+Shift+S` (`⇧⌘S` on macOS) | Stage the selected lines. |
| `Ctrl+Shift+I` (`⇧⌘I` on macOS) | Unstage the selected lines. |
| `Ctrl+Shift+O` (`⇧⌘O` on macOS) | Open the file at the first line in view. |
| `Esc` | Clear the selection. |

### Stacked commit diff

| Shortcut | Action |
| --- | --- |
| Up, Down | Scroll one row. |
| PageUp, PageDown | Scroll one page. |
| `Ctrl+Shift+N`, `Ctrl+Shift+P` (`⇧⌘N`, `⇧⌘P` on macOS) | Next and previous file. Both wrap around at the first and last file. |
| `Ctrl+Shift+O` (`⇧⌘O` on macOS) | Open the file at the top of the view, at its first line in view. |
| `Esc` | Return to the commit. |

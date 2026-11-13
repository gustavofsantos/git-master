---
title: Terminal companion
description: Run GitMaster in a terminal next to your coding agent to read the changes, stage them and comment on them with the mouse or the keyboard.
status: partial
order: 115
---

The terminal companion is GitMaster in a terminal. It runs in a terminal pane next to a coding agent such as Claude Code, for example in a tmux window split in two. You read what the agent changed, stage what you accept and comment on the lines you want changed. The agent reads your comments through the [MCP server](../agents/mcp-server.md) and works through them in its own pane, and the companion shows each comment as the agent handles it.

The companion uses the same engine and the same store as the app. A comment you write in the terminal is listed in the app's **Review** page, and the other way around. It writes nothing into your working tree.

## Start it

```sh
gitmaster tui                 # the repository that contains the current directory
gitmaster tui ~/src/project   # the repository at this path
```

The companion opens no window and needs no display, so it works over SSH and inside tmux, screen or any terminal emulator. It takes the whole terminal, or the whole tmux pane, until you press `q`. On exit the terminal is restored as it was.

A typical tmux layout keeps the agent on one side and the companion on the other:

```sh
tmux split-window -h 'gitmaster tui'
```

The terminal must be at least 80 columns wide and 24 rows high. A smaller terminal shows only a message that asks for more room.

## The screen

> **Planned.** The diff, its header and the comments in it are not shown yet, and the status bar does not count comments yet.

The screen has three parts:

- On the left, the **file list**: the changed files in two groups, **Unstaged** and **Staged**. A file with changes on both sides is listed in both groups. Each row shows the status letter and the path.
- On the right, the **diff** of the file selected in the list, under a header with its path and `+a −r`, for the group it is in: the unstaged diff for a row under **Unstaged**, the staged diff for a row under **Staged**. Each hunk starts with its header and a **[stage]** or **[unstage]** button. Review comments are drawn under the lines they are anchored to.
- At the bottom, the **status bar**: the branch, the number of open and handled comments, and the keys that work where the cursor is.

When the terminal is narrower than 100 columns, the companion shows one part at a time. `Tab` switches between the file list and the diff.

The companion follows the colors of your terminal: added lines use its green and removed lines its red, so it matches your terminal theme and your tmux theme.

The companion keeps itself up to date. When a file changes, when the agent stages something or when the agent handles a comment, the file list, the diff and the comments refresh by themselves.

## Use the mouse

> **Planned.** Selecting lines and the buttons on hunks, selections and comments are not available yet; clicking a file, clicking a pane and the wheel over the file list work.

The companion takes the mouse, so every action below works with a click. In tmux, turn the mouse on first with `set -g mouse on` in your tmux configuration, otherwise tmux keeps the clicks for itself.

| Action | With the mouse |
|---|---|
| Open a file's diff | Click its row in the file list. |
| Scroll | Turn the wheel over the file list or the diff. |
| Move the cursor | Click a line of the diff. |
| Select lines | Press on a line of the diff and drag to the last line. The range is highlighted. |
| Stage or unstage a hunk | Click **[stage]** or **[unstage]** on its header. |
| Act on a selection | Click a button in the bar that appears beside the selection: **[comment]**, and **[stage lines]** or **[unstage lines]**. |
| Act on a comment | Click **[reply]**, **[resolve]**, **[reopen]** or **[delete]** on the comment. |

While the companion has the mouse, your terminal's own text selection needs a modifier: hold `Shift` while you drag in most terminal emulators. Inside tmux, use tmux's copy mode.

## Use the keyboard

> **Planned.** Only `j`, `k`, the arrows, `Ctrl+D`, `Ctrl+U`, `g`, `G`, `Tab` and `q` work yet, and they move in the file list.

Every action has a key, so you can work without the mouse.

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | Move the cursor down or up. |
| `Ctrl+D` / `Ctrl+U` | Move half a screen down or up. |
| `g` / `G` | Go to the first or the last line. |
| `Tab` | Switch between the file list and the diff. |
| `]` / `[` | Go to the next or the previous hunk. |
| `}` / `{` | Go to the next or the previous comment. |
| `v` | Start a line selection at the cursor. Move the cursor to extend it. |
| `Esc` | Clear the selection, or close the comment box. |
| `s` | Stage the selected lines; with no selection, the hunk at the cursor; in the file list, the whole file. |
| `u` | Unstage, with the same rules as `s`. |
| `c` | Comment on the selected lines, or on the line at the cursor. |
| `C` | Show or hide the comments list. |
| `r` | Reply to the comment at the cursor. |
| `R` | Resolve the comment at the cursor. |
| `o` | Reopen the comment at the cursor. |
| `d` | Delete the comment at the cursor, after you confirm with `y`. |
| `?` | Show every key. |
| `q` | Quit. |

## Stage and unstage

> **Planned.** Staging and unstaging are not available yet.

Stage a whole file from the file list, a hunk from its header, or any lines you select in the diff. Staging lines works as in the app; see [Diff and staging](diff-and-staging.md). A selection can span several hunks: each hunk stages its own selected lines.

The selection that stages lines is the same selection you comment on. After staging, the selection clears and the file moves or splits between the **Unstaged** and **Staged** groups.

## Comment on lines

> **Planned.** Commenting is not available yet.

1. Select the lines: drag over them, or press `v` and move the cursor.
2. Click **[comment]** or press `c`. A comment box opens under the selection.
3. Write the comment. `Enter` starts a new line. Press `Ctrl+S` to save it, or `Esc` to cancel. When the box holds text, `Esc` asks first.

The comment appears under its lines with the badge **Open**. Tell your agent to work through the open review comments, or run `/gitmaster:inbox` in Claude Code. The rules are those of [Local review](../agents/local-review.md):

- In the unstaged diff, the lines are the file's own, so the comment is anchored at once.
- In the staged diff, the companion looks for the selected text in the file as it is now. If it appears exactly once, the comment is anchored there; otherwise the companion says so and writes nothing.
- Removed lines are not in the working tree. A selection of only removed lines cannot be commented.

## Follow the agent's work

> **Planned.** Comments and their actions are not shown yet.

Each comment shows its badge, **Open**, **Handled**, **Resolved** or **Outdated**, its author and its replies, oldest first. A reply from the agent is labelled **Agent-written**. When the agent handles a comment, the badge changes to **Handled** and the agent's summary appears as a reply, without any action from you.

- Read the change, then press `R` or click **[resolve]** to resolve the comment.
- Press `r` or click **[reply]** to answer. Your reply on a handled comment reopens it, and the agent sees it the next time it lists the open comments.
- Press `o` or click **[reopen]** to send a resolved comment back.

An **Outdated** comment has no lines to sit under. It is listed in the comments list.

## The comments list

> **Planned.** The comments list is not available yet.

Press `C` to show the comments of the repository in place of the diff, grouped by file, with their badges. Press `Enter` on a comment, or click it, to go to its lines in the diff. Press `C` again to go back.

## See also

- [Local review](../agents/local-review.md): the review loop, comment states and replies.
- [Diff and staging](diff-and-staging.md): staging hunks and lines in the app.
- [MCP server](../agents/mcp-server.md): connecting your agent.
- [Command line](../reference/cli.md): every `gitmaster` subcommand.

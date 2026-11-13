---
title: Terminal companion
description: Run GitMaster in a terminal next to your coding agent to read the changes, stage them and review them in threads, with buttons you click or keys you press.
status: partial
order: 115
---

The terminal companion is GitMaster in a terminal. It runs in a terminal pane next to a coding agent such as Claude Code, for example in a tmux window split in two. You read what the agent changed, stage what you accept and start a review thread on the lines you want changed. The agent reads your threads through the [MCP server](../agents/mcp-server.md) and works through them in its own pane, and the companion shows its answers as they arrive.

The companion uses the same engine and the same store as the app. A thread you start in the terminal is listed on the app's **Review** page, and the other way around. It writes nothing into your working tree.

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

> **Planned.** The top bar, the tabs, the file tree with its counts, the context chips and the button bar are not shown yet; the companion shows a flat list of unstaged and staged files and a status line.

Every screen has the same frame:

- The **top bar** shows `gitmaster`, the tabs **Diff**, **File** and **Reviews**, and on the right the branch and its upstream. The **Reviews** tab shows how many threads are open.
- The **sidebar** on the left lists the changed files as a tree of folders. Two chips at its top choose what it lists: **Unstaged N**, the changes not staged yet, and **Staged N**, the changes ready to commit. Each file shows its status letter, `+a −r` and the number of threads on it. Click a folder to fold or unfold it.
- The **main pane** on the right shows the selected tab for the selected file.
- The **button bar** at the bottom lists the actions that work where you are, each with its key. Every entry is a button: click it or press its key.

When the terminal is narrower than 100 columns, the sidebar and the main pane show one at a time. `Tab` switches between them.

The companion uses your terminal's colors, so it matches your terminal theme and your tmux theme.

The companion keeps itself up to date. When a file changes, when the agent stages something or when the agent answers a thread, the screen refreshes by itself.

## Buttons and keys

Everything the companion does is a button you can click, and every button has a key. Buttons look like `[stage]`. The tabs, the chips, the folders, the files, the hunk headers, the threads and the entries of the button bar are buttons too. In tmux, clicks reach the companion when the mouse is on (`set -g mouse on`).

The wheel scrolls the pane under the pointer. While the companion has the mouse, your terminal's own text selection needs a modifier: hold `Shift` while you drag in most terminal emulators. Inside tmux, use tmux's copy mode.

These keys work everywhere:

| Key | Action |
|---|---|
| `1`, `2`, `3` | Show the **Diff**, **File** or **Reviews** tab. |
| `Tab` | Switch between the sidebar and the main pane. |
| `t` | Switch the sidebar between **Unstaged** and **Staged**. |
| `?` | Show every key. |
| `q` | Quit. |

> **Planned.** Only `Tab` and `q` work yet, with `j`, `k`, the arrows, `Ctrl+D`, `Ctrl+U`, `g` and `G` in the file list.

## Read the changes

> **Planned.** The **Diff** tab is not shown yet.

The **Diff** tab shows the diff of the selected file in the chosen context: its unstaged changes under **Unstaged**, its staged changes under **Staged**. Its header shows the path, `+a −r` and which hunk the cursor is in, such as `hunk 1/5`.

Each hunk starts with a header bar that shows the function or section it is in, as Git finds it, and a button: **[stage]** under **Unstaged**, **[unstage]** under **Staged**. Each line shows its number in the file and a `+` or `−` in the gutter, in your terminal's green and red. Code is colored by its language.

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | Move the cursor down or up a line. |
| `J` / `K` | Go to the next or the previous hunk. |
| `Ctrl+D` / `Ctrl+U` | Move half a screen down or up. |
| `g` / `G` | Go to the first or the last line. |
| `}` / `{` | Go to the next or the previous thread. |
| `f` | Show the whole file on the **File** tab, at the same line. |
| `e` | Open the file in your editor at the line under the cursor. |

`e` opens the editor named by `$VISUAL` or `$EDITOR` in the companion's terminal. The companion comes back when the editor exits.

## Stage and unstage

> **Planned.** Staging and unstaging are not available yet.

Stage what you accept, at the size you choose:

- A file: click **[stage]** on its row in the sidebar, or press `s` with the file selected.
- A hunk: click **[stage]** on its header, or press `s` with the cursor in it and nothing selected.
- Lines: select them (see below), then click **[stage lines]** or press `s`.

Under **Staged**, the same buttons and `u` unstage. Staging lines works as in the app; see [Diff and staging](diff-and-staging.md). A selection can span several hunks: each hunk stages its own selected lines.

## Select lines

> **Planned.** Selecting lines is not available yet.

Press on a line and drag to the last line, or press `V` and move the cursor. The header shows the range, such as `44–48 · 5 lines`, and the selected lines are marked in the gutter. `o` moves the cursor to the other end of the selection, and `Esc` clears it.

A selection shows a bar with **[comment]** and **[stage lines]** (or **[unstage lines]** under **Staged**). The same selection is used for both.

## Start a thread

> **Planned.** Starting a thread is not available yet.

1. Select the lines.
2. Click **[comment]** or press `c`. A box opens under the selection. It names the lines it is anchored to.
3. Choose the type with the chips **change**, **question** or **nit**: click one, or press `Tab` to move between them.
4. Write the comment. Press `Enter` or click **[create]** to start the thread; `Alt+Enter` starts a new line. `Esc` cancels; when the box holds text, it asks first.

The thread appears under its lines with the status **Open**. Your agent reads it the next time it lists the review comments over MCP. The rules are those of [Local review](../agents/local-review.md):

- Under **Unstaged**, the lines are the file's own, so the thread is anchored at once.
- Under **Staged**, the companion looks for the selected text in the file as it is now. If it appears exactly once, the thread is anchored there; otherwise the companion says so and writes nothing.
- Removed lines are not in the working tree. A selection of only removed lines cannot start a thread.

The type is the comment's label: **change** is `issue`, **question** is `question`, **nit** is `nitpick`.

## Threads in the code

> **Planned.** Threads are not drawn in the code yet.

Each thread is drawn under the last line it is anchored to, in a box with a colored edge. Its first line shows the thread's number, its lines, its status and how many messages it has. Below come the messages, oldest first, each with its author and time. A message from the agent is marked **Agent-written**. When the agent has handled the thread, the box also shows what the agent changed, as GitMaster measured it from its own snapshots: the lines added and removed in the thread's lines, and elsewhere.

An open thread is drawn whole. A handled or resolved thread folds to its first line; click it or press `Enter` on it to unfold it. The box has the buttons for what you can do next:

| Button | Key | Action |
|---|---|---|
| **[reply]** | `r` | Write a reply. Your reply on a handled thread reopens it. |
| **[resolve]** | `R` | Resolve the thread. |
| **[reopen]** | `o` | Send a resolved thread back to the agent. |
| **[delete]** | `D` | Delete the thread, after you confirm with `y`. |

## The Reviews tab

> **Planned.** The **Reviews** tab is not available yet.

The **Reviews** tab lists the threads of the repository. The chips at the top filter them: **open**, **handled**, **resolved** and **all**, each with its count. The list is grouped by who acts next:

- **Waiting for the agent**: open threads, the last word yours.
- **Waiting for you**: handled threads, and questions the agent asked.
- **Resolved**.

Each entry shows the thread's status, number, file and lines, the last author and time, and the start of the last message. Select one to see it on the right: the code around its anchor as a diff, its messages, the change the agent made, and a reply box. `Enter` sends your reply and reopens the thread; **[resolve]** or `R` resolves it without a reply. **[go to diff]** or `g` shows the thread in the **Diff** tab.

| Key | Action |
|---|---|
| `j` / `k` | Select the next or the previous thread. |
| `F` | Show the next filter. |
| `Enter` | Reply to the selected thread. |
| `R` | Resolve the selected thread. |
| `o` | Reopen the selected thread. |
| `g` | Show the thread in the **Diff** tab. |

An **Outdated** thread, whose text is no longer found exactly once, is listed here and is not drawn in the code.

## The File tab

> **Planned.** The **File** tab is not available yet.

The **File** tab shows the whole selected file as it is in the working tree, colored by its language. The gutter marks the lines that changed with `┃` and the places where lines were removed with `▾`. A long run of unchanged lines folds to one line, such as `┈┈ 14 unchanged lines (53–66)`; click it or press `z` on it to unfold it. Threads are drawn folded under their lines.

Select lines and start a thread here as in the **Diff** tab: any line of the file can carry a thread, changed or not. `f` goes back to the **Diff** tab at the same line, and `e` opens your editor there.

## See also

- [Local review](../agents/local-review.md): the review loop, thread states and replies.
- [Diff and staging](diff-and-staging.md): staging hunks and lines in the app.
- [MCP server](../agents/mcp-server.md): connecting your agent.
- [Command line](../reference/cli.md): every `gitmaster` subcommand.

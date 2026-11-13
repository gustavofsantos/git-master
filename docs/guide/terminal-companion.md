---
title: Terminal companion
description: Run GitMaster in a terminal next to your coding agent: one status screen of foldable sections, a menu that shows what you can do where you are, and keys you can remap.
status: partial
order: 115
---

The terminal companion is GitMaster in a terminal. It runs in a pane next to a coding agent such as Claude Code, for example in a tmux window split in two. You read what the agent changed, stage what you accept, start a review thread on the lines you want changed, and follow how the code got where it is. The agent reads your threads through the [MCP server](../agents/mcp-server.md) and works through them in its own pane, and the companion shows its answers as they arrive.

The companion uses the same engine and the same store as the app. A thread you start in the terminal is listed on the app's **Review** page, and the other way around. It writes nothing into your working tree.

## Start it

```sh
gitmaster tui                 # the repository that contains the current directory
gitmaster tui ~/src/project   # the repository at this path
```

The companion opens no window and needs no display, so it works over SSH and inside tmux, screen or any terminal emulator. It takes the whole terminal, or the whole tmux pane, until you quit. On exit the terminal is restored as it was.

```sh
tmux split-window -h 'gitmaster tui'
```

The terminal must be at least 80 columns wide and 24 rows high. A smaller terminal shows only a message that asks for more room.

The terminal must use a Nerd Font: the companion draws its icons, such as the branch, the folders, the file types and the status of a thread, with Nerd Font glyphs, and has no other set. With another font they show as empty boxes.

The companion uses your terminal's colors, so it matches your terminal theme and your tmux theme. It keeps itself up to date: when a file changes, when the agent stages something or when the agent answers a thread, the screen refreshes by itself.

## Screens

> **Planned.** The companion shows tabs and a sidebar; the screens below are not available yet.

The companion is a stack of screens. It opens on **Status**. Opening something, such as a thread or the blame of a line, puts its screen on top; `Esc` goes back to the screen below, exactly as you left it: the same cursor, the same folds, the same selection. The line at the bottom shows where you are, such as `status › threads › thread 12`; click a step to go back to it.

| Screen | Shows |
|---|---|
| **Status** | The repository at a glance, in sections. |
| **Threads** | The review threads, by who acts next. |
| **Thread** | One review thread: its lines, its messages, what the agent changed. |
| **Diff** | The changes of the branch against its base, or of a commit. |
| **File** | A whole file, with its changes marked. |
| **Blame** | Who last changed each line of a file, a person or an agent's thread. |
| **Origin** | How some lines got where they are, change by change. |
| **Agent thread** | An agent's session, by its request, decisions, changes and runs. |

Every screen is made of sections you can fold. `Tab` folds or unfolds the section at the cursor; a click on a section's title does the same. `Enter` opens what is at the cursor: a thread, a commit, a file at that line in your editor.

## The Status screen

> **Planned.** The Status screen is not available yet.

Status starts with the branch: **Head**, the branch and its last commit; **Merge**, its upstream or base; **Stack**, when the branch is in a [stack](stacks.md). Then come its sections, each with a count:

- **Untracked files**, **Unstaged changes** and **Staged changes**. A file unfolds into its hunks, with line numbers, `+` and `-` in your terminal's green and red, and code colored by its language. Review threads are drawn under the lines they are about.
- **Review threads**, by who acts next: waiting for you, waiting for the agent.
- **Commits** on the branch that are not on its base.
- **Stashes**.

Each file shows its status, an icon for its type, `+a -r` and how many threads on it are not resolved.

## Modes

> **Planned.** Modes are not available yet.

What a key does depends on the mode, shown at the left of the bottom line:

| Mode | When |
|---|---|
| **NORMAL** | Moving and acting. |
| **VISUAL** | Lines are selected: an action applies to them. |
| **INSERT** | You are writing a comment, a reply or a commit message. |

## The menu

> **Planned.** The menu is not available yet.

Press `Space` to see what you can do where you are. The menu opens at the bottom of the screen, grouped, one key for each action, and lists only what applies to what is under the cursor or selected. Its title says what it acts on, such as `Hunk 2/5 · src/ledger/posting.rs`. Press a key to act, or click the entry. An entry ending in `…` opens a second menu. `Esc` closes it.

Most menus close after one action. A menu for something you repeat, such as staging hunk after hunk or moving from thread to thread, stays open while you work and says so; `Esc` closes it.

You do not have to wait for the menu: `Space s` stages at once.

Press `?` to see every key of the screen you are on.

## Keys

> **Planned.** These keys are not available yet; see [Your own keys](#your-own-keys) to change them.

| Key | Action |
|---|---|
| `↓` `↑`, `j` `k` | Move down or up. |
| `PgDn` `PgUp`, `Ctrl+D` `Ctrl+U` | Move half a screen. |
| `Home` `End`, `g g` `G` | Go to the top or the bottom. |
| `Shift+↓` `Shift+↑`, `V`, a drag | Select lines (VISUAL). |
| `Tab` | Fold or unfold the section at the cursor. |
| `Enter` | Open what is at the cursor. |
| `Esc` | Close the menu, clear the selection, or go back one screen. |
| `Space` | Open the menu. |
| `?` | Every key of this screen. |
| `Ctrl+C` | Quit. |

## Stage and unstage

> **Planned.** Staging from the Status screen and its menu is not available yet.

Stage what is under the cursor, at the size you choose: a section, a file, a hunk, or the lines you selected. Open the menu and choose **stage** or **unstage**. Staging lines works as in the app; see [Diff and staging](diff-and-staging.md). A selection over several hunks stages each hunk's selected lines.

## Review threads

> **Planned.** Threads from the menu, with the text box, are not available yet.

Select lines, or put the cursor on one, open the menu and choose **comment**. A text box opens at the bottom with the screen still visible above. Choose the type: **change**, **question** or **nit**. Write the comment; `Enter` starts a new line, `Ctrl+S` sends it and `Esc` cancels, asking first when the box holds text.

The thread appears under its lines with the status **Open**. The agent reads it the next time it lists the review comments over MCP. The rules are those of [Local review](../agents/local-review.md): under **Unstaged** the lines are the file's own; elsewhere the companion finds the selected text in the file as it is now, exactly once, or says why not; removed lines cannot carry a thread.

On a thread, the menu offers **reply**, **resolve** (or **reopen**) and **delete**, which asks first. Your reply on a handled thread reopens it.

**copy reference** puts `@path:lines` of the selection on the clipboard, to paste into your agent's prompt.

## The Threads and Thread screens

> **Planned.** The Threads and Thread screens are not available yet.

**Threads** lists the review threads in three sections: **Waiting for you** (threads the agent handled, and threads whose last word is the agent's), **Waiting for the agent** (threads whose last word is yours) and **Resolved**, the latest activity first. A filter in the menu narrows the list as the app's **Review** page does: **Open**, **Resolved**, **Outdated**, **Needs answer**, **Agent**, **All**.

**Thread** shows one thread: its lines as they were when you commented (**Before**) and when the agent handled it (**After**), with the changed lines marked; what the agent changed in those lines, as GitMaster measured it from its own snapshots; and the comment and its replies. From the menu: reply, resolve, reopen, delete, and show it in its diff.

## Diff, File, Blame and Origin

> **Planned.** These screens are not available yet.

**Diff** shows the changes of the branch against its base (from the point it split from the default branch to its tip) or of one commit. It is read-only, and you can start a thread on its lines.

**File** shows a whole file as it is in the working tree, colored, with changed lines marked and long unchanged runs folded.

**Blame** shows, for each line, the change that last touched it and who made it: a person, or an agent and the thread it ran in, or **not committed** for your working tree. The section of a line tells its origin: the commit, the agent thread and its request.

**Origin** answers how some lines got where they are, newest first: for each change, the request that led to it, the review thread it answered, the change itself, and the evidence, such as the tests the agent ran. Every result states how strongly it is tied to the code, as the app's [**Show Why This Code Is Here…**](threads.md#understand-why-code-is-the-way-it-is) does.

**Agent thread** shows a captured agent session by sections: the **Request**, the **Decisions** (questions the agent asked you and your answers), the **Changes** to each file, the **Runs** of commands and how they ended, the **Files read**, and the **Timeline**.

## Your own keys

> **Planned.** A key map of your own is not available yet.

Every action has a name, and every key is a binding of a name, so you can make the keys yours: rebind any action, in any mode, and choose the prefix that opens the menu, such as `,` or `g` in place of `Space`. Two keys cannot be taken away: `Ctrl+C` always quits, and `Esc` always closes the menu. A binding that names an unknown action, or a key bound twice, is reported when the companion starts, and the default is kept for it.

## See also

- [Local review](../agents/local-review.md): the review loop, thread states and replies.
- [Threads](threads.md): agent sessions and why code is the way it is.
- [Diff and staging](diff-and-staging.md): staging hunks and lines in the app.
- [MCP server](../agents/mcp-server.md): connecting your agent.
- [Command line](../reference/cli.md): every `gitmaster` subcommand.

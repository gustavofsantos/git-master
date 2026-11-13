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

The companion uses your terminal's colors, so it matches your terminal theme and your tmux theme. Set `NO_COLOR` to anything to turn colors off. It keeps itself up to date: when a file changes, when the agent stages something or when the agent answers a thread, the screen refreshes by itself.

## Screens

> **Planned.** **Blame**, **Origin** and **Agent thread** are not available yet.

The companion is a stack of screens. It opens on **Status**. Opening something, such as a thread or the blame of a line, puts its screen on top; `q` or `Esc` goes back to the screen below, exactly as you left it: the same cursor, the same folds, the same selection. The line at the bottom shows where you are, such as `status › threads › thread/12`; click a step to go back to it.

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
| **Keys** | Every key, grouped, with what it does (`?`). |

Every screen is made of sections you can fold. `Tab` folds or unfolds the section at the cursor; a click on a section's title does the same. `/` searches the screen, as in less: type the text and press `Enter`, then `n` and `N` go to the next and the previous match. `Enter` opens what is at the cursor: a thread, a commit, a file at that line in your editor.

## The Status screen

Status starts with the branch: **Head**, the branch and its last commit; **Merge**, its upstream, with how far ahead and behind it is; **Stack**, when the branch is in a [stack](stacks.md): the branches from the root to yours, how far ahead and behind its parent it is, whether a restack is due and how many branches sit on it; **Base**, the default branch it is compared with. Then come its sections, each with a count:

- **Untracked files**, **Unstaged changes** and **Staged changes**. A file unfolds into its hunks, with line numbers, `+` and `-` in your terminal's green and red, and code colored by its language. Review threads are drawn under the lines they are about.
- **Review threads**, by who acts next: waiting for you, waiting for the agent.
- **Commits** on the branch that are not on its base, newest first, with when each was written. In a stack, the base is the default branch, so the commits of the branches below yours are listed too.
- **Stashes**, newest first.

Each file shows its status, an icon for its type, `+a -r` and how many threads on it are not resolved.

## Modes

What a key does depends on the mode, shown at the left of the bottom line:

| Mode | When |
|---|---|
| **NORMAL** | Moving and acting. |
| **VISUAL** | Lines are selected: an action applies to them. |
| **INSERT** | You are writing a comment, a reply or a commit message. |

## The menu

Press `Space` to see what you can do where you are. The menu opens at the bottom of the screen, grouped, one key for each action, and lists only what applies to what is under the cursor or selected. Its title says what it acts on, such as `Hunk 2/5 · src/ledger/posting.rs`. Press a key to act, or click the entry. An entry ending in `…` opens a second menu. `Esc` closes it.

Most menus close after one action. A menu for something you repeat, such as staging hunk after hunk or moving from thread to thread, stays open while you work and says so; `Esc` closes it.

You do not have to wait for the menu: `Space s` stages at once.

Press `?` to see every key on the **Keys** screen. It opens on top of the screen you were on: move, scroll and search it like any screen, and `q` or `Esc` takes you back.

## Keys

> **Planned.** The keys cannot be changed yet; see [Your own keys](#your-own-keys).

| Key | Action |
|---|---|
| `↓` `↑`, `j` `k` | Move down or up. |
| `PgDn` `PgUp`, `Ctrl+D` `Ctrl+U` | Move half a screen. |
| `Home` `End`, `g g` `G` | Go to the top or the bottom. |
| `Shift+↓` `Shift+↑`, `V`, a drag | Select lines (VISUAL). |
| `] c` `[ c` | Go to the next or the previous hunk. |
| `Tab`, `z a` | Fold or unfold the section at the cursor. |
| `z o` `z c` | Unfold or fold the section at the cursor. |
| `z R` `z M` | Unfold or fold every section. |
| `/`, `n` `N` | Search the screen; the next or the previous match. |
| `Enter` | Open what is at the cursor: a line in your editor, a thread, a commit's diff; on a section, fold or unfold it. |
| `g f` | Open the whole file of the line, file or thread at the cursor on the **File** screen. |
| `Space d b` | Open the branch's changes on the **Diff** screen. |
| `q`, `Esc` | Close the menu, clear the selection, or go back one screen; on **Status**, `q` quits. |
| `Space` | Open the menu. |
| `?` | Open the **Keys** screen. |
| `Ctrl+Z` | Suspend to the shell; `fg` brings the companion back. |
| `Ctrl+L` | Redraw the screen. |
| `Ctrl+C` | Quit. |

## Stage and unstage

Stage what is under the cursor, at the size you choose: a section, a file, a hunk, or the lines you selected. Open the menu and choose **stage** or **unstage**. Staging lines works as in the app; see [Diff and staging](diff-and-staging.md). A selection over several hunks stages each hunk's selected lines.

## Review threads

Select lines, or put the cursor on one, open the menu and choose **comment**. A text box opens at the bottom with the screen still visible above. Choose the type: **change**, **question** or **nit**. Write the comment; `Enter` starts a new line, `Ctrl+S` sends it and `Esc` cancels, asking first when the box holds text.

The thread appears under its lines with the status **Open**. The agent reads it the next time it lists the review comments over MCP. The rules are those of [Local review](../agents/local-review.md): under **Unstaged** the lines are the file's own; elsewhere the companion finds the selected text in the file as it is now, exactly once, or says why not; removed lines cannot carry a thread.

On a thread, the menu offers **reply**, **resolve** (or **reopen**) and **delete**, which asks first. Your reply on a handled thread reopens it.

## Questions wait a second

Every question the companion asks you, and the text box, waits one second before it takes an answer that ends it. In that second, `Esc` and the other ways to cancel are ignored; for a question whose yes changes something you cannot take back, such as deleting a thread, the yes is ignored too. Keys pressed in that second are dropped, so a key you meant for the screen behind cannot answer the question by accident. The keys show dimmed until they are taken. `Ctrl+C` still quits at once, and the menu answers your first key without waiting.

**copy reference** puts `@path:lines` of the selection on the clipboard, to paste into your agent's prompt.

## The Threads and Thread screens

Open **Threads** from the menu anywhere (`Space t`). It lists the review threads in three sections: **Waiting for you** (threads the agent handled, and threads whose last word is the agent's), **Waiting for the agent** (threads whose last word is yours) and **Resolved**, the latest activity first. The line at the top says which filter is on and how many threads it keeps. `Space f` narrows the list as the app's **Review** page does: `o` **Open**, `r` **Resolved**, `u` **Outdated**, `n` **Needs answer**, `a` **Agent**, `A` **All**. `Enter` on a thread opens it.

**Thread** opens with `Enter` on a thread anywhere: in Threads, in **Review threads** on Status, or on a thread under its lines. It shows the thread's lines as they were when you commented (**Before**) and when the agent handled it (**After**; the file as it is now until then), with the changed lines marked `-` and `+`; what the agent changed in those lines, as GitMaster measured it from its own snapshots; and the comment and its replies. It refreshes by itself when the agent answers. From the menu, wherever the cursor is: reply, resolve, reopen, delete, and **show in diff** (`Space g`, or `Enter`), which goes back to Status with the thread under its lines.

## Diff, File, Blame and Origin

**Diff** shows the changes of the branch against its base (from the point it split from the default branch to its tip; `Space d b`) or of one commit (`Enter` on a commit). Each file is unfolded into its hunks, with the review threads under their lines. It is read-only, and you can start a thread on its lines. One Diff screen is open at a time: opening another replaces it.

**File** (`g f` on a line, a file or a thread) shows a whole file as it is in the working tree, colored, at the line you came from. Lines not yet committed are marked `+`: green when the change is not staged, cyan when it is. Long runs of unchanged lines are folded; `Tab` unfolds one. It follows the file as you or the agent edit it. One File screen is open at a time.

`Enter` on a line, on Status, Diff or File, opens the file at that line in `$VISUAL`, else `$EDITOR`, else `vi`. The companion hands the terminal to the editor and takes it back when the editor quits.

> **Planned.** The screens below are not available yet.

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

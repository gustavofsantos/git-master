---
title: Terminal companion
description: Run GitMaster in a terminal next to your coding agent: one status screen of foldable sections, a menu that shows what you can do where you are, commit, pull and push, and keys you can remap.
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

The companion uses your terminal's colors, so it matches your terminal theme and your tmux theme. At start it asks the terminal for its palette and blends those colors into soft backgrounds: the line under the cursor, the selected lines, and added and removed lines tinted with your terminal's green and red. Through tmux this works as it does in the terminal itself. A terminal that does not answer, or that does not set `COLORTERM` to `truecolor` or `24bit`, gets no backgrounds: the line under the cursor is shown reversed. Set `NO_COLOR` to anything to turn colors off. It keeps itself up to date: when a file changes, when the agent stages something or when the agent answers a thread, the screen refreshes by itself.

Code is colored by its language wherever the companion shows it: the hunks on Status and Diff, removed lines as well as added ones, the File and Blame screens, a thread's **Before** and **After** lines, and the patches on an **Agent thread**. Without the soft backgrounds, removed lines stay in your terminal's red. The language comes from the file's name, as in the app's [file viewer](code-navigation.md); a file of another language, a diff of more than 20,000 lines, or a file that takes more than two seconds to color is shown plain.

## Screens

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
| `Space o` | Find a file of the worktree by its name and open it on the **File** screen; see [Open a file](#open-a-file). |
| `g f` | Open the whole file of the line, file or thread at the cursor on the **File** screen. |
| `Space d b` | Open the branch's changes on the **Diff** screen. |
| `Space d s` | Switch the diffs between **Unified** and **Split**. |
| `Space d w` | On the **File** screen, wrap long lines or cut them at the edge. |
| `g b` | Open who last changed each line of the file at the cursor on the **Blame** screen. |
| `Space w` | Show how the selected lines, or the line, got where they are on the **Origin** screen. |
| `Space C` | Commit the staged changes, with a message written in a text box. |
| `Space F` | Fetch every remote. |
| `Space p` | Pull: fast-forward the branch to its upstream. |
| `Space P` | Push the branch to its upstream, after asking. |
| `q`, `Esc` | Close the menu, clear the selection, or go back one screen; on **Status**, `q` quits. |
| `Space` | Open the menu. |
| `?` | Open the **Keys** screen. |
| `Ctrl+Z` | Suspend to the shell; `fg` brings the companion back. |
| `Ctrl+L` | Redraw the screen. |
| `Ctrl+C` | Quit. |

## Stage and unstage

Stage what is under the cursor, at the size you choose: a section, a file, a hunk, or the lines you selected. Open the menu and choose **stage** or **unstage**. Staging lines works as in the app; see [Diff and staging](diff-and-staging.md). A selection over several hunks stages each hunk's selected lines.

## Commit, pull and push

Once you staged what you accept, commit it from Status: open the menu and choose **commit** (`Space C`). A text box opens at the bottom, as for a comment: write the message, the subject on the first line; `Enter` starts a new line, `Ctrl+S` commits and `Esc` cancels, asking first when the box holds text. The commit takes what is staged and nothing else, so **commit** is offered only when something is staged. When Git refuses the commit, the box stays open with Git's reason under your message. When it succeeds, the box closes and the hint line shows the new commit's short id.

The branch's remote is one key away, in the **Remote** group of the menu:

- **fetch** (`Space F`) fetches every remote and removes the remote branches a remote deleted.
- **pull** (`Space p`) fast-forwards the branch to its upstream. It never makes a merge commit and never rebases: when the branch and its upstream have both moved, it fails and says so. It is offered only when the branch has an upstream.
- **push** (`Space P`) pushes the branch alone, with a lease: to its upstream, or, when it has none, to the only remote, which becomes its upstream. It asks first, naming the branch and where it goes, since a remote cannot take a push back. Tags and other branches are never pushed.

These run as the app's own [Fetch, Pull and Push with lease](branches.md#fetch-pull-and-push-with-lease) do, without asking for a password: use a credential helper or an SSH agent. The hint line says what runs (`Pushing…`), then how it ended, with Git's reason when it failed. **Merge** on Status follows: how far ahead and behind the upstream the branch is.

The companion runs no [workflow](../workflows/overview.md) on a commit or a push: it has no place to approve a run. Commit and push from the app when a workflow should gate them.

## Open a file

`Space o` opens the file picker at the bottom of the screen, over whatever screen you are on. Type part of a path: the files of the worktree that match it are listed best first, the letters that matched in bold, with how many files matched out of how many there are. The letters only have to appear in order, so `srcpar` finds `src/parser.rs`. Files are ranked as in the app's [Quick Open](code-navigation.md#how-results-are-ranked): a match in the file's name counts for more than one in its folder, and letters typed next to each other or at the start of a name rank higher. Up to 50 files are listed. The list holds tracked files and untracked files that `.gitignore` does not hide, and is read again each time the picker opens, so a file the agent just wrote is found.

`↓` and `↑`, or `Ctrl+N` and `Ctrl+P` (`Ctrl+J` and `Ctrl+K` too), move through the list. `Enter`, or a click on a file, opens it on the **File** screen. End the query with `:` and a line number, such as `parser:120`, to open the file at that line. `Esc` closes the picker and leaves the screen as it was. While the picker is open every key types, so `q` and `Space` go into the query.

## Review threads

Select lines, or put the cursor on one, on **Status**, **Diff** or **File**, open the menu and choose **comment**. A text box opens at the bottom with the screen still visible above. Choose the type: **change**, **question** or **nit**. Write the comment; `Enter` starts a new line, `Ctrl+S` sends it and `Esc` cancels, asking first when the box holds text.

The thread appears under its lines with the status **Open**, on Status and on the File screen alike. The agent reads it the next time it lists the review comments over MCP. Any lines the companion shows take a thread, on any screen with lines: **Status**, **Diff** (the branch or a commit) and **File**. The thread is made on the version those lines belong to: the working tree under **Unstaged** and on **File**, the index under **Staged**, the commit on a commit's diff, the branch's tip on the branch's diff; removed lines alone are on the version before the change. It is placed in the working tree by the rules of [Anchors](../agents/local-review.md#anchors): a text that repeats in the file is told apart by the lines around it, then by its line, and a text that is gone leaves the thread **Outdated**, kept with its commit. To comment on a file as a whole, comment on its first line.

On a thread, the menu offers **reply** (`Space r`), **resolve** (`Space R`) or **reopen** (`Space O`), and **delete** (`Space D`), which asks first. Your reply on a handled thread reopens it.

## Questions wait a second

Every question the companion asks you, and the text box, waits one second before it takes an answer that ends it. In that second, `Esc` and the other ways to cancel are ignored; for a question whose yes changes something you cannot take back, such as deleting a thread, the yes is ignored too. Keys pressed in that second are dropped, so a key you meant for the screen behind cannot answer the question by accident. The keys show dimmed until they are taken. `Ctrl+C` still quits at once, and the menu answers your first key without waiting.

**copy reference** puts `@path:lines` of the selection on the clipboard, to paste into your agent's prompt.

## The Threads and Thread screens

Open **Threads** from the menu anywhere (`Space t`). It lists the review threads in three sections: **Waiting for you** (threads the agent handled, and threads whose last word is the agent's), **Waiting for the agent** (threads whose last word is yours) and **Resolved**, the latest activity first. The line at the top says which filter is on and how many threads it keeps. `Space f` narrows the list as the app's **Review** page does: `o` **Open**, `r` **Resolved**, `u` **Outdated**, `n` **Needs answer**, `a` **Agent**, `A` **All**. `Enter` on a thread opens it.

**Thread** opens with `Enter` on a thread anywhere: in Threads, in **Review threads** on Status, or on a thread under its lines. It shows the thread's lines as they were when you commented (**Before**) and when the agent handled it (**After**; the file as it is now until then), with the changed lines marked `-` and `+`; what the agent changed in those lines, as GitMaster measured it from its own snapshots; and the comment and its replies. It refreshes by itself when the agent answers. From the menu, wherever the cursor is: reply, resolve, reopen, delete, and **show in diff** (`Space g`, or `Enter`), which goes back to Status with the thread under its lines.

## Diff, File, Blame and Origin

**Diff** shows the changes of the branch against its base (from the point it split from the default branch to its tip; `Space d b`) or of one commit (`Enter` on a commit). Each file is unfolded into its hunks, with the review threads under their lines. It is read-only, and you can start a thread on its lines. One Diff screen is open at a time: opening another replaces it.

Diffs, on Status and on Diff, have two layouts, as in the app's [diff views](diff-and-staging.md#unified-and-split-layouts). **Unified** shows removed and added lines one after another. **Split** shows the old side on the left and the new side on the right, a removed line next to the added line that replaces it; each side has its own line numbers and its own tint. The companion starts in the layout set by `gitmaster.diffLayout` (see [Configuration](../reference/configuration.md#preferences)), and `Space d s` switches it for as long as it runs; the switch does not change the setting, so the app keeps its own. Split needs a terminal at least 100 columns wide: in a narrower one the diffs are shown Unified, and the hint line says why. The cursor stays on the same line when the layout changes.

In Split, a row holds a line of each side, and selecting rows selects both of their lines: staging a selection stages the removed and the added lines of those rows and no others, and a comment covers the new lines from the first selected row to the last.

**File** (`g f` on a line, a file or a thread, or `Space o` from anywhere) shows a whole file as it is in the working tree, every line of it, colored, at the line you came from. The line at the bottom names the file, so the screen has no header. At the left of each line is a gutter, as in an editor's sign column:

- a sign on a line not yet committed. Its color says what changed: green for an added line, blue for a modified one (a line that replaces a removed one), red for lines removed next to it. Its shape says whether the change is staged: solid while it is not, open once it is, so the background shows through:

  | Change | Not staged | Staged |
  |---|---|---|
  | Added or modified line | `▎` | `░` |
  | Lines removed after this line | `▁` | `_` |
  | Lines removed before the first line | `▔` | `‾` |

  A line changed again after it was staged shows its unstaged sign;
- the line's number, in bold on the line under the cursor;
- a bar `┃` on a line a review thread covers, in the color of the thread's state: yellow while it is open, cyan once the agent handled it, green when it is resolved. A line under several threads takes the color of the one that waits the most, open before handled before resolved, and its number takes that color too.

The thread is drawn under its last line, as on Status, and a thread whose text is gone is listed above the file as outdated. `Tab` on any line of a thread folds or unfolds it; while it is folded, its last line shows `▸` in the gutter instead of the bar.

Long lines wrap: the rest of a line continues on the next rows, under the code, with the gutter's bars carried along. `Space d w` cuts them at the edge instead, for this session; set `gitmaster.lineWrap` to `false` to start that way (see [Configuration](../reference/configuration.md)). Move with the usual keys, `/` searches the file, `V` selects lines to comment on, and `Space o` opens another file. It follows the file as you or the agent edit it. One File screen is open at a time.

`Enter` on a line, on Status, Diff or File, opens the file at that line in `$VISUAL`, else `$EDITOR`, else `vi`. The companion hands the terminal to the editor and takes it back when the editor quits.

**Blame** (`g b` on a line, a file or a thread) shows the file as it is in your working tree, cut into runs of lines that one change last touched. Each run is a section headed by the commit, its subject, who made it and when, or **not committed** for lines you have not committed yet, staged or not. `Enter` on a run's heading opens the commit on the **Diff** screen; `Enter` on a line opens your editor there; `Space w` asks **Origin** about the line or the lines you selected.

> **Planned.** A run made by an agent does not name the agent and its thread on Blame yet; `Space w` shows them on Origin.

**Origin** (`Space w` on a line or a selection, on Status, Diff, File or Blame) answers how those lines got where they are: each commit that last changed some of them, and under it the agent threads tied to it, with their edits and the requests and answers around each edit. Every thread and edit states how strongly it is tied to the code (a blob match, a line match, a hunk overlap, a commit made during the thread, a link you made), as the app's [**Show Why This Code Is Here…**](threads.md#understand-why-code-is-the-way-it-is) does, in the same words. `Enter` on a commit opens it on the **Diff** screen.

> **Planned.** Origin does not yet show the review thread a change answered, nor the tests the agent ran.

**Agent thread** (`Enter` on an agent thread, one of its edits or a linked thread on **Origin**) shows a captured agent session, read as the app's **Threads** page reads it, by sections: the **Request**, the **Decisions** (questions the agent asked you and your answers), the **Changes** to each file with their patches, the **Runs** of commands, how they ended, how long they took and their output, the **Files read**, and the **Timeline** of turns, folded, with every message and tool call in order. `g f` and `g b` on a changed file open it on **File** and **Blame**. The thread's text is shown as plain text, a line per row; long outputs and patches show their first 40 lines, and a session of more than 5,000 entries shows its first 5,000.

## Your own keys

> **Planned.** A key map of your own is not available yet.

Every action has a name, and every key is a binding of a name, so you can make the keys yours: rebind any action, in any mode, and choose the prefix that opens the menu, such as `,` or `g` in place of `Space`. Two keys cannot be taken away: `Ctrl+C` always quits, and `Esc` always closes the menu. A binding that names an unknown action, or a key bound twice, is reported when the companion starts, and the default is kept for it.

## See also

- [Local review](../agents/local-review.md): the review loop, thread states and replies.
- [Threads](threads.md): agent sessions and why code is the way it is.
- [Diff and staging](diff-and-staging.md): staging hunks and lines in the app.
- [MCP server](../agents/mcp-server.md): connecting your agent.
- [Command line](../reference/cli.md): every `gitmaster` subcommand.

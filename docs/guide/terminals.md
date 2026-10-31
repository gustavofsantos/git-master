---
title: Terminals
description: Run a shell, a command or a coding agent in a terminal inside the app, started at the repository root or in any worktree.
status: planned
order: 110
---

> **Planned.** Terminals are not available in the app yet.

A terminal is a shell that runs inside GitMaster, next to the diff and the history. Use it to run a command quickly, to run the tests, or to start a coding agent such as Claude Code without leaving the window. The **Terminal** section of the sidebar lists your terminals.

GitMaster runs a terminal the way any terminal emulator does. It starts your shell and shows what the shell prints. It never types into a terminal, and it never presses Enter for you.

## The Terminal section

The **Terminal** section sits in the sidebar with the other sections. Each row is one terminal and shows the folder it started in, and the command it is running when it runs one. A terminal whose process has ended stays in the list and shows its exit status.

Click the header to fold the section. The **+** in the header stays available while it is folded. See [Branches](branches.md#fold-the-sections).

Click a row to show that terminal. Terminals appear in a dock at the bottom of the window. The dock keeps the layout of the rest of the window as it is.

## Start a terminal

Click **+** in the section header to start a terminal in the root of the repository. The terminal runs your login shell.

To start in a worktree instead, click the arrow next to **+**. The menu lists the repository root and every [worktree](worktrees.md) of the repository, by branch name. Choose one to start the terminal in that folder. A worktree whose folder is missing is not listed.

**New Terminal** in the [command palette](command-palette.md) does the same as **+**.

## Work in a terminal

The terminal behaves like a regular terminal emulator:

- Colors, cursor shapes, the alternate screen and full-screen programs such as `vim`, `less` and `htop` work.
- Select text with the mouse. Copy and paste with `Ctrl+Shift+C` and `Ctrl+Shift+V` (`⌘C` and `⌘V` on macOS).
- Scroll back through earlier output with the mouse wheel. Search the output with `Ctrl+Shift+F` (`⌘F` on macOS).
- Programs that ask for mouse input receive it.
- A link in the output opens in your browser when you click it with `Ctrl` held (`⌘` on macOS).
- Dead keys, the compose key and input methods produce the characters you expect.

While a terminal has focus it receives every key, except the few shortcuts that belong to the app. The palette lists them.

## Run a coding agent

Start a terminal and type the command of your agent, for example `claude`. The agent is a normal program in the terminal. GitMaster does not read its output and does not show an "agent state" on the row.

> **Note.** An agent that you run in a terminal is not recorded as a thread unless it is set up as described in [Agent integration](../agents/agent-integration.md). A terminal is a place to run a program, not a way for the app to observe it.

## Close a terminal

Click **×** on a row, or run **Close Terminal** from the palette. GitMaster ends the terminal's process group: first it asks the programs to stop, then, after a short grace period, it kills the ones that remain.

Hiding the dock does not close anything. A hidden terminal keeps running and keeps its output.

Terminals live as long as the app. When you quit with terminals that still run a process, GitMaster lists them and asks you to confirm. Closing the window or quitting ends every terminal; terminals are not restored the next time you open the app.

## Safety and limits

- A terminal does not write anything into the repository. Its state is not saved to the repository, to `.git/` or to the configuration.
- A terminal has the same permissions as you. The app does not sandbox it, and [approvals](../concepts/trust-and-safety.md) do not apply to what you or your agent run there.
- A terminal's output stays in memory, up to a fixed number of scrollback lines.
- Terminals run on Linux and macOS.

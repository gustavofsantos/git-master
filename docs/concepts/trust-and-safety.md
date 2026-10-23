---
title: Trust and safety
description: The trust model of GitMaster: who may change a repository, what counts as untrusted data, what the app runs and records, and where its data is kept.
status: partial
order: 20
---

GitMaster is built so that every change to a repository has an actor you can identify. This page explains who may do what, what the app treats as untrusted data, what it runs, what it records, and where it keeps its data. The principles behind these rules are on the [Principles](principles.md) page.

## Effect levels

Every action in GitMaster has one of three effect levels.

| Level | What it does | Examples | Status |
|---|---|---|---|
| Read | Looks at the repository and changes nothing. | Status, diff, history, branch list, text search. | Implemented |
| Propose | Computes a plan or a suggestion and changes nothing. | Commit message suggestion from a workflow. Restack plan, conflict suggestion. | Partial |
| Apply | Changes the repository or its Git state. | Commit, checkout, push, delete a branch, remove a worktree. | Partial |

## Who may do what

### Human actor

You act through the window, the menus, the command palette and the keyboard. Each action runs when you start it. Destructive items in menus and the palette are shown in red and ask for confirmation first. Force-deleting a branch needs an explicit confirmation checkbox.

### Agent actor

> **Planned.** Agents that connect through the MCP server are not available in the app yet.

An agent reads the repository and proposes changes. An agent never applies a change without your approval in the app. The policy in your configuration sets which levels an agent may run without asking, and Apply always waits for you. Apply accepts only a plan that you approved, never free-form arguments, so what you approve is exactly what runs. An agent's requests pass through the same checks as yours. See [MCP server](../agents/mcp-server.md) for how agents connect.

## Untrusted data

Some text in a repository is written by people or programs you do not control. GitMaster treats the following as data:

- commit messages
- branch names
- file contents
- the output of commands

The app displays this text, searches it and compares it. It never runs it.

### Text in agent requests

> **Planned.** Agent requests that use repository text are not available in the app yet.

Text from the repository never becomes an argument of an Apply action unless a plan that you approved produced that action. Anyone who can write to a repository can write these strings, and an agent that reads them can be steered by text it did not write. A plan fixes the exact action before you approve it, so the text cannot change that action afterwards.

## What GitMaster runs

### Git

GitMaster runs `git` as a program with a list of arguments, never through a shell. Git runs its own hooks when you commit, such as `pre-commit`, just as it does on the command line.

### Programs you configure

Workflow scripts run in a sandbox, and they act only through the actions the app provides. A workflow step can run a program that you choose, such as a tool that writes a commit message. That program receives an argument list, never a shell, and it is stopped when its run is cancelled. Its output is untrusted data.

## Approvals for workflows

> **Planned.** Automatic approval, and the approval rules for runs started by an event or by an agent, are not available in the app yet.

A workflow script runs only after you have reviewed it. Each version of a script is identified by its content. You review a version the first time it runs, and any edit, by you or by an agent, makes it unreviewed again. A run that you start by hand goes ahead, and a run started by an event waits for your approval at each Apply step, unless you have chosen automatic approval for that reviewed version. After a run has executed a program, its later Apply steps ask again, and the dialog shows the exact arguments. A run started by an agent always waits for you. See [Approvals and security](../workflows/approvals-and-security.md).

## What GitMaster records

### Git commands

Open the **Git commands** panel from the status bar. It lists every Git command that changes something that the app ran: stage, unstage, commit, checkout, fetch, pull, push, and branch and worktree operations. Each entry shows the program, its arguments, the exit code, the duration, standard output and standard error. Reads, such as status, diff and history, are not listed. The panel keeps the last 1000 entries in memory. Nothing is saved to disk, so the list is empty the next time you open the app.

### Operation log

> **Planned.** The operation log is not available in the app yet.

The operation log records each operation that changes a repository, what was asked, and who asked for it: you, or an agent, shown as `agent:<client>` with the name of the client that connected.

## Where data is kept

| Data | Where it is kept | Status |
|---|---|---|
| Working tree | Never receives GitMaster's own files. Changes come only from Git operations you start or approve. | Implemented |
| Recent repositories | The app data directory. Entries for folders that no longer exist are dropped. | Implemented |
| Settings, such as the diff layout and the branch filter | The user config directory, in `settings.json`. | Implemented |
| Git commands panel | Memory only. | Implemented |
| Operation log | Outside the working tree. | Planned |
| Local review comments and their snapshots | Outside the working tree. | Planned |
| Workflow scripts | The workflows directory in your user config: `~/.config/gitmaster/workflows/` on Linux, and `Library/Application Support/gitmaster/workflows/` in your home directory on macOS. Also any directory listed in `gitmaster.workflowPath`. Never a repository. | Implemented |
| Workflow review record | `gitmaster.db` in the app data directory, keyed by the hash of each script version. | Implemented |
| Workflow run history | `.git/gitmaster/state.db`, in the Git common directory of the repository. | Implemented |

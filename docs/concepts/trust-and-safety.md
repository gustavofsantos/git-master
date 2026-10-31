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

An agent that connects through the [MCP server](../agents/mcp-server.md) can read review comments and threads, and record that it handled a review comment. It cannot change the repository through GitMaster: the server has no tool that edits, commits, pushes or deletes. What an agent records is yours to confirm in the app, so a comment it marks `handled` stays open to you until you resolve it.

> **Planned.** Tools that propose and apply a change to the repository. Your agent asks you before each apply call, and apply accepts only a plan, never free-form arguments, so what you approve is exactly what runs. The agent enforces this approval, not GitMaster, so it holds only as long as the agent's own permission settings do. See [Approval](../agents/mcp-server.md#approval).

## Untrusted data

Some text in a repository is written by people or programs you do not control. GitMaster treats the following as data:

- commit messages
- branch names
- file contents
- the output of commands
- the text of threads: your earlier prompts, the agent's replies and the output of its tools

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

### Threads

With the [agent integration](../agents/agent-integration.md) installed, GitMaster records each agent session in the repository as a [thread](../guide/threads.md): your prompts, the agent's replies, its tool calls and results, and every file edit. The record is made from the agent's own hooks and session file, not from a summary the agent writes. Secrets are removed before anything is stored. An agent can read threads through MCP and can never change or delete one.

A thread is an observed record, not a tamper-proof one. An agent that runs as you can write to the same files as any other process.

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
| Threads | `.git/gitmaster/threads.db` in the Git common directory, readable only by you. | |
| Agent integration plugin | The `agents/` folder of the app data directory. | |
| Local review comments and their snapshots | Outside the working tree. | Planned |
| Workflow scripts | The workflows directory in your user config: `~/.config/gitmaster/workflows/` on Linux, and `Library/Application Support/gitmaster/workflows/` in your home directory on macOS. Also any directory listed in `gitmaster.workflowPath`. Never a repository. | Implemented |
| Workflow review record | `gitmaster.db` in the app data directory, keyed by the hash of each script version. | Implemented |
| Workflow run history | `.git/gitmaster/state.db`, in the Git common directory of the repository. | Implemented |

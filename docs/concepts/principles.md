---
title: Principles
description: The rules GitMaster follows in every feature, and what each one means for you.
status: partial
order: 10
---

GitMaster follows a small set of principles. Each one is a rule you can rely on: every feature keeps it, and a feature that would break one does not ship. This page states the rules and what they mean for you. For the safety model that applies them to agents, see [Trust and safety](trust-and-safety.md).

## Determinism

The same inputs give the same results. A plan and its execution produce the same result, and GitMaster does not act on a guess about what is likely to happen. Output from a language model is only ever a proposal. It is shown to you, and it is never applied on its own.

## The human approves and the agent proposes

> **Planned.** Approval of agent changes is not available in the app yet.

An agent can read the repository and propose changes. An operation that an agent asks for and that changes the repository needs your approval in the app, and it runs only the exact change you approved. See [Trust and safety](trust-and-safety.md) for the details.

## Git is the source of truth

GitMaster shows what Git says, and it does not keep a separate copy of the repository state that it trusts over Git. Git changes outside the app all the time: from a terminal, from another tool, or from an agent that runs the `git` command line. GitMaster watches `HEAD`, the index, the refs and the working tree, and refreshes what you see when they change. Stale state is refreshed, not trusted.

For example, if you run `git commit` in a terminal while GitMaster is open, the Working Copy and History update without any action from you.

## The UI never blocks

Work that takes time, such as reading a long history, runs in the background. The window stays responsive while it runs, and long work can be cancelled.

## Nothing is written to the working tree

GitMaster keeps its configuration and state outside the repository: in your user config directory, in the `.git` directory of the repository, or in the app data directory. Your files change only through the Git operations you start or approve. This matters because many work repositories do not allow even a `.gitignore` edit.

## Observed beats reported

> **Planned.** An operation log of every action an agent takes is not available in the app yet. Review comments are the only record that is measured today.

What an agent did is measured from the repository, by comparing its state before and after the work. When an agent marks a review comment as handled, the app measures the change to the lines of that comment from its own snapshots. The measurement is returned to the agent with the result, and the Local review page shows it for a handled comment. The agent's own description of its work is shown as narrative only. You can rely on the measured record more than on the agent's account.

## Local-first

GitMaster is for one engineer on one machine. It has no account, and it has no headless, server or CI mode. The processes that run without a window are its command-line subcommands: `gitmaster mcp`, `gitmaster workflow`, `gitmaster thread` and `gitmaster harness`. They are started by you or by your agent, for the length of one command or one session. `gitmaster mcp` talks over standard input and output and opens no port and no socket.

## Self-contained

The only external program GitMaster needs is `git`. Features that would need another program installed, such as a search tool or a language server, are built into the app instead. Text search, for example, uses `git grep`, which is part of Git.

## Quality bar

The app stays smooth and fast as repositories grow. Long lists draw only the rows you can see, and long work runs in the background. Speed is measured in release builds. A feature that cannot meet this bar is made lighter or left out; it does not ship slow.

## What GitMaster is not

- **A server or a CI tool.** There is no headless, server or CI mode. GitMaster runs in a window on your machine. The MCP server is started by your agent, for the length of its session, and listens on nothing.
- **A web or JavaScript front end.** The interface is native and is not built with web or JavaScript technology.
- **A host for plugins loaded as dynamic libraries.** Extensions run as separate processes, or as workflow scripts.

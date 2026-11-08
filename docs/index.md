---
title: GitMaster
description: What GitMaster is, the three pillars it is built on, who it is for, and where to find each part of the documentation.
status: partial
order: 0
---

GitMaster is a Git client joined to the tools an engineer needs to work with code. It is built so that people who are accountable for their source code can use AI agents safely: an agent proposes a change, you approve it in the app, and the app records what was actually done.

## The three pillars

### A complete and fast Git client

GitMaster covers the work you do every day in a repository. The **Working Copy** shows your changes, with staging per file, per chunk and per line, and a commit composer. **History** shows the commit graph with the files and diff of each commit. Branches and worktrees are managed from the sidebar, and the remote operations **Fetch**, **Pull** and **Push with lease** run from the sync button in the top bar. Every action can be reached from the command palette (`Ctrl+K`, `⌘K` on macOS), the menus and the keyboard.

Resolving conflicts by hand is a preview feature. See [Resolving conflicts](guide/conflicts.md).

### A workspace for working with code

You can move around a repository without leaving the app. **Quick Open** (`Ctrl+P`, `⌘P` on macOS) finds a file by name in the active worktree. The **Code** page shows the folder tree and a read-only file viewer on its **Files** tab. Its **Search** tab runs a text search with case, whole word and regular expression options, and groups the matches by file.

### Agents under human accountability

An agent can read your review comments and earlier threads through GitMaster, record that it handled a comment, and reply on the comment's thread. It can also attach labels to threads and link related threads. These records are kept in GitMaster's own store, never in the repository. GitMaster gives the agent no way to change the repository, and a comment the agent marks as handled stays open to you until you resolve it. What the agent did is measured from the repository, not taken from its own account.

Workflows are Lua scripts that run in a sandbox and act only through GitMaster's own actions. You run them from the command palette or with a keyboard shortcut. The agent integration is exposed through an MCP server and a local review loop. Branch stacks with restack are a preview feature. See [Stacks](guide/stacks.md).

A session with an agent is kept as a **thread**: what you asked, what the agent answered, the tools it ran and the files it edited. Secrets are removed before anything is stored, and you can search, read and delete threads in the app.

> **Planned.** Tools that propose and apply a change through the MCP server, and workflow runs that an agent starts, are not available in the app yet.

## Who it is for

GitMaster is for engineers who work in large repositories with many contributors, and who review the changes that AI agents make to their code in volume. It is built for one engineer on one machine. It has no account and no server mode.

## What you need

GitMaster needs one external program: `git`. The app does not require anything else to be installed.

GitMaster runs on Linux and macOS. Each release has prebuilt Linux binaries for amd64 and arm64; on macOS you build it from source. To install it, see [Installation](getting-started/installation.md).

### Windows

> **Planned.** Windows is not supported yet.

## Where to go next

| Area | Start with | Covers |
|---|---|---|
| Getting started | [Installation](getting-started/installation.md) | Installing GitMaster on macOS or Linux. |
| Getting started | [First steps](getting-started/first-steps.md) | Opening a repository and getting around the window. |
| Concepts | [Principles](concepts/principles.md) | The rules every feature follows. |
| Concepts | [Trust and safety](concepts/trust-and-safety.md) | Who may change a repository, and what the app records. |
| Guide | [Working Copy](guide/working-copy.md) | Changes, staging and commits. |
| Guide | [Diff and staging](guide/diff-and-staging.md) | Reading diffs, and staging chunks and lines. |
| Guide | [History](guide/history.md) | The commit graph and commit details. |
| Guide | [Branches](guide/branches.md) | Switching, creating and deleting branches. |
| Guide | [Stacks](guide/stacks.md) | Branch stacks and restack. Preview feature. |
| Guide | [Resolving conflicts](guide/conflicts.md) | Resolving a file's conflicts by hand. Preview feature. |
| Guide | [Command palette](guide/command-palette.md) | Finding and running any action. |
| Guide | [Threads](guide/threads.md) | Reading and searching your agent sessions. |
| Workflows | [Overview](workflows/overview.md) | Automating GitMaster with workflow scripts. |
| Agents | [Agent integration](agents/agent-integration.md) | Capturing agent sessions as threads. |
| Agents | [MCP server](agents/mcp-server.md) | Connecting an agent to the app. |
| Agents | [Local review](agents/local-review.md) | Commenting on code for an agent to resolve. |
| Reference | [Keyboard shortcuts](reference/keyboard-shortcuts.md) | Every shortcut in one table. |
| Reference | [Configuration](reference/configuration.md) | Settings and where they are stored. |

## How to read the status markers

Each page has a `status` in its front matter:

| Status | Meaning |
|---|---|
| `implemented` | Everything on the page works in the app. |
| `partial` | Some of the page works. Each part that does not is marked. |
| `planned` | Nothing on the page works yet. |

A part that the app does not provide yet is marked with a **Planned** label. The label is a blockquote that starts with the bold text **Planned.**, followed by one sentence that says what is missing. It sits directly under the heading of the part it applies to. In a table, a row that the app does not provide has `Planned` in its status column.

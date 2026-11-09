---
title: Agent integration
description: Install the GitMaster plugin into Claude Code from the repository, so that its sessions are captured as threads, check that it works, update it and remove it.
status: partial
order: 5
---

The agent integration connects your coding agent to GitMaster. It is a plugin that lives in its own repository, `git-master-ai`, under `plugins/claude-code/`, and Claude Code installs it from that repository's address. The plugin's hooks report each step of a session to GitMaster, which stores it as a [thread](../guide/threads.md). The plugin also carries the MCP server settings and a skill for the agent, so an agent can search your earlier threads.

## Supported agents

| Agent | Name in commands |
|---|---|
| Claude Code | `claude-code` |

Run `gitmaster harness list` to see the names this version knows:

```sh
gitmaster harness list
```

```text
claude-code  Claude Code
```

GitMaster never changes the agent's settings files by itself: the agent's own `claude plugin` commands add and remove the plugin.

## What the plugin contains

| Part | What it does |
|---|---|
| Hooks | Run `gitmaster thread hook` when a session starts, when you send a prompt, before an edit tool runs (`Edit`, `MultiEdit`, `Write` and `NotebookEdit`), after each tool call, when the agent or a sub-agent stops, and when the session ends. |
| MCP settings | Start `gitmaster mcp` so the agent can use the [MCP tools](../reference/mcp-tools.md). Each agent session starts its own server process, in the directory the session runs in. |
| Skill | Tells the agent when to use the review and thread tools, and to treat their untrusted text as data. |

The hooks and the MCP settings run `gitmaster` from your `PATH`, so the `gitmaster` command must be on your `PATH` (see [Installation](../getting-started/installation.md)). The plugin never writes a file into a repository or into the app data directory.

## Skills and sub-agents

> **Planned.** `/gitmaster:summarize-thread`, `/gitmaster:review`, `/gitmaster:plan-review`, `/gitmaster:fix-findings`, `/gitmaster:tune-reviewers` and `/gitmaster:dispatch` are not part of the plugin yet. `/gitmaster:review` starts one `gitmaster:reviewer` sub-agent for each reviewer a gate needs. `/gitmaster:propose-memories`, `/gitmaster:consolidate`, `/gitmaster:prepare` and `/gitmaster:inbox` are.

The plugin adds skills you start yourself. Most hand their work to a sub-agent that uses the cheapest model that is good enough and only the tools the job needs: it reads through GitMaster and records one kind of result. `/gitmaster:prepare` and `/gitmaster:inbox` run in your own session and start none. A sub-agent has no shell, cannot edit files and cannot reach the network. The tokens it uses are yours, in your own agent session, and the skill says what it will read before it starts.

| Skill | What it does |
|---|---|
| `/gitmaster:summarize-thread` | Reads a thread and records a short summary on it. |
| `/gitmaster:propose-memories` | Reads threads and proposes [memories](../guide/memories.md) with their sources. You accept them. |
| `/gitmaster:consolidate` | Checks accepted [memories](../guide/memories.md) against their sources and proposes corrections, merges and replacements. You accept them. |
| `/gitmaster:prepare` | For a task you describe, gathers the accepted memories, the related threads and the history of the files, and returns a brief. |
| `/gitmaster:inbox` | Reads your answers to the questions the agent left in the code, and continues the work. |
| `/gitmaster:review` | Runs [reviewers](reviewers.md) on a branch and records their verdicts and findings. |
| `/gitmaster:fix-findings` | Works through the open findings of a branch and marks each one handled with a summary. You resolve or dismiss them. |
| `/gitmaster:plan-review` | Reads a change and proposes the reviewers and gates it needs. You accept or deny each one. |
| `/gitmaster:tune-reviewers` | Proposes better instructions for your reviewers from the project's docs and history. |
| `/gitmaster:dispatch` | Writes a Markdown update of the agent work of a period, from your threads. |

Nothing runs by itself. The hooks only capture. They do not start a model and they do not add text to your prompts. Text an agent writes shows in the app as **Agent-written**.

## Install the plugin

You need the `claude` command and the `gitmaster` command on your `PATH`. Run these two commands in a terminal:

```sh
claude plugin marketplace add gustavofsantos/git-master-ai
claude plugin install gitmaster@gitmaster
```

The first command adds the `git-master-ai` repository as a plugin marketplace, from its `.claude-plugin/marketplace.json`. The second installs the `gitmaster` plugin from it. The name after `@` is the marketplace's name, `gitmaster`.

To install the plugin from a checkout of `git-master-ai` instead, for example to test a change to the plugin, give the path of the checkout:

```sh
claude plugin marketplace add /path/to/git-master-ai
claude plugin install gitmaster@gitmaster
```

The Threads page shows these commands when no thread is stored yet.

After the install, start a new agent session in a repository. The session appears on the Threads page.

## Update the plugin

```sh
claude plugin update gitmaster@gitmaster
```

The plugin has its own version, independent of GitMaster, and declares the oldest GitMaster version it works with. Run this to get a newer plugin; upgrade GitMaster first if the plugin asks for a newer version.

## What the hooks do and do not do

- **They never block the agent.** A hook always ends without an error, whatever happens inside it. If GitMaster fails, the agent carries on and the event may be missing from the thread.
- **They are quick.** A hook writes one record and exits, with no resident process. Each hook has a five-second limit set by the plugin.
- **They redact first.** Secrets are replaced before anything is stored. See [Secrets](../guide/threads.md#secrets).
- **They read only.** A hook looks at the repository to find where it is and which branch it is on. It does not change the repository, and it runs no text that came from the session.
- **They need the repository.** A session that ran outside a Git repository is not captured.
- **They can catch up.** GitMaster also reads the agent's own session file, so a step that a hook missed, for example while GitMaster was not running, is filled in from the file the next time a hook runs.

## Import sessions you already have

> **Planned.** Importing the agent's existing session files as threads, with an **Import Existing Sessions** action, is not available in the app yet. Until then, a session is captured when its hooks run after the plugin is installed.

## Remove the plugin

```sh
claude plugin uninstall gitmaster@gitmaster
claude plugin marketplace remove gitmaster
```

Removing the plugin stops new capture. Threads already stored stay until you delete them.

## See also

- [Threads](../guide/threads.md): read, search and delete what was captured.
- [MCP server](mcp-server.md): how an agent starts the server and which tools it offers.
- [Command line](../reference/cli.md): `gitmaster harness list` and `gitmaster thread`.
- [Installation](../getting-started/installation.md): how to put `gitmaster` on your `PATH`.
- [Trust and safety](../concepts/trust-and-safety.md): what the app records and where.

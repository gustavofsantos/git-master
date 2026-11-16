---
title: Agent integration
description: Install the GitMaster plugin into Claude Code or Cursor from the repository, so that its sessions are captured as threads, check that it works, update it and remove it.
status: partial
order: 5
---

The agent integration connects your coding agent to GitMaster. It is a plugin that lives in its own repository, `git-master-ai`, with one folder per agent: `plugins/claude-code/` and `plugins/cursor/`. Claude Code installs it from that repository's address; Cursor installs it from a checkout of the repository with one command. The plugin's hooks report each step of a session to GitMaster, which stores it as a [thread](../guide/threads.md). The plugin also carries the MCP server settings, skills and sub-agents, so an agent can search your earlier threads.

## Supported agents

| Agent | Name in commands |
|---|---|
| Claude Code | `claude-code` |
| Cursor | `cursor` |

Run `gitmaster harness list` to see the names this version knows:

```sh
gitmaster harness list
```

```text
claude-code  Claude Code
cursor  Cursor
```

GitMaster never changes the agent's settings files by itself: Claude Code's own `claude plugin` commands add and remove its plugin, and for Cursor a script in `git-master-ai` copies the plugin into Cursor's folder of local plugins and removes it from there.

## What the plugin contains

| Part | What it does |
|---|---|
| Hooks | Run `gitmaster thread hook` when a session starts, when you send a prompt, before an edit tool runs (Claude Code: `Edit`, `MultiEdit`, `Write` and `NotebookEdit`; Cursor: `Write` and `Delete`), after each tool call, when the agent or a sub-agent stops, and when the session ends. In Cursor they also run when a tool call fails and when the agent replies. |
| MCP settings | Start `gitmaster mcp` so the agent can use the [MCP tools](../reference/mcp-tools.md). Each agent session starts its own server process, in the directory the session runs in. Cursor's plugin also names the workspace folder in `GITMASTER_PROJECT_DIR`, which the server uses when that directory is not in a repository. |
| Skills and sub-agents | The `gitmaster` skill tells the agent when to use the review and thread tools, and to treat their untrusted text as data. The others are listed below. |

The hooks and the MCP settings run `gitmaster` from your `PATH`, so the `gitmaster` command must be on your `PATH` (see [Installation](../getting-started/installation.md)). The plugin never writes a file into a repository or into the app data directory.

## Skills and sub-agents

> **Planned.** `/gitmaster:summarize-thread` and `/gitmaster:dispatch` are not part of the plugin yet. `/gitmaster:write-reviewer` is planned too: it helps you and your agent write a [review workflow](reviewers.md). `/gitmaster:propose-memories`, `/gitmaster:consolidate`, `/gitmaster:prepare` and `/gitmaster:inbox` are part of the plugin.

The plugin adds skills you start yourself. In Cursor, plugin skills are not namespaced, so each one is named with a `gitmaster-` prefix instead: `/gitmaster:prepare` in Claude Code is `/gitmaster-prepare` in Cursor, and the same for every skill below. Most hand their work to a sub-agent that uses the cheapest model that is good enough and only the tools the job needs: it reads through GitMaster and records one kind of result. `/gitmaster:prepare` and `/gitmaster:inbox` run in your own session and start none. In Claude Code a sub-agent has no shell, cannot edit files and cannot reach the network. Cursor cannot limit a sub-agent's tools: its sub-agents are read-only, so they cannot edit files or run commands that change anything, and their instructions allow only GitMaster tools. The tokens it uses are yours, in your own agent session, and the skill says what it will read before it starts.

| Skill | What it does |
|---|---|
| `/gitmaster:summarize-thread` | Reads a thread and records a short summary on it. |
| `/gitmaster:propose-memories` | Reads threads and proposes [memories](../guide/memories.md) with their sources. You accept them. |
| `/gitmaster:consolidate` | Checks accepted [memories](../guide/memories.md) against their sources and proposes corrections, merges and replacements. You accept them. |
| `/gitmaster:prepare` | For a task you describe, gathers the accepted memories, the related threads and the history of the files, and returns a brief. |
| `/gitmaster:inbox` | Reads your answers to the questions the agent left in the code, and continues the work. |
| `/gitmaster:dispatch` | Writes a Markdown update of the agent work of a period, from your threads. |

Nothing runs by itself. The hooks only capture. They do not start a model and they do not add text to your prompts. Text an agent writes shows in the app as **Agent-written**.

## Install the plugin in Claude Code

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

## Install the plugin in Cursor

You need a checkout of `git-master-ai`, `jq`, and the `gitmaster` command on your `PATH`. Run in a terminal:

```sh
git clone https://github.com/gustavofsantos/git-master-ai
git-master-ai/scripts/install-local.sh cursor
```

The script copies `plugins/cursor/` to `~/.cursor/plugins/local/gitmaster/`, the folder where Cursor loads local plugins for every workspace. It copies rather than links, because Cursor skips a link that points outside that folder. It warns when `gitmaster` is not on your `PATH` or is older than the plugin needs. `--dir <folder>` installs into another folder of local plugins.

Then reload Cursor (**Developer: Reload Window**) and open **Customize**: the `gitmaster` skills, sub-agents, hooks and MCP server are listed there. Start a new agent session in a repository; it appears on the Threads page. On an Enterprise plan, local plugins load only when an administrator allows **Local Plugin Imports**, and a marketplace plugin with the same name takes precedence over the local copy.

Cursor asks before it uses an MCP tool the first time, as it does for any MCP server.

## Update the plugin

In Claude Code:

```sh
claude plugin update gitmaster@gitmaster
```

In Cursor, pull the checkout and run the script again; it replaces the copy:

```sh
git -C git-master-ai pull
git-master-ai/scripts/install-local.sh cursor
```

The plugin has its own version, independent of GitMaster, and declares the oldest GitMaster version it works with. Run this to get a newer plugin; upgrade GitMaster first if the plugin asks for a newer version.

## What the hooks do and do not do

- **They never block the agent.** A hook always ends without an error, whatever happens inside it. For Cursor, which reads a hook's output, it prints an empty answer that allows the action and asks for nothing. If GitMaster fails, the agent carries on and the event may be missing from the thread.
- **They are quick.** A hook writes one record and exits, with no resident process. Each hook has a five-second limit set by the plugin.
- **They redact first.** Secrets are replaced before anything is stored. See [Secrets](../guide/threads.md#secrets).
- **They read only.** A hook looks at the repository to find where it is and which branch it is on. It does not change the repository, and it runs no text that came from the session.
- **They need the repository.** A session that ran outside a Git repository is not captured.
- **They can catch up.** For Claude Code, GitMaster also reads the agent's own session file, so a step that a hook missed, for example while GitMaster was not running, is filled in from the file the next time a hook runs.

## Import sessions you already have

> **Planned.** Importing the agent's existing session files as threads, with an **Import Existing Sessions** action, is not available in the app yet. Until then, a session is captured when its hooks run after the plugin is installed.

## Remove the plugin

In Claude Code:

```sh
claude plugin uninstall gitmaster@gitmaster
claude plugin marketplace remove gitmaster
```

In Cursor, then reload the window:

```sh
git-master-ai/scripts/install-local.sh cursor --uninstall
```

Removing the plugin stops new capture. Threads already stored stay until you delete them.

## See also

- [Threads](../guide/threads.md): read, search and delete what was captured.
- [MCP server](mcp-server.md): how an agent starts the server and which tools it offers.
- [Command line](../reference/cli.md): `gitmaster harness list` and `gitmaster thread`.
- [Installation](../getting-started/installation.md): how to put `gitmaster` on your `PATH`.
- [Trust and safety](../concepts/trust-and-safety.md): what the app records and where.

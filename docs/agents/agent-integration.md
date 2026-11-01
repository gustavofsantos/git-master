---
title: Agent integration
description: Install the GitMaster plugin into Claude Code from the repository, so that its sessions are captured as threads, check that it works, update it and remove it.
status: implemented
order: 5
---

The agent integration connects your coding agent to GitMaster. It is a plugin that lives in the GitMaster repository, under `plugins/claude-code/`, and Claude Code installs it from the repository's address. The plugin's hooks report each step of a session to GitMaster, which stores it as a [thread](../guide/threads.md). The plugin also carries the MCP server settings and a skill for the agent, so an agent can search your earlier threads.

## Supported agents

| Agent | Name in commands |
|---|---|
| Claude Code | `claude-code` |

Run `gitmaster harness list` to see the names this version knows. GitMaster never changes the agent's settings files by itself: the agent's own `claude plugin` commands add and remove the plugin.

## What the plugin contains

| Part | What it does |
|---|---|
| Hooks | Run `gitmaster thread hook` when a session starts and ends, when you send a prompt, before and after each tool call, and when the agent stops. |
| MCP settings | Start `gitmaster mcp` so the agent can use the [MCP tools](../reference/mcp-tools.md). Each agent session starts its own server process, in the directory the session runs in. |
| Skill | Tells the agent when to use the review and thread tools, and to treat their untrusted text as data. |

The hooks and the MCP settings run `gitmaster` from your `PATH`, so the `gitmaster` command must be on your `PATH` (see [Installation](../getting-started/installation.md)). The plugin never writes a file into a repository or into the app data directory.

## Install the plugin

You need the `claude` command and the `gitmaster` command on your `PATH`. Run these two commands in a terminal:

```sh
claude plugin marketplace add gustavofsantos/git-master
claude plugin install gitmaster@gitmaster
```

The first command adds the repository as a plugin marketplace, from its `.claude-plugin/marketplace.json`. The second installs the `gitmaster` plugin from it. The name after `@` is the marketplace's name, `gitmaster`.

To install the plugin from a checkout of the repository instead, for example to test a change to the plugin, give the path of the checkout:

```sh
claude plugin marketplace add /path/to/git-master
claude plugin install gitmaster@gitmaster
```

The Threads page shows these commands when no thread is stored yet.

After the install, start a new agent session in a repository. The session appears on the Threads page.

## Update the plugin

```sh
claude plugin update gitmaster@gitmaster
```

Run this after you upgrade GitMaster, so that the plugin matches the new version.

## What the hooks do and do not do

- **They never block the agent.** A hook always ends without an error, whatever happens inside it. If GitMaster fails, the agent carries on and the event may be missing from the thread.
- **They are quick.** A hook writes one record and exits, with no resident process.
- **They redact first.** Secrets are replaced before anything is stored. See [Secrets](../guide/threads.md#secrets).
- **They read only.** A hook looks at the repository to find where it is and which branch it is on. It does not change the repository, and it runs no text that came from the session.
- **They need the repository.** A session that ran outside a Git repository is not captured.
- **They can catch up.** GitMaster also reads the agent's own session file, so a step that a hook missed, for example before you installed the plugin, is filled in from the file.

## Import sessions you already have

Choose **Import Existing Sessions** on the Threads page to read the agent's session files for the open repository and add them as threads. The same redaction applies. A session that is already a thread is not duplicated.

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

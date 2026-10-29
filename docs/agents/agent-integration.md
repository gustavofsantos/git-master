---
title: Agent integration
description: Install the GitMaster plugin into your coding agent so that its sessions are captured as threads, check that it works, and remove it.
status: planned
order: 5
---

> **Planned.** The agent integration is not available in the app yet.

The agent integration connects your coding agent to GitMaster. It is a plugin that GitMaster generates for the agent. The plugin's hooks report each step of a session to GitMaster, which stores it as a [thread](../guide/threads.md). The plugin also carries the MCP server settings and a skill for the agent, so an agent can search your earlier threads.

## Supported agents

| Agent | Name in commands |
|---|---|
| Claude Code | `claude-code` |

Run `gitmaster harness list` to see the names this version knows. GitMaster does not need the agent to be installed, and it never changes the agent's settings files by itself.

## What the plugin contains

| Part | What it does |
|---|---|
| Hooks | Run `gitmaster thread hook` when a session starts and ends, when you send a prompt, before and after each tool call, and when the agent stops. |
| MCP settings | Start `gitmaster mcp` so the agent can use the [MCP tools](../reference/mcp-tools.md). |
| Skill | Tells the agent when and how to search threads. |

> **Planned.** The MCP settings and the skill are not part of the plugin yet. The plugin carries the hooks only.

GitMaster writes the plugin into its own data directory, `agents/claude-code/` under the user data directory. It never writes a file into a repository.

## Install the integration

Open the **Threads** page and choose **Install Agent Integration**, or run **Install Agent Integration** from the command palette. From a terminal:

```sh
gitmaster harness install claude-code
```

GitMaster generates the plugin and shows the exact commands that add it to the agent. Run them yourself, or, if the `claude` command is on your `PATH`, choose **Run Commands** and GitMaster runs them for you. Nothing is changed in the agent until you do one of these.

After the install, start a new agent session in a repository. The session appears on the Threads page.

## Check the integration

```sh
gitmaster harness status claude-code
```

The status reports:

- whether the plugin is generated and up to date with the app;
- whether the hooks point at the `gitmaster` command that is running. If you moved or reinstalled GitMaster, the hooks name a path that no longer exists and the status says **stale path**. Run the install again.
- whether capture is paused, and why;
- how many events were skipped because capture was paused, and how many are waiting to be written.

The Threads page shows the same information in its empty state and in a banner when something needs attention.

## What the hooks do and do not do

- **They never block the agent.** A hook always ends without an error, whatever happens inside it. If GitMaster fails, the agent carries on and the event may be missing from the thread.
- **They are quick.** A hook writes one record and exits, with no resident process.
- **They redact first.** Secrets are replaced before anything is stored. See [Secrets](../guide/threads.md#secrets).
- **They read only.** A hook looks at the repository to find where it is and which branch it is on. It does not change the repository, and it runs no text that came from the session.
- **They need the repository.** A session that ran outside a Git repository is not captured.
- **They can catch up.** GitMaster also reads the agent's own session file, so a step that a hook missed, for example before you installed the plugin, is filled in from the file.

## Import sessions you already have

Choose **Import Existing Sessions** on the Threads page to read the agent's session files for the open repository and add them as threads. The same redaction applies. A session that is already a thread is not duplicated.

## Remove the integration

```sh
gitmaster harness uninstall claude-code
```

This shows the commands that remove the plugin from the agent, in the same way as the install. Removing the plugin stops new capture. Threads already stored stay until you delete them.

## See also

- [Threads](../guide/threads.md): read, search and delete what was captured.
- [MCP server](mcp-server.md): how an agent connects to the app.
- [Command line](../reference/cli.md): `gitmaster harness` and `gitmaster thread`.
- [Trust and safety](../concepts/trust-and-safety.md): what the app records and where.

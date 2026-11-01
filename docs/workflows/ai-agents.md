---
title: Writing workflows with an AI agent
description: How an AI coding agent writes a workflow script, which generated API files it reads, how it tests the script without the app window, and how you review and run the result.
status: partial
order: 50
---

An AI coding agent with access to your files can write a workflow script. GitMaster has no MCP tool that creates or edits scripts. The agent writes a `.lua` file in your workflows directory, as it would any other text file. You stay in charge: a script that has changed is unreviewed until you run it and confirm **Run this version?**. See [Approvals and security](approvals-and-security.md).

## Where the agent writes

- Put the file in your workflows directory: `~/.config/gitmaster/workflows/` on Linux, or `Library/Application Support/gitmaster/workflows/` in your home folder on macOS. Folders listed in `gitmaster.workflowPath` are read too. See [Writing workflows](writing-workflows.md).
- One workflow is one `.lua` file that returns a module with `meta` and `execute`.
- A `.toml` file in the workflows directory is ignored, and GitMaster shows a warning for it.
- A folder whose name starts with a dot is not read as a workflow source.

## The generated API files

Each time the app starts with a new version, it writes a `.api/` folder inside the workflows directory. The agent reads these files before it writes a script:

| File | Content |
|---|---|
| `API.md` | The reference of `ctx`, generated from the action catalog: each call, its arguments, effect level and outcomes. It also has the shape of `M.meta`, the phases and examples. The same reference is in [Workflow API](../reference/workflow-api.md). |
| `ctx.d.luau` | The same `ctx` signatures as Luau types, for an editor with a Luau language server. |
| `SKILL.md` | A short skill file for agents that read skills. |

The app writes these files and replaces them when the version changes, so do not edit them.

If a script calls a name that is not in `ctx`, the call fails with the list of valid names at that level. The agent can correct the script from that message.

## Testing without the app window

The agent tests a script with three one-shot commands. None of them starts a resident process, and none of them applies a change. See [Command line](../reference/cli.md) for the arguments.

| Command | Use |
|---|---|
| `gitmaster workflow check <file>` | Compiles the script, checks `M.meta`, the trigger and the inputs. Each problem is printed as `file:line: message`. A non-zero exit code means an error. |
| `gitmaster workflow dry-run <file> [--input k=v] [--repo <path>]` | Runs the script against the repository. Reads are real. Calls that propose, apply or run a program are recorded, and each returns a result of the same shape, so the script keeps going. Programs never run. The command prints the list of calls. Anything that follows a stubbed result is an estimate, and it is marked. |
| `gitmaster workflow test <file> [fixture]` | Runs the script against canned answers from a fixture file, and checks the sequence of calls and the value the script returns. |

Effects run only from the app, with you as the actor. The command line never applies a change.

## A suggested loop

1. The agent reads `API.md` and writes the script.
2. Run `gitmaster workflow check` on the file and fix every error it reports.
3. Run `gitmaster workflow dry-run` and read the list of calls. Check the estimates.
4. Write a fixture file with a canned answer for each call and the sequence of calls the script should make. Run `gitmaster workflow test` until it passes.
5. Open GitMaster and review the script in the app. Confirm **Run this version?**, then start the run. A manual run applies its changes.

## Limits for agents

- An agent cannot approve anything for you. Approval dialogs appear in the app.
- A script that an agent wrote is unreviewed until you review it. Its first manual run shows you the script before anything happens.

> **Planned.** An agent cannot start a run through the MCP `workflow_run` tool yet. Every applying action of an agent's run will wait for you. See [MCP tools](../reference/mcp-tools.md).

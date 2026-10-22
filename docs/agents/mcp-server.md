---
title: MCP server
description: How AI agents connect to GitMaster through MCP, and how approvals keep you in charge of every change an agent makes.
status: planned
order: 10
---

> **Planned.** The MCP server is not available in the app yet.

GitMaster runs its MCP server inside the running app. An agent connected to it sees the same repository state you see, and every change it applies waits for your approval in the UI.

## How the server is reached

The server runs inside the app, so you and the agent share one view of the repository and one set of approvals. There is no headless mode, no separate server process and no account.

`gitmaster mcp` is a stdio shim. Your MCP client starts it, the shim forwards each request to the app over a Unix socket protected by a token, and the shim opens the app if the app is closed. A token-protected socket is used instead of HTTP on localhost because any local process can reach a TCP port, and web pages can reach one through DNS rebinding.

Configure your MCP client to start this command as a stdio server:

```sh
gitmaster mcp
```

See [CLI](../reference/cli.md) for the command.

## Set up the server

1. Press `Ctrl+,` (`⌘,` on macOS) to open **Settings**, then choose **MCP & Agents**.
2. Under **MCP Server**, turn the server on. Set the path of the Unix socket if you need a different one.
3. Under **Authentication**, generate a token for the `gitmaster mcp` shim.
4. Give the token to the shim, and start your MCP client with it.

To remove access, go back to **Authentication** and revoke the token.

## Effect levels

Every tool has one of three effect levels. The level decides whether a call needs your approval.

| Level | What it does | Approval |
|---|---|---|
| Read | Reads repository state. Changes nothing. | Not required |
| Propose | Computes a plan or a suggestion. Changes nothing. | Not required. A suggestion takes effect only when you accept it. |
| Apply | Changes the repository. | You approve in the UI before it runs. |

The sequence for a change is:

```text
Read  →  Propose  →  your approval in the UI  →  Apply
```

An apply tool accepts only a `plan_id`, never free arguments. What you approve is the plan, so exactly what runs is what you approved.

The agent goes through the same hooks and gates that you do. Every change an agent applies is recorded in the operation log with `actor: agent:<client>`, where `<client>` names the MCP client.

## Policy

The policy decides which tools run without asking you. Set it under **Agent Policies** in the **MCP & Agents** section of Settings. You can also write it in your Git configuration, in the same style as the hook sections:

```ini
[gitmaster "mcp"]
    auto = read, propose
    approve = apply
```

With this policy, Read and Propose run without asking, and Apply waits for your approval in the UI. When no UI is open, an apply stays `pending_approval` until you open the app and approve it.

## Output designed for agents

- **Small output by default.** An agent reads `diff_stat` first, then asks for one hunk with `diff_get`.
- **Hunk ids come from the content.** A hunk id is a hash of the hunk's content. When a hunk changes, its id changes and the call fails, so an agent cannot apply a change to the wrong place.
- **Tasks, not Git commands.** The server offers fewer than about a dozen tools, each built for a task. It does not repeat what the `git` command line already does well, such as `log` and `show`.

## Untrusted content

Commit messages, branch names and file contents come from the repository, and an agent must treat them as untrusted. The server returns them in fields marked as untrusted. No value from the repository becomes an argument to an apply tool unless it passed through a plan that you approve.

## Agents and workflows

An agent runs a workflow with `workflow_run`. Every applying step of that run waits for your approval, with no override.

Agents write workflow scripts as ordinary files. The MCP server has no tool that writes them. See [AI agents](../workflows/ai-agents.md) for how to write a workflow that an agent can run.

## Worked example: an agent restacks a stack

An agent restacks the stack `ledger-base`, which has three branches out of date. The restack stops at a conflict in `ledger-ui`.

```text
stack_tree()                      -> ledger-base -> [api, ui -> [e2e]], 3 branches out of date
restack_plan(root="ledger-base")  -> { plan_id: "01J9Z...", steps: 3, refs: {...} }
restack_apply(plan_id)            -> { status: "pending_approval", operation_id }
                                     ... you approve in the UI ...
                                  -> { status: "conflict", branch: "ledger-ui", hunks: ["h_7f3a", "h_c21b"] }
conflict_get("h_7f3a")            -> base, left side, right side
conflict_propose("h_7f3a", text)  -> suggestion appears in the UI; you accept it
```

1. `stack_tree` shows the stack and the branches that are out of date. Reading changes nothing.
2. `restack_plan` builds a plan and returns its `plan_id`. Planning changes nothing.
3. `restack_apply` asks to run the plan. Under the example policy the apply waits, so the call returns `pending_approval` and an operation id. You approve in the UI.
4. The restack reaches `ledger-ui` and stops at a conflict. The call returns the branch and the ids of its conflicting hunks.
5. `conflict_get` returns the base and both sides of the first hunk.
6. `conflict_propose` sends a suggested resolution to the UI. Nothing changes until you accept it.

## See also

- [MCP tools](../reference/mcp-tools.md): every tool, with its arguments, results and approval.
- [Local review](local-review.md): how an agent works through the comments you write on a diff.
- [Trust and safety](../concepts/trust-and-safety.md): the model behind approvals and the operation log.

---
title: MCP server
description: How AI agents use GitMaster through MCP, which tools it offers, and how it keeps you in charge of what the agent records.
status: partial
order: 10
---

> **Planned.** Only the review and thread tools are available. The tools that propose or apply a change to the repository, and the agent policy, are not.

`gitmaster mcp` is an MCP server that your agent starts as a separate process and talks to over its standard input and output. It offers the tools that only GitMaster can offer: the review comments you wrote on a diff, and the threads GitMaster captured from earlier agent sessions. It does not repeat what the agent already has. Reading files, searching text and running `git` stay with the agent's own tools.

## How the server runs

Your MCP client starts `gitmaster mcp` as a stdio server and stops it when the session ends. There is no socket, no port, no token and no resident process: the server lives exactly as long as the client that started it. The app does not need to be open.

Configure your MCP client to start this command:

```sh
gitmaster mcp
```

The server works on the repository that contains its working directory, which is the directory the client starts it in. To name another repository, pass its path:

```sh
gitmaster mcp --repo ~/src/project
```

See [CLI](../reference/cli.md) for the command.

The server and the app share what they store in the repository's `.git/gitmaster/` directory. A comment the agent marks as handled appears in the app, and a comment you write in the app is there the next time the agent lists comments. Neither one waits for the other.

## Several servers at once

Each agent session starts its own `gitmaster mcp`, so several can run on the same repository at the same time, in the same worktree or in different worktrees, and next to the app. They do not talk to each other and none of them blocks the others. Review comments and threads belong to the repository, so every worktree sees the same ones, and a comment handled by one server is handled for all. The comment's lines are found in the files of the worktree the server runs in.

When two servers write the same comment at the same time, the last summary stays. A comment you resolved stays resolved: no server can mark it handled again.

## What the server may do

The server offers two kinds of tool:

| Kind | What it does | Approval |
|---|---|---|
| Read | Reads review comments and threads. Changes nothing. | Not required |
| Record | Writes to GitMaster's own store only: it marks a review comment as handled, with a summary. It never changes your files, your branches or your index. | Not required. You confirm the result in the app. |

A record tool takes effect on GitMaster's record, and a person still decides what it means. When an agent marks a comment as `handled`, the comment waits for you. Only you make it `resolved`.

There is no tool that commits, pushes, restacks, edits a file or deletes anything. An agent that changes your code does it with its own tools, as it does without GitMaster, and the [thread](../guide/threads.md) records what it did.

> **Planned.** Tools that propose a change, such as a restack plan, and that apply it after your approval in the app.

## Output designed for agents

- **Small output by default.** Lists return a page, and the thread tools take a limit. An agent reads a thread's summary first, then asks for the entries it needs.
- **Tasks, not Git commands.** The server offers a handful of tools, each built for a task.

## Untrusted content

Review comments, thread entries and file text come from people and from earlier sessions, so an agent must treat them as untrusted. The server returns them in fields marked as untrusted. Text that GitMaster redacted shows as `[REDACTED:name]`, and the original is not available to the agent.

A change an agent records is written to the operation log with `actor: agent:<client>`, where `<client>` is the name the MCP client gave when it connected. Reads are not logged.

## Worked example: an agent works through your comments

You commented on two lines of `src/ledger.rs` in the app: "Return an error here instead of unwrapping". The agent handles it:

```text
review_comments_list(status="open")  -> [{ id: 7, path: "src/ledger.rs", lines: 41-42, body: "Return an error ...", untrusted: true }]
                                        ... the agent edits src/ledger.rs with its own tools ...
review_comment_resolve(id=7, summary="parse_amount now returns Result; callers propagate with ?")
                                     -> { id: 7, status: "handled" }
```

1. `review_comments_list` returns the open comments with the lines they are anchored to now. A comment whose text no longer appears exactly once is `outdated` and has no lines.
2. The agent changes the code with its own tools.
3. `review_comment_resolve` marks the comment `handled` and stores the agent's summary. The comment appears as handled in the app, and you decide whether to resolve it.

An agent that needs an earlier decision searches the threads instead:

```text
threads_search(query="retry backoff") -> [{ thread: "01J9Z...", turn: 4, snippet: "...we chose jittered backoff because...", untrusted: true }]
thread_read(id="01J9Z...", from=4, limit=10)
```

## See also

- [MCP tools](../reference/mcp-tools.md): every tool, with its arguments and results.
- [Local review](local-review.md): how an agent works through the comments you write on a diff.
- [Threads](../guide/threads.md): the sessions an agent can search with the thread tools.
- [Trust and safety](../concepts/trust-and-safety.md): the model behind approvals and the operation log.

---
title: MCP server
description: How AI agents use GitMaster through MCP, which tools it offers, and how it keeps you in charge of what the agent records.
status: partial
order: 10
---

> **Planned.** The tools that propose or apply a change to the repository, and the agent policy, are not available yet.

`gitmaster mcp` is an MCP server that your agent starts as a separate process and talks to over its standard input and output. It offers the tools that only GitMaster can offer: the review comments you wrote on a diff, the threads GitMaster captured from earlier agent sessions, the labels and links that connect those threads, and the reasons behind a range of lines, which GitMaster works out from those threads. It does not repeat what the agent already has. Reading files, searching text and running `git` stay with the agent's own tools.

## How the server runs

Your MCP client starts `gitmaster mcp` as a stdio server and stops it when the session ends. There is no socket, no port, no token and no resident process: the server lives exactly as long as the client that started it. The app does not need to be open.

Configure your MCP client to start this command:

```sh
gitmaster mcp
```

The Claude Code plugin does this for you with this entry in its `.mcp.json`:

```json
{
  "mcpServers": {
    "gitmaster": {
      "command": "gitmaster",
      "args": ["mcp"]
    }
  }
}
```

The server works on the repository that contains its working directory, which is the directory the client starts it in. If that directory is not in a repository, the server uses the directory in `CLAUDE_PROJECT_DIR`, which Claude Code sets. To name another repository, pass its path:

```sh
gitmaster mcp --repo ~/src/project
```

See [CLI](../reference/cli.md) for the command.

The server and the app share what they store in the repository's `.git/gitmaster/` directory. A comment the agent marks as handled appears in the app, and a comment you write in the app is there the next time the agent lists comments. Neither one waits for the other. The server runs no workflows: workflow triggers and phases run only in the app.

## Try the server by hand

You can send the server JSON-RPC messages on its standard input. Each line is one message, and the server answers each one on its standard output. This example starts it in a new repository and asks for the list of tools.

```sh
repo=$(mktemp -d) && git -C "$repo" init -q
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"try","version":"0"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | gitmaster mcp --repo "$repo"
```

The `tools/list` answer names each tool with its description, its `inputSchema` and its `annotations`. Each read tool is marked `readOnlyHint: true`, and each record tool `readOnlyHint: false`. The answer lists the tools in [MCP tools](../reference/mcp-tools.md).

To call a tool, send `tools/call` with the tool's `name` and its `arguments`. This asks for the open review comments of a repository:

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"review_comments_list","arguments":{"status":"open"}}}
```

A repository with no review comments answers with an empty list:

```json
{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"{\n  \"comments\": []\n}"}],"isError":false}}
```

The text inside `content` is the JSON result, as the agent reads it. Each comment in `comments` has the fields listed under [review_comments_list](../reference/mcp-tools.md#read).

## Several servers at once

Each agent session starts its own `gitmaster mcp`, so several can run on the same repository at the same time, in the same worktree or in different worktrees, and next to the app. They do not talk to each other and none of them blocks the others. Review comments and threads belong to the repository, so every worktree sees the same ones, and a comment handled by one server is handled for all. The comment's lines are found in the files of the worktree the server runs in.

When two servers write the same comment at the same time, the last summary stays. A comment you resolved stays resolved: no server can mark it handled again.

## What the server may do

The server offers two kinds of tool:

| Kind | What it does | Approval |
|---|---|---|
| Read | Reads review comments, threads, labels and the provenance of lines. Changes nothing. | Not required |
| Record | Writes to GitMaster's own store only: it marks a review comment as handled, with a summary, creates labels, attaches them and links threads. It never changes your files, your branches or your index. | Not required from GitMaster. Your agent may ask until you allow the tool, see [Claude Code permissions](#claude-code-permissions). |

A record tool takes effect on GitMaster's record, and a person still decides what it means. When an agent marks a comment as `handled`, the comment waits for you. Only you make it `resolved`. A label or a link an agent records carries the agent's name as its origin.

There is no tool that commits, pushes, restacks, edits a file or deletes anything. An agent that changes your code does it with its own tools, as it does without GitMaster, and the [thread](../guide/threads.md) records what it did. The record tools never write thread text, and they never delete or rename anything.

> **Planned.** Tools that propose a change, such as a restack plan, and that apply it. See [Approval](#approval).

## Approval

Your agent asks you before it uses a tool that changes the repository. GitMaster does not show its own approval prompt: the server runs without a window, so the approval is the one your agent already gives you.

> **Planned.** The tools that apply a change are not available yet. The rest of this section is how they will work.

- **Plan first.** A propose tool returns the whole plan and a `plan_id`. An apply tool accepts only that `plan_id`, never free arguments, so what you approve is the plan.
- **The agent asks every time.** Each apply tool tells the client that it needs a person's approval on every call. In Claude Code the prompt appears even when you allow edits automatically, in auto mode and in bypass mode. It offers no "don't ask again", and an allow rule does not skip it. In Claude Code's `dontAsk` mode the call is denied instead.
- **The server still checks.** Before it applies, it rebuilds the plan and refuses one that is stale, because the branches moved, or one that was already applied.
- **Only as strong as the client.** The approval is enforced by the agent, not by GitMaster. A client with no such control could let an apply call through, so the apply tools are offered only to clients known to enforce it.
- **Read and record tools do not ask for approval from GitMaster.** Your agent may still ask the first time you use them. See [Claude Code permissions](#claude-code-permissions).

## Output designed for agents

- **Small output by default.** Lists return a page, and the thread tools take a limit. An agent reads a thread's summary first, then asks for the entries it needs.
- **Tasks, not Git commands.** The server offers a handful of tools, each built for a task.

## Claude Code permissions

Claude Code asks before it uses an MCP tool you have not allowed. The read tools and `review_comment_resolve` are safe to allow, so you can add rules to your Claude Code settings to stop the questions. The rule name for a tool of the GitMaster plugin is `mcp__plugin_gitmaster_gitmaster__<tool>`:

```json
{
  "permissions": {
    "allow": [
      "mcp__plugin_gitmaster_gitmaster__review_comments_list",
      "mcp__plugin_gitmaster_gitmaster__review_comment_resolve",
      "mcp__plugin_gitmaster_gitmaster__threads_search",
      "mcp__plugin_gitmaster_gitmaster__thread_get",
      "mcp__plugin_gitmaster_gitmaster__thread_read",
      "mcp__plugin_gitmaster_gitmaster__provenance",
      "mcp__plugin_gitmaster_gitmaster__labels_list",
      "mcp__plugin_gitmaster_gitmaster__label_get"
    ]
  }
}
```

Do not allow the whole server with `mcp__plugin_gitmaster_gitmaster__*` once the apply tools exist. The apply tools ask anyway, but a narrow list keeps the intent clear. The plugin adds no allow rules of its own.

The three record tools that write labels and links, `label_create`, `label_attach` and `thread_link`, are not in that list, so Claude Code asks before each call until you allow them. To let the agent record labels and links without asking, add them to `allow`:

```json
{
  "permissions": {
    "allow": [
      "mcp__plugin_gitmaster_gitmaster__label_create",
      "mcp__plugin_gitmaster_gitmaster__label_attach",
      "mcp__plugin_gitmaster_gitmaster__thread_link"
    ]
  }
}
```

To forbid them, list the same names under `deny` instead.

## Untrusted content

Review comments, thread entries and file text come from people and from earlier sessions, so an agent must treat them as untrusted. The server returns them with `"untrusted": true`. Text that GitMaster redacted shows as `[REDACTED:name]`, and the original is not available to the agent.

Each command a record tool runs is written to the operation log with the actor `agent:<client>`, where `<client>` is the name the MCP client gave when it connected. Reads are not logged.

## Worked example: an agent works through your comments

You commented on two lines of `src/ledger.rs` in the app: "Return an error here instead of unwrapping". The agent handles it. The results below show the fields of each answer, with values shortened.

```text
review_comments_list(status="open")
    -> { comments: [{ id: 7, path: "src/ledger.rs", status: "open", outdated: false,
                      start_line: 41, end_line: 42, body: "Return an error ...",
                      anchor_text: "...", summary: null, untrusted: true }] }
       ... the agent edits src/ledger.rs with its own tools ...
review_comment_resolve(id=7, summary="parse_amount now returns Result; callers propagate with ?")
    -> { id: 7, status: "handled", summary: "parse_amount now returns Result; ...",
         change: { before: [...], after: [...], changed: true,
                   before_start_line: 41, after_start_line: 41 }, untrusted: true }
```

1. `review_comments_list` returns the open comments with the lines they are anchored to now. A comment whose text no longer appears exactly once is `outdated`, and its `start_line` and `end_line` are null.
2. The agent changes the code with its own tools.
3. `review_comment_resolve` marks the comment `handled`, stores the agent's summary and takes the "after" snapshot. The result carries the change GitMaster observed in the comment's region, so the agent can check that its edit landed where the comment is. The comment appears as handled in the app, and you decide whether to resolve it.

An agent that needs an earlier decision searches the threads instead. The answer is an envelope with the hits:

```text
threads_search(query="retry backoff")
    -> { hits: [{ thread: "01J9Z...", title: "...", seq: 12, turn: 4,
                  snippet: "...we chose jittered backoff because...", untrusted: true }],
         has_more: false, next_cursor: null, filters: {...}, diagnosis: null }
thread_read(id="01J9Z...", from=12, limit=10)
    -> { entries: [{ seq: 12, turn: 4, kind: ..., text: "...", untrusted: true }], has_more: true }
```

An agent that asks why a range of lines is the way it is gets the commits that wrote them and the threads behind each commit:

```text
provenance(path="src/ledger.rs", from_line=41, to_line=42)
    -> { complete: true, cuts: [], untrusted: true,
         commits: [{ id: "9f1c...", subject: "retry parse_amount", lines: [{ from: 41, to: 42 }],
                     threads: [{ thread: "01J9Z...", evidence: ["line_match"], ambiguous: false,
                                 edits: [{ id: 88, seq: 12, evidence: ["line_match", "hunk_overlap"],
                                           entries: [...] }] }] }] }
```

When a bound cuts the answer, `complete` is false and `cuts` names the bound, so the agent treats the answer as partial.

An agent that finds a thread worth a second look marks it for you, and you find it again by the label:

```text
label_create(name="needs-review", description="Retry logic changed without a test", color="#d97706")
    -> { label: { name: "needs-review", attachment_count: 0, origin: "agent:<client>", ... },
         created: true, origin: "agent:<client>", untrusted: true }
label_attach(label="needs-review", kind="thread", ref="01J9Z...")
    -> { label: {...}, attachment: { kind: "thread", ref: "01J9Z...", origin: "agent:<client>", at: ... },
         origin: "agent:<client>", untrusted: true }
labels_list(query="review")
    -> { labels: [{ name: "needs-review", attachment_count: 1, untrusted: true }], has_more: false }
```

## See also

- [MCP tools](../reference/mcp-tools.md): every tool, with its arguments and results.
- [Local review](local-review.md): how an agent works through the comments you write on a diff.
- [Threads](../guide/threads.md): the sessions an agent can search with the thread tools.
- [Trust and safety](../concepts/trust-and-safety.md): the model behind approvals and the operation log.

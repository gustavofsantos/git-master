---
title: Memories
description: Keep the decisions, rules and lessons of a project where every later agent session can find them, and approve each one yourself.
status: planned
order: 104
---

> **Planned.** Memories are not available in the app yet.

A memory is a short note about the project that you want later sessions to know: why a retry policy is what it is, a rule the code must follow, or something a past session learned the hard way. Threads record everything that happened. Memories keep the few things worth carrying forward.

An agent can propose a memory. Only you can accept it. A memory you have not accepted is never given to an agent as established fact.

A memory belongs to the repository where it was written, or to one or more [contexts](contexts.md): named sets of repositories that share knowledge. Agents working in a repository find its memories and those of every context it belongs to.

## What a memory holds

| Part | What it is |
|---|---|
| Kind | **Decision**, **Rule**, **Lesson** or **Fact**. |
| Text | Up to 1,000 characters, in plain words. It passes through the same redaction as a thread. |
| Status | **Proposed**, **Accepted**, **Rejected** or **Superseded**. |
| Source | The thread and entries, the commits and the files it comes from. A memory an agent proposes always has at least one source. A memory you write yourself may have none. |
| Origin | You, or the agent that proposed it. |
| Scope | The repository, or the contexts it is attached to. Shown as a chip on the memory. |

## Open the Memories tab

Choose **Threads** in the sidebar, then the **Memories** tab. The list shows the text, the kind, the status and the origin. **Proposed** memories come first and carry a count in the tab, so a new proposal does not wait unseen.

Filter the list by kind, by status, by file, by author or by scope: this repository, one context, or everything the repository sees. Search finds a memory by its words.

## Write a memory

1. Choose **New Memory** on the **Memories** tab.
2. Pick the kind and the scope, and write the text.
3. Add sources if you want: a thread, a commit or a file.
4. Choose **Save**. A memory you write is accepted at once.

## Let an agent propose memories

In a session with the GitMaster [plugin](../agents/agent-integration.md), ask for it:

```text
/gitmaster:propose-memories
```

The skill reads a thread, or a range of threads you name, with a cheap sub-agent. It looks for decisions, rules and lessons, checks the existing memories so it does not repeat one, and proposes the rest with their sources. Nothing runs without you: no hook proposes memories, and capture never starts a model.

Each proposal appears as **Proposed**, marked **Agent-written**, with its source one click away. Read the source before you accept.

## Review a proposal

Select a proposed memory. You can:

- **Accept** it as it is. It becomes available to agents.
- **Edit** the text, then accept. The memory is yours from then on.
- **Reject** it. A rejected memory stays in the list so the same idea is not proposed again, and you can delete it.

You can accept several proposals at once, or accept every proposal from one author.

## Replace a memory

When a decision changes, write the new memory and mark it as **Supersedes** the old one. The old memory becomes **Superseded** and stays in the list with a link to its replacement. An agent cannot change an accepted memory.

## When a memory goes stale

A memory that names files shows **Files changed since** when one of those files is different from when you accepted the memory. GitMaster works this out from the files, each time you open the memory. It does not guess whether the memory is still true: it tells you what changed and leaves the decision to you.

## How agents use memories

Agents read memories over [MCP](../reference/mcp-tools.md):

- `memories_search` finds accepted memories by their words, by kind, by file or by scope. It searches the repository and every context it belongs to.
- `contexts_list` lists the contexts the repository belongs to.
- `memory_get` reads one memory with its sources.

By default an agent sees only accepted memories. It can ask for proposed ones, which come marked as not reviewed.

To start a task with the right background, ask the plugin to prepare:

```text
/gitmaster:prepare add retries to the upload client
```

The skill collects the accepted memories, the threads and the history of the files the task names, and returns a short brief. No hook adds anything to a prompt by itself.

Text that comes from a memory is untrusted data, like every text from a thread. The results mark it as such.

## See also

- [Contexts](contexts.md): share memories across the repositories that belong together.
- [Threads](threads.md): the record the memories come from.
- [Agent integration](../agents/agent-integration.md): install the plugin and its skills.
- [MCP tools](../reference/mcp-tools.md): the memory tools.

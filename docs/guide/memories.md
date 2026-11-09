---
title: Memories
description: Keep the decisions, rules and lessons of a project where every later agent session can find them, and approve each one yourself.
status: partial
order: 104
---

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
| Basis | How it is known: **Observed** (seen in a thread or a file), **Tested** (checked by running something) or **Unconfirmed** (a belief). An unconfirmed memory is marked and is never given to an agent as established fact. |
| Origin | You, or the agent that proposed it. |
| Scope | The repository, or the contexts it is attached to. Shown as a chip on the memory. |

## Open the Memories tab

Open **Agents** in the sidebar, then choose the **Memories** tab. Or run **Show Memories** from the command palette. The tabs **Threads**, **Memories** and **Labels** sit above the page. The list shows the text, the kind, the status and the origin. A memory that is **Tested** or **Unconfirmed** also shows its basis as a chip; an **Observed** memory shows none. An **Unconfirmed** chip is flagged, as a changed file is. **Proposed** memories come first and carry a count in the tab, so a new proposal does not wait unseen.

Filter the list by kind, by status, by file, by author or by scope: this repository, one context, or everything the repository sees. Search finds a memory by its words.

## Write a memory

1. Choose **New Memory** on the **Memories** tab.
2. Pick the kind, the basis and the scope, and write the text. The basis starts at **Observed**; choose **Tested** if you checked it by running something, or **Unconfirmed** if it is a belief.
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
- **Edit** the text, then choose **Accept edited**. The memory is yours from then on.
- **Reject** it. A rejected memory stays in the list so the same idea is not proposed again, and you can delete it.

You can choose **Accept selected** for the proposals you choose, or **Accept all from** one author.

## Replace a memory

> **Planned.** A memory you write by hand cannot supersede another memory yet; only a proposal can.

When a decision changes, select the proposed memory that replaces the old one and choose **Supersedes…**. Pick the accepted memories it replaces, then accept it. The chosen memories become **Superseded** and stay in the list with a link to their replacement. An agent cannot change an accepted memory.

## Suggest an update to a memory

> **Planned.** An agent cannot suggest an update to an accepted memory yet.

A memory can go out of date, or miss a file it matters for. An agent that finds this can suggest an update instead of writing a second memory. The suggestion changes the text, the files the memory applies to, the links, or any of them, and it always gives a reason.

The suggestion appears on the memory as **Proposed update**, marked **Agent-written**, with the memory as it is and as it would become, side by side. You can:

- **Accept** it. The memory changes in place. Its marks, its links and the contexts it belongs to stay.
- **Accept edited**: change the new text first, then accept.
- **Deny** it. A denied suggestion stays in the list so the same idea is not proposed again.

If the memory changed after the agent wrote its suggestion, the suggestion says **Based on an older text** and you can accept it only after editing. The detail of a memory keeps its history: each earlier text, who suggested the change and when you accepted it. An agent can have at most 3 open suggestions on one memory, and at most 20 in a repository.

Use **Supersedes…** instead when a different memory replaces the old one.

## Where a memory applies

> **Planned.** Memories do not list the files they apply to yet.

A memory can list the files it matters for: a path such as `src/billing/retry.rs`, or a pattern such as `src/payments/**`. Use `*` for any name within a folder, `**` for any depth and `?` for one character. A pattern is relative to the root of the repository, and an absolute path or `..` is refused.

These are not sources. A source says where the memory comes from, and a **Files changed** tag compares it with the file as it was. The files a memory applies to say when it is worth reading. Add or edit them in the memory form and in its detail. An agent proposes them with a new memory or with a suggested update, and you accept them with it.

An agent asks which memories apply to a file with `memories_for_path`. The answer lists accepted memories only, each with a short snippet and the pattern that matched, and the agent reads the full memory with `memory_get`.

## Link memories

> **Planned.** Memories cannot be linked yet.

Choose **Link…** in the detail of an accepted memory and pick another accepted memory the repository can see. Three kinds of link are available:

| Kind | Meaning |
|---|---|
| **Relates to** | The two memories are about the same thing. The link works both ways. |
| **Depends on** | This memory holds because of the other one, such as a rule that follows from a decision. |
| **Contradicts** | The two cannot both be true. Both show **Conflicts with** until you supersede or reject one of them, or remove the link. |

A memory has at most 10 links. An agent proposes links inside a suggested update, and they take effect when you accept it. `memory_get` shows the linked memories one step away.

## Keep memories true

A memory is for what nobody can see again by looking: the reason for a choice, a rule, a lesson, a failed attempt. What the code, `git log` or a command shows does not belong in one, and the proposing skill leaves it out.

In a session with the GitMaster [plugin](../agents/agent-integration.md), ask:

```text
/gitmaster:consolidate
```

A cheap sub-agent checks accepted memories against their sources, the files they name, which changed since you accepted them, and the threads they cite. It starts with the memories whose files changed, then the ones marked unconfirmed, then the oldest accepted ones. It proposes a correction, a merge or a replacement. Each proposal is **Proposed**; one that replaces a memory begins by naming it, and when you accept it you choose **Supersedes…** to replace the old one. You accept it or not; nothing is replaced without you. The sub-agent reads the memory and its sources but not the current text of the files, so a memory that a file change does not touch can still be wrong.

GitMaster counts on your machine how often each memory is retrieved and when. An agent's search counts each memory it returns, and an agent's read counts the memory it reads. The counts are never shared, never sent to a model and never in an agent's results. The detail of a memory says **Retrieved 3 times, last 2 h ago**, or **Never retrieved**.

The line above the list gives the size of the scope you chose: how many accepted memories it holds and how many characters of text they hold, such as **12 accepted, 3.1k characters**. Search and the other filters do not change it. A store that grows without use is then visible.

## When a memory goes stale

A memory that names files shows a **Files changed** tag on its row when one of those files is different from when you accepted the memory, or is gone. Its detail lists them under **Changed since accepted** or **Removed since accepted**. GitMaster works this out from the files, each time you open the memory. It does not guess whether the memory is still true: it tells you what changed and leaves the decision to you.

## Mark a memory useful or wrong

A memory can be marked **Useful**, when it helped the work, or **Wrong**, when it misled someone or is no longer true. An agent marks a memory with the `memory_mark` tool of [MCP](../reference/mcp-tools.md), and can give a one-line reason. Only an accepted memory takes a mark.

Every mark is kept. The latest mark of each actor counts, and an earlier one stays in the history. If an agent called a memory useful and that memory then led to a wrong action, the agent marks it wrong; the earlier mark stays. Marking a memory the same way again changes nothing.

A memory whose latest mark from anyone is **Wrong** shows a note that asks to check it before relying on it. A mark is a signal and nothing more: it never deletes, supersedes or reorders a memory, and it never changes its text or status. Only you accept a change.

### Marks you make in the app

Select an accepted memory on the **Memories** tab. Its detail has two buttons, **Useful** and **Wrong**. The button of your current mark is highlighted. Choosing the other button changes your mark; choosing the one you already have does nothing. Before you choose, you can type an optional reason in the **Reason** box, in one line of at most 200 characters. The reason is kept with that mark, and the box empties once the mark is saved.

The detail lists the latest mark of each actor, you and each agent, with the time and the reason of each. It then lists every mark the memory has, oldest first. When your latest mark and an agent's latest mark differ, the detail shows **Disagreement**. A memory whose latest mark from anyone is **Wrong** shows a **Marked wrong** chip in the list and in the detail.

A proposed, rejected or superseded memory cannot be marked. Its two buttons are muted, and a click says why.

You can also choose **Mark Memory Useful** or **Mark Memory Wrong** in the [command palette](command-palette.md) for the selected memory. The palette marks it without a reason.

## How agents use memories

Agents read memories over [MCP](../reference/mcp-tools.md):

- `memories_search` finds accepted memories by their words, by kind, by file or by scope. It searches the repository and every context it belongs to.
- `contexts_list` lists the contexts the repository belongs to.
- `memory_get` reads one memory with its sources and its marks.
- `memories_for_path` lists the accepted memories that apply to a file.
- `memory_revise` suggests an update to an accepted memory. You accept or deny it.
- `memory_mark` records that a memory was useful or wrong, with an optional reason.

By default an agent sees only accepted memories. It can ask for proposed ones, which come marked as not reviewed.

Text that comes from a memory is untrusted data, like every text from a thread. The results mark it as such.

### Prepare a task

To start a task with the right background, ask the plugin to prepare:

```text
/gitmaster:prepare add retries to the upload client
```

The skill collects the accepted memories, the threads and the history of the files the task names, and returns a short brief. No hook adds anything to a prompt by itself.

## See also

- [Contexts](contexts.md): share memories across the repositories that belong together.
- [Threads](threads.md): the record the memories come from.
- [Agent integration](../agents/agent-integration.md): install the plugin and its skills.
- [MCP tools](../reference/mcp-tools.md): the memory tools.

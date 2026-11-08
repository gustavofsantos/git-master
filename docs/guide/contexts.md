---
title: Contexts
description: Group repositories that share one context, so a memory about work that touches several of them is found from each of them.
status: implemented
order: 106
---

Some work does not belong to one repository: a rule about how services call each other, a decision about a shared data format, a lesson from a change that went through four repositories. A context is a named set of repositories that share such knowledge. A [memory](memories.md) attached to a context is stored once and found from every repository that belongs to it.

A context is not a workspace. A [workspace](workspaces.md) puts branches of several repositories side by side so you can change them together. A context only says which repositories belong together. It needs no branches and no checked-out folders, and it lasts as long as you keep it.

## How contexts are organized

- A repository can belong to **many contexts**. A repository that two projects use sits in both.
- Contexts **nest**. A context can have sub-contexts, and a sub-context has one parent.
- From a repository you see its own memories, the memories of every context it belongs to, and the memories of every parent of those contexts.

For example:

```text
Platform                  7 repositories: api, web, billing, auth, jobs, infra, docs
  Billing                 billing, api, jobs
Partners                  5 repositories: portal, api, sync, auth, docs
```

A memory attached to **Platform** is found from all seven repositories, including `billing`, `api` and `jobs`, which reach it through **Billing**. A memory attached to **Billing** is found from `billing`, `api` and `jobs` only; to reach the other four, attach it to **Platform**. `api`, `auth` and `docs` belong to both **Platform** and **Partners**, so they see the memories of both.

## Manage contexts

Choose **Manage Contexts…** on the **Memories** tab of the **Agents** page, or from the command palette.

1. Choose **New Context** and give it a name. When a context is selected, the name box also asks whether the new one is a sub-context of it: choose **As a sub-context of** to switch between yes and no.
2. Choose **Add Repository** and pick from your recent repositories, or add the repository that is open.
3. Choose **⋯** on a context, then **Rename**, **Move…** or **Delete**.

When a repository first joins a context, GitMaster gives that clone an id, in its `.git` folder, so the membership still holds if you move the folder. Nothing is written into the working tree. If another clone already holds that id, the dialog says so and offers **Give this clone a new id**. After that the clone belongs to no context, so add it to the contexts again.

A context cannot be deleted while it has sub-contexts. Deleting one lists the memories that belong to no other context and asks before it removes them.

## Memories in a context

The **Memories** tab shows a chip on every memory with the scope it belongs to: the repository, or the context with its parents, such as `Platform / Billing`. Use the scope filter to show only this repository, one context, or everything you can see.

### Write a memory in a context

Choose **New Memory** and pick the scope. A memory you write is accepted at once.

### Move a memory into a context

A memory written in one repository can turn out to matter to others. Select it, choose **Move to Context…** and pick the context. The memory moves, with its sources, and leaves the repository's own list. To make a context memory also belong to another context, choose **Attach to Context…**. It stays one memory, not a copy.

### Detach a memory

To take a context memory out of a context, choose **Detach from Context…**. A memory attached to several contexts lists them, and you pick the one to leave; a memory always keeps at least one context.

### Let an agent propose a memory in a context

An agent working in a repository that belongs to a context can propose a memory in that context, and it names the context when it does. The proposal is **Proposed** like any other, and no other repository's agent sees it as established until you accept it. Accept it from any repository that belongs to the context.

A memory's sources can be threads, commits and files of different repositories. A source in another repository shows **In another repository** and cannot be opened from this one.

## What agents see

Over [MCP](../reference/mcp-tools.md), `memories_search` returns the memories of every scope the repository sees, each labeled with its scope. An agent can narrow the search to the repository or to one context, and `contexts_list` tells it which contexts its repository belongs to.

## What contexts do not do

- They do not organize memories into procedures, steps or any other structure. A memory is one short record, and a context is a place to find it.
- They do not search threads across repositories. A memory's sources open the thread in its own repository.

## See also

- [Memories](memories.md): write, propose, accept and use memories.
- [Workspaces](workspaces.md): change branches of several repositories together.
- [MCP tools](../reference/mcp-tools.md): the memory and context tools.
- [Configuration](../reference/configuration.md): where the context database is kept.

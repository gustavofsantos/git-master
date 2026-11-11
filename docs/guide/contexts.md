---
title: Contexts
description: Organize what you know into contexts such as payments, accounting or a recurring incident, nest them, and bind the repositories that work in them.
status: implemented
order: 106
---

A context is an area of knowledge: payments, sales, accounting, how services call each other, or one problem that comes back from time to time and has a runbook you follow by hand. Every [memory](memories.md) belongs to a context. Repositories are bound to the contexts they work in, so an agent working in a repository finds the knowledge of those contexts.

A context is not a repository and not a workspace. It exists on its own: you can create a context, write memories in it and bind no repository to it at all. A [workspace](workspaces.md) puts branches of several repositories side by side so you can change them together; a context says what those repositories have to know.

## How contexts are organized

- Contexts **nest**. A context can have sub-contexts, and a sub-context has one parent. A sub-context is a narrower part of its parent: **Refunds** under **Payments**, **Payments** under **Finance**.
- A sub-context **inherits** the memories of its parent and of every context above it. A memory in **Finance** holds in **Payments** and in **Refunds** too. The memories of a sub-context do not flow up: a memory in **Refunds** is not part of **Payments**.
- Naming a context to read it also covers its **sub-contexts**, at any depth. Reading **Payments** by name reads **Payments**, **Finance** (what it inherits) and **Refunds**. A repository bound to **Payments** still sees only its default reach; the sub-contexts come in when the context is named.
- A memory belongs to **one or more** contexts. It is stored once. When it holds in two contexts that are not parent and child, such as **Payments** and **Accounting**, it is attached to both.
- A repository is **bound to any number of contexts**, or to none. A repository that two areas use is bound to both.

For example:

```text
Finance
  Payments                bound: billing, api
    Refunds               bound: refunds-worker
  Accounting              bound: ledger
  Access                  bound: iam
Sales                     bound: crm, api
Incident: stuck exports   bound: no repository
```

An agent in `refunds-worker` sees the memories of **Refunds**, **Payments** and **Finance**. An agent in `api` sees those of **Payments**, **Finance** and **Sales**. An agent in `ledger` sees **Accounting** and **Finance**, but not **Payments**. **Incident: stuck exports** has no repository; you and any agent can still read it by name.

An agent in `refunds-worker` is changing a call to an endpoint that needs authorization. Its default reach does not include **Access**, which belongs to the area of the **iam** repository. The agent names the context to read it: `memories_search` with `scope: "Finance / Access"` reads **Access** and what it inherits, **Finance**. Naming **Finance** instead reads all of **Finance**, **Access** included, but not **Sales**.

## Manage contexts

Choose **Manage Contexts…** on the **Memories** tab of the **Agents** page, or from the command palette. The dialog shows every context you have as a tree, whether or not the open repository is bound to it. A context the open repository is bound to is marked **bound**.

1. Choose **New Context** and give it a name. When a context is selected, the name box also asks whether the new one is a sub-context of it: choose **As a sub-context of** to switch between yes and no.
2. To bind repositories, select a context and choose **Bind Repository**. Pick from your recent repositories, bind the repository that is open, or choose **Browse…** to pick any folder that holds a Git repository. A context can have as many repositories as you like, or none.
3. To unbind one, select it under the context and choose **Unbind**. Its memories stay in the context.
4. Choose **⋯** on a context, then **Rename**, **Move…** or **Delete**.

A context cannot be deleted while it has sub-contexts. Deleting one lists the memories that belong to no other context and asks before it removes them.

## Where contexts are kept

Contexts, the repositories bound to each and every memory are in one database of your own, in the GitMaster data folder. Nothing about a context is kept in any repository, so a context can reach across as many repositories as you want. When a repository is first bound to a context, GitMaster gives that clone an id, in its `.git` folder, so the binding still holds if you move the folder. That id is the only thing GitMaster writes in a repository for a context, and nothing is written into the working tree. If another clone already holds that id, the dialog says so and offers **Give this clone a new id**. After that the clone is bound to no context, so bind it again.

## The context of a repository

Knowledge that matters to one repository only still belongs to a context: one bound to that repository alone. GitMaster creates it for you, named after the repository's folder, the first time a memory is written or proposed in a repository that is bound to no context. It is an ordinary context: rename it, move it under another one, or bind more repositories to it.

## Memories in a context

The **Memories** tab shows a chip on every memory with the context it belongs to, with its parents, such as `Finance / Payments`. A memory attached to several contexts shows each of them.

The scope filter chooses what the list holds:

- **Bound to this repository**, the default: the memories of the contexts the repository is bound to and of every context above them.
- **One context**: pick any context in the tree. The list holds its memories, those it inherits and those of every sub-context below it. Choosing **Finance** lists the memories of **Finance**, **Payments**, **Refunds**, **Accounting** and **Access**, but not those of **Sales**.

### Write a memory in a context

Choose **New Memory** and pick the context. It starts at the first context the repository is bound to, by path, and you can pick any context in the tree. When the repository is bound to none, the form says **Goes to the context of this repository.** and the memory goes there. A memory you write is accepted at once.

### Attach a memory to another context

To make a memory also belong to another context, select it and choose **Attach to Context…**. It stays one memory, not a copy. Prefer the parent when the memory holds for every context under it: attaching it there once is enough.

### Move or detach a memory

Choose **Move to Context…** to take a memory out of its contexts and put it in another one. To take it out of one of several contexts, choose **Detach from Context…** and pick the one to leave. A memory always keeps at least one context.

### Let an agent propose a memory in a context

An agent proposes a memory in a context its repository is bound to, or in a context above one. When the repository is bound to one context, the agent may leave the context out. When it is bound to several, the agent names one; `contexts_list` tells it which. When it is bound to none, the proposal goes to the context of the repository, created as described above.

The proposal is **Proposed** like any other, and no agent sees it as established until you accept it. Accept it on the **Memories** tab of any repository: pick its context in the scope filter to find it.

A memory's sources can be threads, commits and files of different repositories. Each source names its repository. A source in a repository that is not on this computer shows **In another repository** and cannot be opened.

## What agents see

Over [MCP](../reference/mcp-tools.md), `memories_search` returns, by default, the memories of the contexts the repository is bound to and of every context above them, each labeled with its context. An agent can name any context, bound or not, to read that context, what it inherits and every sub-context below it: a runbook context bound to no repository is read this way. An agent working near another area names it to read it: an agent in `refunds-worker` that touches authorization names **Access** with `scope: "Finance / Access"`, and names **Finance** to see everything under it. The default `bound` scope does not change, so the other areas stay out until they are named. `contexts_list` lists the contexts the repository is bound to; with `all` it lists every context, and its `accepted_memories` counts the memories that naming the context reads.

## Contexts in a workspace

A [workspace](workspaces.md) is bound to the contexts of its members, and to its own contexts if you bind it. The memories of all of them are found from the workspace. See [Memories and contexts in a workspace](workspaces.md#memories-and-contexts-in-a-workspace).

## What contexts do not do

- They do not organize memories into procedures, steps or any other structure. A memory is one short record, and a context is where it holds.
- They do not search threads across repositories. A memory's sources open the thread in its own repository.

## See also

- [Memories](memories.md): write, propose, accept and use memories.
- [Workspaces](workspaces.md): change branches of several repositories together.
- [MCP tools](../reference/mcp-tools.md): the memory and context tools.
- [Configuration](../reference/configuration.md): where the contexts database is kept.

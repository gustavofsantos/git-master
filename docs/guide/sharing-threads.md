---
title: Sharing threads
description: Publish your threads to a Git remote you choose, and read the threads your teammates published, without changing how Git or GitMaster behave when sharing is off.
status: planned
order: 102
---

> **Planned.** Sharing threads is not available in the app yet.

Threads stay on your machine. Sharing is a separate, optional step: you publish chosen threads to a Git remote, and your teammates fetch them and read them next to their own. With sharing off, which is the default, GitMaster adds no ref, no hook, no file and no network call, and `git push` does exactly what it did before.

Sharing needs [threads](threads.md). It reads only what GitMaster already stored, and it never stores anything unredacted.

## Turn sharing on

Sharing is a setting of each repository.

1. Open **Threads Settings…** and find **Sharing**.
2. Turn on **Share Threads**.
3. Choose the **Remote** the threads go to.

There is no default remote. You can choose the repository's own remote or a separate repository that only holds threads. A separate repository is the better choice when the code repository is public, because everyone who can read the remote can read every thread published to it.

These choices are `gitmaster.sharing.*` keys in Git's configuration, like the other GitMaster preferences: see [Configuration](../reference/configuration.md#sharing). GitMaster never writes a file into the working tree for sharing.

## Publish threads

Publishing is always an action you take. GitMaster never publishes on its own, and it installs no Git hook for it.

1. Select one or more threads in the list, or select none to publish every thread that is not published yet.
2. Choose **Publish Threads…** from the **⋯** menu or the command palette. From a terminal, run `gitmaster thread publish`.
3. Read the confirmation. It shows the remote, the number of threads, how many secrets redaction removed from them, and a reminder of who can read the remote.
4. Choose **Publish**.

GitMaster pushes only the refs it created for the threads. It never pushes a branch.

### What is published

| Published | Not published |
|---|---|
| The prompts, replies, tool calls and results, as stored and already redacted | Your notes on a thread |
| The file edits, as patches | Stars |
| The agent, the model, the branch, the start and end commits and the times | Labels |
| The commit links you or GitMaster made | The local capture queue |
| Your name and email, taken from your Git configuration | |

A thread is published as one object. Publishing it again replaces the earlier version; the remote does not keep a history of versions. If your Git signing is configured, the object is signed.

### Checks before a thread leaves your machine

Redaction happens when a thread is captured, and publishing does not change the stored text. Before it pushes, GitMaster checks every thread against your current redaction patterns and sensitive paths. If a thread contains text that one of them matches, for example because you added the pattern after the thread was captured, the publish stops for that thread and names the thread and the pattern. Run **Scrub Threads** to apply the patterns to stored threads, then publish again.

> **Warning.** Redaction is a safety net, not a guarantee. It finds secrets by pattern. For customer data or regulated material, publish to a private repository, or do not publish.

## Fetch shared threads

1. Choose **Fetch Shared Threads** from the **⋯** menu or the command palette. From a terminal, run `gitmaster thread fetch`.
2. GitMaster reads the thread refs from the remote and adds each new thread to your list.

A thread you already have is skipped. A fetched thread shows its author, and the list can be filtered by author.

### Imported threads

An imported thread is read-only. You can read it, search it, star it, label it, add a note and delete it, and all of that stays on your machine. You cannot change its entries.

- Its text goes through your redaction patterns when it is imported, so your patterns apply to what your teammates sent.
- GitMaster checks the thread against the hash its author published. If they differ, the thread is marked **Modified**.
- An agent that reads an imported thread over MCP receives the text marked as untrusted, like every text that comes from a thread. See [Trust and safety](../concepts/trust-and-safety.md).

## Unpublish a thread

Choose **Unpublish Thread** on a published thread. GitMaster deletes the ref from the remote and from your repository.

> **Note.** Unpublishing is not erasure. A teammate who already fetched the thread keeps it, and the Git host can keep the objects until it collects them. Treat anything you publish as shared.

## Share memories too

If you use [memories](memories.md), turn on **Share Accepted Memories** next to **Share Threads**. Accepted memories are published under the same rules as threads. Memories you fetch arrive as proposals that you accept yourself.

## See also

- [Threads](threads.md): what a thread holds and how it is captured.
- [Memories](memories.md): decisions and rules that outlive a session.
- [Configuration](../reference/configuration.md): the sharing settings.
- [Trust and safety](../concepts/trust-and-safety.md): what GitMaster records and where.

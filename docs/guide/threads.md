---
title: Threads
description: Read, search and keep the record of your agent sessions: what you asked, what the agent answered, the tools it ran and the files it edited.
status: partial
order: 100
---

A thread is the record of one agent session in one repository. GitMaster captures the conversation, every tool call the agent made and every file edit, stores it in a database inside the repository's `.git` directory, and shows it on the **Threads** page. You can search old threads, open an agent's edit as a diff, link a thread to a commit, and delete what you do not want to keep.

Threads are captured by the agent integration. Install it first: see [Agent integration](../agents/agent-integration.md).

## What a thread holds

| Part | What is recorded |
|---|---|
| Prompts | What you typed to the agent, in order. |
| Replies | What the agent wrote back. |
| Tool calls | Each tool the agent used, with its arguments and result. A long result is cut at 64 KiB, and the entry says it was cut and how large it was. |
| Edits | Each change an agent tool made to a file, as a patch, with the number of added and removed lines. |
| Context | The agent, the model, the branch and the commit at the start, and the start and end time. |

A thread does not hold the model's reasoning, images or other attachments. An image shows as a placeholder with its type and size.

A thread belongs to the repository where the agent ran. A session started in a linked worktree is stored with the repository, and every worktree sees it. A session that ran outside a Git repository is not captured.

## Open the Threads page

Choose **Threads** in the sidebar, or run **Show Threads** from the command palette. The list is on the left and the selected thread is on the right.

Each row of the list shows the title, the agent, the branch, how long ago the last event was, and how many edits the thread made. A dot marks a session that is still open. A **redacted** mark shows when GitMaster removed a secret from the thread.

The title comes from your first prompt, as plain text: the Markdown markers, emphasis and links are removed, and a long title is cut at 120 characters. You can rename it.

### Read a thread

The header of the thread takes two lines. The first has the title, then the star (★ or ☆), **Copy as Markdown** and **⋯**. The **⋯** button opens the less used actions: Rename, Add Note, Tags, Link to Commit, Export, Expand all tool calls, Collapse all tool calls and Delete. The second line has the agent, the branch, when the thread started and whether it is still open, then the chips: **N commits**, **N files**, **N edits** and, when the thread has one, **Note**.

A commit chip, a file chip or the **Note** chip opens a details panel under the header. Only one panel is open at a time, and a click on its chip closes it. The commits panel lists each commit with whether it was created during this thread or linked by you; a link you made has **Unlink**. The files panel lists each edited file once, with the lines its edits added and removed. A path inside the worktree is shown relative to it, a path inside your home folder starts with `~`, and a long path is cut at its start so the file name stays visible. The **Note** panel shows the note in full. A panel is at most six rows high and scrolls past that.

The thread is grouped by turn. A turn is one prompt of yours and everything the agent did until it stopped. Each turn starts with a thin divider that says `Turn n`; the first turn has no divider.

- Your prompt is marked `You` in the left column and has a thin accent bar. The agent's text shows in full, as Markdown: headings, bold, italic and strikethrough, lists and task lists, quotes, tables, code blocks and links, and it is marked `Agent` in the left column. A run of replies with no prompt between them names the agent once, on the first. A code block in a language GitMaster knows is coloured, and a link shows its text without being followed. Tool arguments and results stay plain text. A prompt appears when the agent's next event arrives, such as its first tool call or the end of its turn, because GitMaster reads prompts from the agent's session file.
- Each tool call is one line with the tool, a short summary of its arguments, whether it worked and how long it took. Select the line to expand the arguments and the result. A **Delete Entry** control appears at the line's right end while the pointer is over it.
- Each edit is one line with the path, `+n −m`, its state, **Open Diff** and **View File**. **Open Diff** expands the patch under the line, and **View File** opens the file in the [Code page](code-navigation.md).

The thread loads as you scroll. The first entries appear at once, and the next page is read when you come near the end of what is shown. A search result opens at its entry, and the entries before it are read when you scroll to the top. A line at the end says **Loading…** while a page is read and **End of thread** when there is nothing more.

Tool calls that follow each other in a turn form a group. Its header says how many calls there are and which tools ran, for example `▸ 7 tool calls: Read ×3, Edit ×2, Bash ×2`. A group shows its edits and any failed call even while it is closed. Select the header to open the group, and select a call inside it to see its arguments and result. **Expand all** and **Collapse all** above the thread open or close every group at once. A group is closed when you open a thread, except the one that holds a search result. The **⋯** menu's **Expand all tool calls** and **Collapse all tool calls** open or close every group of the thread at once.

An edit has one of these states, worked out each time you open the thread from the files themselves:

| State | Meaning |
|---|---|
| In working tree | The file on disk has exactly the content the agent wrote. |
| In HEAD | The file at `HEAD` has exactly the content the agent wrote. |
| Changed since | The file differs from what the agent wrote. |

An edit that GitMaster could not measure from the file, for example because the file was too large, is marked **From tool input** and shows what the agent asked for, not what the file contained.

### Changes that are not edits

Only the agent's file-editing tools produce edits. A change made by a shell command, a formatter or a code generator appears as the command and its output, not as an edit.

> **Planned.** A list of the changes that happened during a turn, including those made by shell commands, is not available in the app yet.

## Search threads

Type in the search box above the list. There is no search syntax.

- Words are matched as the start of a word, and all of them must appear. `retry back` finds `retry backoff`.
- Put words in double quotes to match them as a phrase.
- Identifiers match whole, so `file_index` finds `file_index` and not every `file`.

Results show a snippet around the match. Select one to open the thread at that entry. Narrow the list with the **Starred** and **Has edits** filters, and with a file: choose **Show Threads for File** on a file to list the threads that edited it. A removable chip shows the file.

> **Planned.** Filters for the agent, the branch and the dates are not available in the app yet.

Search runs on your machine over what is stored in the repository. Nothing is sent anywhere.

## Link threads to your work

### Threads for a file or a commit

Choose **Show Threads for File** in the menu of a file in the Working Copy to list the threads that edited that file.

Select a commit in [History](history.md) and its panel lists the threads linked to it, under **Threads**. Each row shows the title, the agent and how long ago the thread was active. Select a row to open that thread on this page, even when a filter would leave it out. The section lists the newest ten threads.

> **Planned.** The section does not yet mark the threads that were created during the thread, and it does not say how many threads follow the first ten.

### How a commit gets linked

GitMaster links a thread to a commit in two ways only:

- **Created during this thread.** The commit appeared on the thread's branch between two events of the thread. This says when the commit was made, not who made it: a commit you made yourself in the same period also counts.
- **Linked by you.** Choose **Link to Commit** in the **⋯** menu of the thread's header and pick the commit. The thread's **Commits** panel shows the link as linked by you, with **Unlink** beside it. Unlinking removes the link only; the commit is untouched.

The **Commits** chip of the header counts both kinds. The **Files** panel lists the files the thread edited, and a file you linked by hand, with its own **Unlink**.

GitMaster never guesses that a thread produced a commit because the contents look alike.

## Keep and clean up

| Action | What it does |
|---|---|
| **Rename** | Changes the title. |
| **Add Note** | Attaches a note of your own to the thread. A note is searchable. |
| **Star** | Marks the thread so you can filter for it. |
| **Tags** | Adds your own labels. |
| **Export** | Writes the thread as a Markdown file at a path you choose. Redaction is applied again on the way out. |
| **Copy as Markdown** | Puts the thread on the clipboard as the Markdown that Export writes, with the same redaction. |
| **Delete** | Removes the thread from the database. The confirmation says what is removed. |
| **Delete Entry** | Removes one message or tool call from a thread. |

A delete is final. GitMaster overwrites the deleted text in the database file, and it does not keep a copy. It does not touch the agent's own session files; see [What is not removed](#what-is-not-removed).

Rename, Add Note, Tags, Link to Commit, Export and Delete are in the **⋯** menu of the header; Star and Copy as Markdown are buttons beside the title. The thread's right-click menu in the list has the same actions.

Only you can change or delete a thread. An agent connected through MCP can read threads and cannot write to them.

## Secrets

Agents read files and run commands that can print passwords and keys, and the session carries them. GitMaster removes secrets **before** anything is saved: the database, the search index and the temporary files never hold the original text.

A matched secret is replaced with `[REDACTED:name]`, where `name` is the pattern that matched, for example `[REDACTED:github-token]`. The thread keeps a count of redactions per pattern, never the values.

### What is redacted

GitMaster ships patterns for private key blocks, cloud and API keys and tokens from common services, JSON web tokens, `Authorization` headers, passwords in URLs, and values assigned to names such as `password`, `secret`, `token` and `api_key`. It also replaces the content of any file whose path matches a sensitive path, such as `.env`, `*.pem` and `id_rsa*`, while keeping the path.

### Add your own patterns

Add patterns for secrets that are specific to you, such as an internal token format or a private host name, in **Threads Settings…** from the command palette or the Threads page, or in `settings.json`. See [Configuration](../reference/configuration.md) for the keys.

```json
{
  "threads": {
    "redact": {
      "patterns": [
        { "name": "acme-token", "regex": "acme_[A-Za-z0-9]{32}" }
      ],
      "paths": [".env", ".env.*", "*.pem", "secrets/**"]
    }
  }
}
```

A pattern is a regular expression. Use the test field in **Threads Settings…**, or `gitmaster thread redact-test`, to try a pattern on sample text before you rely on it.

A pattern that is not a valid regular expression **pauses capture** in every repository until you fix or remove it. Events that arrive while it is paused are not stored. The Threads page says how many were skipped and which pattern is at fault. GitMaster stops capturing rather than risk storing a secret.

### Apply a new pattern to old threads

A pattern you add applies to what is captured from then on. To apply it to threads you already have, run **Scrub Threads** from the command palette. GitMaster runs the current patterns over every stored thread and tells you how many entries changed. Scrub can only remove more text, never restore any.

### What is not removed

- Redaction cannot reach the agent's own session files. The agent keeps its original text there, and GitMaster only reads them.
- A pattern catches what it describes. A secret written as ordinary words is not caught. Add a pattern, or delete the thread.
- Redaction cannot be undone. GitMaster does not keep the original.

## Pause or stop capturing

- Turn off **Capture Threads** in **Threads Settings…** to stop capturing everywhere. Threads already stored stay.

> **Planned.** **Pause Capture in This Repository**, which stops capturing in one repository only, is not available in the app yet.
- Remove the integration to stop the agent from reporting at all. See [Agent integration](../agents/agent-integration.md#remove-the-integration).

## Where threads are stored

Threads are stored in `.git/gitmaster/threads.db`, in the Git common directory of the repository, readable only by you. Nothing is written to the working tree, so no `.gitignore` entry is needed. Deleting the repository deletes its threads. See [Configuration](../reference/configuration.md).

## Search from an agent

> **Planned.** The MCP tools that let an agent search and read threads are not available in the app yet.

Once the MCP server is on, an agent can search earlier threads, for example to find why a past change was made. The text of a thread is returned marked as untrusted, because it came from earlier prompts and tool output. See [MCP tools](../reference/mcp-tools.md).

## See also

- [Agent integration](../agents/agent-integration.md): install the capture hooks.
- [Trust and safety](../concepts/trust-and-safety.md): what GitMaster records and where.
- [Configuration](../reference/configuration.md): the `threads` settings and the database file.

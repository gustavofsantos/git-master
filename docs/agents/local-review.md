---
title: Local review
description: How to comment on a diff, have an agent work through the comments over MCP, and see for each comment what the agent changed.
status: partial
order: 20
---

> **Planned.** The agent's summary in the diff, and the changes that overlap no comment. Today a handled comment shows its status and its Before/After, and the summary is returned to the agent only.

Local review is a loop between you and an agent on your machine. You comment on lines of a diff. The agent reads the comments through MCP, works on the code and resolves each comment with a summary. GitMaster then shows what changed for each comment, from snapshots of the working tree, not from the agent's own account.

## Comment on a diff

1. Open a diff and select the lines the agent should look at.
2. Write a comment on the selection. The comment appears inline in the diff.

## The loop

1. You comment on the lines.
2. The agent reads the open comments with `review_comments_list`.
3. The agent changes the code, then calls `review_comment_resolve` on each comment it handled, with a summary of what it did.
4. The comment becomes `handled`, with the summary beside it. You read the change and resolve the comment, or reopen it.

## Comment states

| State | Meaning |
|---|---|
| `open` | Waiting for the agent. |
| `handled` | The agent has worked on it and left a summary. The agent sets this state. |
| `resolved` | You confirmed it. The agent only proposes this state; you make it final. |
| `outdated` | The text the comment is anchored to no longer appears exactly once, so GitMaster does not place it. |

## Anchors

Each comment is anchored to its file, the text you selected and a few lines of context around that text.

- If the selected text still appears exactly once in the file, the comment stays on it.
- If the selected text appears more than once, or not at all, the comment becomes `outdated`.

GitMaster never moves a comment to a position it estimated. A comment is either on its exact text or `outdated`.

## Before and after

GitMaster does not rely on the agent's report of what it changed. It takes two snapshots of the working tree:

- one when you write the comment;
- one when the agent marks it handled.

The snapshots are stored as private refs inside `.git`, so the working tree and your branches are not touched.

What the agent did for a comment is the diff between the two snapshots, limited to the region the comment is anchored to. GitMaster finds the anchored lines in the "before" snapshot and follows them through the changes to the file: lines the agent added, removed or rewrote inside the region, or right against its edges, belong to the comment. The agent's summary is the narrative; the diff is the record.

When the agent marks the same comment handled again, the new "after" snapshot replaces the old one. When you reopen a comment, its "after" snapshot is discarded and the agent's summary with it. If a snapshot cannot be taken, the comment is still handled, without an "after".

> **Planned.** Changes that overlap no comment appear as "other changes".

## Read the comments in a diff

Each comment appears inline in the diff. Use the **Before** / **After** selector to see the anchored text as it was when you wrote the comment, or as it was when the agent marked it handled. Until an agent handles the comment, **After** shows the text as it is in the file now.

## Where comments are stored

Comments and snapshots live outside the working tree, in `.git/gitmaster/` of the repository. Nothing is written into your files.

The **Review Storage** setting, in the **Local Review** section of Settings, is where comments and snapshots are kept.

Open the **Local review** entry in the sidebar to see the comments of the repository.

## See also

- [MCP server](mcp-server.md): how the agent connects and how approvals work.
- [MCP tools](../reference/mcp-tools.md): the review tools, with their arguments and approval.
- [Diff and staging](../guide/diff-and-staging.md): the diff where you write comments.

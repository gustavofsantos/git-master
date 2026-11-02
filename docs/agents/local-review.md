---
title: Local review
description: How to comment on a diff, have an agent work through the comments over MCP, and see for each comment what the agent changed.
status: partial
order: 20
---

Local review is a loop between you and an agent on your machine. You comment on lines of a diff. The agent reads the comments through MCP, works on the code and marks each comment handled with a summary. GitMaster then shows what changed for each comment, from snapshots of the working tree, not from the agent's own account. You decide when a comment is resolved.

> **Planned.** The agent's summary of a handled comment is not shown in the app yet. The agent receives it, and the handled comment shows its status and its Before and After text.

## Comment on code

You can comment on any lines of any code in the repository: lines of a diff, changed or not, and lines of a file in the **Code** viewer.

1. Press on a line number and drag to the last line, or click a line number and Shift-click another. The range is highlighted. This selection is separate from the checkboxes that stage lines.
2. Press `Ctrl+Shift+M` (`⌘⇧M` on macOS), or choose **Comment** in the small bar that appears over the selection.
3. Write the comment. It appears inline, and the comments of the repository are listed on the **Local review** page.

Where you can select:

- the **Working Copy** diff, on both sides, in changed lines and in the lines around them;
- a file in the **Code** viewer, changed or not;
- a **commit** or **stash** diff.

A comment is always about text in the working tree. In the Working Copy diff (unstaged side) and in the Code viewer the lines are the file's own, so the comment is anchored at once. In a staged, commit or stash diff GitMaster looks for the selected text in the file as it is now: if it appears exactly once, the comment is anchored there; if it is gone or appears more than once, GitMaster says so and writes nothing. Lines a diff shows as removed are not in the working tree, so a selection of only removed lines cannot be commented.

> **Planned.** Comments already written are drawn inline in the Code viewer and in commit and stash diffs; today they show in the Working Copy diff and on the Local review page.

## Example: ask an agent to fix a line

1. Open the **Working Copy** diff of `src/ledger.rs`, or open the file in the **Code** viewer.
2. Click the line number of `.unwrap()` and Shift-click the line number of the last line of the same expression.
3. Press `Ctrl+Shift+M` and write: `Return an error here instead of unwrapping.`
4. Ask the agent to work through the open review comments. It calls `review_comments_list`, changes the code, and calls `review_comment_resolve` with a summary.
5. Open **Local review** in the sidebar. The comment shows **Handled**. Read the change, then choose **Resolve Comment**, or **Reopen Comment** to send it back.

## The loop

1. You comment on the code.
2. The agent reads the open comments with `review_comments_list`.
3. The agent changes the code, then calls `review_comment_resolve` on each comment it handled, with a summary of what it did.
4. The comment becomes **Handled**, with the summary sent to the agent. You read the change and choose **Resolve Comment**, or **Reopen Comment**.

## Comment states

The app shows each comment with one of these badges:

| Badge | Meaning |
|---|---|
| **Open** | Waiting for the agent. |
| **Handled** | The agent has worked on it and left a summary. The agent sets this state. |
| **Resolved** | You confirmed it. The agent only proposes this state; you make it final. An agent cannot mark a resolved comment handled again. |
| **Outdated** | The text the comment is anchored to no longer appears exactly once, so GitMaster does not place it. |

**Outdated** is a flag on a comment rather than a state of its own: an open, handled or resolved comment becomes outdated when its text is no longer found.

## Anchors

Each comment is anchored to its file, the text you selected and a few lines of context around that text.

- If the selected text still appears exactly once in the file, the comment stays on it.
- If the selected text appears more than once, or not at all, the comment becomes **Outdated**.

GitMaster never moves a comment to a position it estimated. A comment is either on its exact text or **Outdated**.

## Before and after

GitMaster does not rely on the agent's report of what it changed. It takes two snapshots of the working tree:

- one when you write the comment;
- one when the agent marks it handled.

The snapshots are stored as private refs inside `.git`, so the working tree and your branches are not touched.

What the agent did for a comment is the diff between the two snapshots, limited to the region the comment is anchored to. GitMaster finds the anchored lines in the "before" snapshot and follows them through the changes to the file: lines the agent added, removed or rewrote inside the region, or right against its edges, belong to the comment. The agent's summary is the narrative; the diff is the record.

When the agent marks the same comment handled again, the new "after" snapshot replaces the old one. When you reopen a comment, its "after" snapshot and the agent's summary are discarded. If a snapshot cannot be taken, the comment is still handled, without an "after".

> **Planned.** Changes that overlap no comment appear as "other changes".

## Read the comments in a diff

Each comment appears inline in the diff. A comment opens on **Before** when it has a snapshot, which shows the anchored text as it was when you wrote the comment. Switch to **After** to see the text as it was when the agent marked it handled. Until an agent handles the comment, **After** shows the text as it is in the file now, captioned **Now**.

## Where comments are stored

Comments and snapshots live outside the working tree, in the repository's `.git/gitmaster/` directory, in `review.db`. Nothing is written into your files.

Open the **Local review** entry in the sidebar to see the comments of the repository.

## See also

- [MCP server](mcp-server.md): how the agent connects and how approvals work.
- [MCP tools](../reference/mcp-tools.md): the review tools, with their arguments and approval.
- [Diff and staging](../guide/diff-and-staging.md): the diff where you write comments.

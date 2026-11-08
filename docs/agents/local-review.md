---
title: Local review
description: How to comment on a diff, have an agent work through the comments over MCP, and see for each comment what the agent changed.
status: partial
order: 20
---

Local review is a loop between you and an agent on your machine. You comment on lines of a diff. The agent reads the comments through MCP, works on the code and marks each comment handled with a summary. GitMaster then shows what changed for each comment, from snapshots of the working tree, not from the agent's own account. You decide when a comment is resolved.

## The Review page

Open **Review** in the sidebar. The page has three parts:

- On the left, three cards and the conversations. **Changes** shows the branch and its upstream (`feature → origin/feature`), the commits ahead and behind, the lines added and removed in the working tree, and how many files you marked **Viewed**. **Gates** lists the `before.<operation>` workflows that are switched on, with **needs review** on a gate whose version you have not reviewed, and says how many gates run before **Push with lease**. **Reviewers** has no content: reviewers are not available yet. Below the cards, **CONVERSATIONS** lists the review comments, grouped by file, with the filter chips in the page header.
- In the middle, one continuous diff: every changed file, one after another, with its path, `+a −r` and a **Viewed** box. A viewed file folds to its header. The mark clears by itself when the file's change changes. A file shows at most 400 lines; open it on **Changes** to read the rest. At most 200 files are read.
- On the right, the selected conversation: the comment, its replies, the reply box, its actions and the **Before** and **After** of the change.

> **Planned.** Bypassing a gate with a written reason, and reviewers and their verdicts, are not available yet. See [Reviewers and gates](reviewers.md).

## Comment on code

You can comment on any lines of any code in the repository: lines of a diff, changed or not, and lines of a file in the **Code** viewer.

1. Press on a line number and drag to the last line, or click a line number and Shift-click another. The range is highlighted. This selection is separate from the checkboxes that stage lines.
2. Press `Ctrl+Shift+M` (`⌘⇧M` on macOS), or choose **Comment** in the small bar that appears over the selection.
3. Write the comment. It appears inline, and the comments of the repository are listed on the **Review** page.

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
5. Open **Review** in the sidebar. The comment shows **Handled**. Read the change, then choose **Resolve Comment**, or **Reopen Comment** to send it back.

## The loop

1. You comment on the code.
2. The agent reads the open comments with `review_comments_list`.
3. The agent changes the code, then calls `review_comment_resolve` on each comment it handled, with a summary of what it did.
4. The comment becomes **Handled**, and the agent's summary is kept as a reply in its thread (see [Replies](#replies)). You read the change and choose **Resolve Comment**, or **Reopen Comment**.

## Comment states

The app shows each comment with one of these badges:

| Badge | Meaning |
|---|---|
| **Open** | Waiting for the agent. |
| **Handled** | The agent has worked on it and left a summary. The agent sets this state. |
| **Resolved** | You confirmed it. The agent only proposes this state; you make it final. An agent cannot mark a resolved comment handled again. |
| **Outdated** | The text the comment is anchored to no longer appears exactly once, so GitMaster does not place it. |

**Outdated** is a flag on a comment rather than a state of its own: an open, handled or resolved comment becomes outdated when its text is no longer found.

## Replies

A thread is a comment and the replies to it. The comment is the **root** of the thread. Its replies follow it in order, oldest first, directly below it in the diff and on the **Review** page. Each reply has an author, a body and a time. The author is **You** or the agent that wrote the reply, which is shown as **Agent-written**. A reply is plain text: GitMaster shows it as written, does not render markup in it and runs nothing in it.

- A thread is one level deep. A reply to a reply belongs to the thread of the root, so a thread never nests.
- The status, the anchor, the **Outdated** flag and the Before and After snapshots belong to the root. A reply has none of them.
- The agent's summary is a reply. When the agent marks a comment handled, its summary becomes a reply by the agent on the thread. Marking it handled again adds another reply, so the thread keeps every answer, and the "after" snapshot is replaced as before.
- The agent can ask you a question without changing the status. It answers with a reply, which leaves the comment as it is.
- A thread of more than two replies shows its latest reply and a toggle, **N earlier replies**, that shows the others; **Hide earlier replies** folds them again.
- You reply in the **Reply** box under the thread on the **Review** page, or with **Reply** on the card in the diff, which opens the comment box on the thread. Your reply on a handled comment reopens it: it becomes **Open** again, its "after" snapshot is discarded, and the agent sees it the next time it lists the open comments. Your reply on an open comment leaves its status as it is.
- Nobody replies to a resolved comment. The agent is refused, and the **Reply** box is not offered until you choose **Reopen Comment**.
- A reply cannot be edited, resolved, handled or deleted on its own. Deleting a comment deletes its replies too.

### Example: a question and an answer

1. You comment on `parse_amount` in `src/ledger.rs`: "Return an error here instead of unwrapping." The comment is 7.
2. The agent reads it with `review_comments_list`, then asks before it changes anything: `review_comment_reply(id=7, body="Should the error be ParseError or the crate's Error type?")`. The comment stays **Open**.
3. You reply in the **Reply** box: "Use ParseError." The comment stays **Open**.
4. The agent changes the code and calls `review_comment_resolve(id=7, summary="parse_amount returns Result<_, ParseError>; callers propagate with ?")`. The summary becomes its reply, and the comment is **Handled**.
5. You read the change and reply once more: "Cover the empty string too." Your reply reopens the comment: it is **Open** again, and its "after" snapshot is discarded.
6. The agent calls `review_comments_list(status="open")` and gets the comment with its replies. It fixes the empty string and resolves the comment again. A second summary is added as a reply, and the "after" snapshot is replaced.
7. You choose **Resolve Comment**. The thread is resolved, and nobody replies to it until you choose **Reopen Comment**.

The agent sees the thread like this:

```text
review_comments_list(status="open")
    -> { comments: [{ id: 7, path: "src/ledger.rs", status: "open", outdated: false,
                      start_line: 41, end_line: 42, body: "Return an error ...",
                      anchor_text: "...", untrusted: true,
                      replies: [
                        { id: 8, author: "agent:claude-code", body: "Should the error be ...?", created_at: "...", untrusted: true },
                        { id: 9, author: "human", body: "Use ParseError.", created_at: "...", untrusted: true },
                        { id: 10, author: "agent:claude-code", body: "parse_amount returns ...", created_at: "...", untrusted: true },
                        { id: 11, author: "human", body: "Cover the empty string too.", created_at: "...", untrusted: true }] }] }
```

## Notes, questions and flags from the agent

The agent can start a thread of its own on any text in the working tree, with `review_comment_create`. Each one has a kind:

| Kind | Use |
|---|---|
| **Note** | Something worth knowing about this code. It asks nothing of you. |
| **Question** | The agent needs your answer. It shows **Needs answer** until you reply. |
| **Flag** | A risk or a guess you should look at, with a severity of low, medium or high. |

An agent thread is anchored, snapshotted, shown inline and marked **Outdated** like yours, and its root is labelled **Agent-written**. Its root also shows its kind as a chip: **Note**, **Question** or **Flag**. A flag shows its severity after the kind, as **low**, **medium** or **high**. A question that no one has answered shows the **Needs answer** badge. You answer in the same **Reply** box. The agent does not receive your answer by itself: it reads it the next time it lists its threads, for example when you run `/gitmaster:inbox`. Only you close a thread. On a note the button reads **Dismiss** instead of **Resolve**. An agent has at most 20 open threads in a repository and 5 on one file, and a further one is refused.

The **Local review** page has two filters for these threads: **Needs answer**, the questions waiting for your answer, and **Agent**, every thread an agent started, whatever its status. The count on the sidebar's **Local review** row counts only the open threads you started. An agent's note or flag is not work waiting for the agent, so it is not counted there.

`review_comments_list` returns the threads you started by default, so the agent's own notes are not handed back to it as work. A question you have answered is returned, so the agent sees the answer.

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

When the agent marks the same comment handled again, the new "after" snapshot replaces the old one. When you reopen a comment, its "after" snapshot is discarded, and its replies stay. If a snapshot cannot be taken, the comment is still handled, without an "after".

> **Planned.** Changes that overlap no comment appear as "other changes".

## Read the comments in a diff

Each comment appears inline in the diff. A comment opens on **Before** when it has a snapshot, which shows the anchored text as it was when you wrote the comment. Switch to **After** to see the text as it was when the agent marked it handled. Until an agent handles the comment, **After** shows the text as it is in the file now, captioned **Now**.

## Where comments are stored

Comments and snapshots live outside the working tree, in the repository's `.git/gitmaster/` directory, in `review.db`. Nothing is written into your files.

Open the **Review** entry in the sidebar to see the comments of the repository.

## See also

- [MCP server](mcp-server.md): how the agent connects and how approvals work.
- [MCP tools](../reference/mcp-tools.md): the review tools, with their arguments and approval.
- [Diff and staging](../guide/diff-and-staging.md): the diff where you write comments.

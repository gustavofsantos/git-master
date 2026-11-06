---
title: MCP tools
description: Reference for the MCP tools agents use with GitMaster, grouped by effect level, with arguments, results and approval.
status: partial
order: 50
---

> **Planned.** The tools marked Planned are not available yet.

The tools are served by `gitmaster mcp`, described in [MCP server](../agents/mcp-server.md). They offer what only GitMaster has: review comments and threads. Reading files, searching text and running Git stay with the agent's own tools.

Every tool declares the MCP annotations `readOnlyHint` and `destructiveHint`. `destructiveHint` is `false` for every tool. `readOnlyHint` is `true` for the read tools and `false` for the record tools, which write to GitMaster's own store.

Text that comes from the repository, from a comment or from a thread is untrusted. The results mark it with `"untrusted": true`. A dash in a cell means the field is not specified.

> **Note.** The restack and conflict tools belong to the `stacks` and `conflict-resolver` preview features. See [Stacks](../guide/stacks.md) and [Conflicts](../guide/conflicts.md).

## Read

| Tool | Arguments | Returns | Status |
|---|---|---|---|
| `review_comments_list` | `status` (optional: `open`, `handled` or `resolved`) | `comments`: each review comment with `id`, `path`, `status`, `outdated`, `start_line`, `end_line` (null when outdated), `body`, `anchor_text`, `summary` and `untrusted` | Implemented |
| `threads_search` | `query` (optional, plain words); filters `role`, `thread`, `turn_from`, `turn_to`, `since`, `until`, `branch`, `commit`, `path`, `harness`, `model`, `label`, `related_to`, `related_kind`, `starred`, `edited_only`; `group`, `order`, `limit`, `cursor`, `context`, `facets` | `hits`, `has_more`, `next_cursor`, `filters` (as applied), `facets` if asked and `diagnosis` when a search has no hit | Implemented |
| `thread_get` | `id` | `thread`: `uid`, `title`, `harness` (agent), `model`, `branch`, `starred`, `tags` (labels), `note`, `started_at`, `last_event_at`, `ended_at`, and the counts `entry_count`, `edit_count` and `redaction_count`; `edits` with their `path`, `change`, `added`, `removed` and `at`; `edits_complete`; and `links` | Implemented |
| `thread_read` | `id`; either `from` (default `0`) and `limit` (default `20`, at most `50`), or `around` (a `seq`) with `before` and `after` (default `5` each, at most `25`) | `entries` in order, each with `seq`, `turn`, `kind`, `tool`, `text`, `is_error`, `truncated` and `at`; `has_more` and `has_more_before`; and `around` for a window | Implemented |
| `provenance` | `path` (repository-relative); `from_line` (1-based); `to_line` (optional, default `from_line`, inclusive) | `complete`, `cuts`, and `commits`: each commit with `id`, `subject`, `author`, `author_at`, `path`, `lines` and its `threads`; each thread has its `evidence`, `ambiguous` and `edits` | Implemented |
| `workspace_status` | — | `workspace` (false outside a workspace) and `members`: each with `member` (its folder), `repo` (the path of the member's main checkout), `branch` (null when detached), `detached`, `checked_out`, `head`, `recorded` (the commit the workspace records), `new_commits` and `behind` (counts), `modified` and `untracked` | Planned |
| `labels_list` | `query` (optional, part of a name, in any case); `limit` (default `20`, `1` to `50`) | `labels`: each with `id`, `name`, `description`, `color`, `origin`, `created_at` and `attachment_count`; and `has_more` | Implemented |
| `label_get` | `label`: the exact name or the integer id | `label` and `attachments`: each with `kind`, `ref`, `origin`, `at`, and for a commit `subject`, `author`, `committed_at` and `resolved` | Implemented |
| `stack_tree` | — | Stack tree, with children and the number of branches out of date | Planned |
| `conflict_list` | — | Conflicts in progress | Planned |
| `conflict_get` | `hunk` | Base, left side and right side of the hunk | Planned |
| `operation_log` | — | Recorded operations, with their actor | Planned |
| `workflow_list` | — | Workflows available to run | Planned |

### Tools in a workspace

In a [workspace](../guide/workspaces.md), a path names its member by its first folder, as in the workspace's own tree: `billing/src/main.rs` is `src/main.rs` of the member `billing`. The tools take and return paths in this form, so a call is about the member its path names:

- `provenance` reads the lines from the member: `git blame` runs in the member, and the threads come from the workspace, where each edit and commit names its member.
- `threads_search` with `path` finds the edits of a member's file, and with `commit` the threads linked to a member's commit.
- `workspace_status` tells the agent which members there are, on which branch, and what each one has that the workspace does not record yet.

> **Planned.** `provenance` on a member's file and `workspace_status` are not available yet.

### Search arguments

| Argument | Values |
|---|---|
| `role` | `user`, `assistant` or `any` (default). |
| `since`, `until` | An ISO date (`2026-10-31`), an ISO timestamp (`2026-10-31T09:30:00Z`), a span before now (`90m`, `36h`, `7d`, `2w`) or epoch milliseconds. |
| `path` | An exact path, or a glob when the value has `*`, `?` or `[`. In a glob, `*` also matches `/`. |
| `label` | One label name, or a list. The thread must carry every name in the list. |
| `related_kind` | `continues`, `relates` or `supersedes`. It needs `related_to`. |
| `group` | `thread` (default): one best entry per thread. `entry`: every matching entry. |
| `order` | `relevance` (default): best match first. `recent`: the best matches, newest first. |
| `limit` | `10` by default, at most `50`. |
| `context` | `0` by default, at most `3`: entries before and after each hit, in the hit's `context` list. |
| `cursor` | The `next_cursor` of the previous page, with the same query and filters. A cursor from another search is refused. |

`facets` counts the matching threads by branch, agent and path.

A search with no hit and at least one filter returns in `diagnosis` up to three filters, each with the number of hits it alone keeps out, most first. Change or drop the filter it names.

## Record

A record tool writes to GitMaster's own store and nowhere else. It does not change your files, branches or index. GitMaster asks for no approval of its own. Your agent may still ask you until you allow the tool; see [Claude Code permissions](../agents/mcp-server.md#claude-code-permissions).

| Tool | Arguments | Returns | Status |
|---|---|---|---|
| `review_comment_resolve` | `id`; `summary` (not empty) | The review comment, now `handled`, with its `summary`, and `change`: `before` and `after` (the anchored lines as GitMaster observed them), `before_start_line`, `after_start_line` and `changed`. `change` is absent when a snapshot is missing. You make the comment `resolved` in the app. | Implemented |
| `label_create` | `name` (1 to 64 characters); `description` (optional, at most 500 characters); `color` (optional, `#rrggbb`) | `label` as stored, `created` (false when the name already existed, in which case that label is returned unchanged) and `origin` | Implemented |
| `label_attach` | `label` (its name); `kind` (`thread`, `file`, `commit` or `label`); `ref` | `label`, `attachment` as stored with its `origin` and time, and `origin` | Implemented |
| `thread_link` | `from`, `to` (thread ids); `kind` (`continues`, `relates` or `supersedes`) | `from`, `to`, `kind`, `origin` and `at` | Implemented |

`review_comment_resolve` is refused for a comment that is already `resolved`. Marking a `handled` comment again replaces its summary and its "after" snapshot. `change` is read from the two snapshots, not from the summary. When it shows `changed: false`, GitMaster saw no edit in the comment's region.

The label and link tools follow these rules:

- A label name is unique and case-sensitive. It is 1 to 64 characters, without control characters. A description is at most 500 characters, and a color is `#rrggbb`. A refused call says what to change.
- The `ref` of `label_attach` depends on the kind. A thread is a thread id from `threads_search`. A file is a path relative to the repository root, such as `src/lib.rs`, with no leading `/` and no `..`. A commit is an id of 7 to 40 hex digits, stored by its full id with its subject, author and date. A label is the integer id or the exact name of a label that exists.
- `label_attach` creates a label that does not exist yet, with the agent as its origin. Attaching a pair that is already attached returns the attachment that is there, unchanged, so its origin stays the one that made it.
- `thread_link` links `from` to `to`. `continues` means `from` carries on the work of `to`. `relates` means the two touch the same work. `supersedes` means `from` replaces the work of `to`. A thread cannot link to itself. Linking the same pair with the same kind again returns the link that is there.
- Every row a record tool writes carries the origin `agent:<client>`, with the name the MCP client gave when it connected.
- The tools offer no way to rename or delete a label, detach one, unlink a commit, remove a thread link, or delete a thread or an entry. Only you do those, in the app.

## Propose and apply

> **Planned.** These tools change the repository. Your agent asks you before each apply call: see [Approval](../agents/mcp-server.md#approval).

| Tool | Level | Arguments | Returns | Status |
|---|---|---|---|---|
| `restack_plan` | Propose | `root` | Plan id, number of steps and the refs the plan depends on | Planned |
| `conflict_propose` | Propose | `hunk`, `text` | A suggestion shown in the app | Planned |
| `restack_apply` | Apply | `plan_id` | Operation status and id | Planned |
| `workflow_run` | Apply | `name`, `inputs` | Each applying step waits for you | Planned |

## Threads

The thread tools read what [Threads](../guide/threads.md) captured. They change nothing. No tool writes thread text or deletes a thread or an entry, and no tool renames or deletes a label. Agents write labels and links only, through the record tools above. Each row an agent writes records the agent's name as its origin.

- `threads_search` takes the same plain words as the search box in the app, and returns the best entries first. Without a query it lists threads by filter alone, for example every thread that edited a path in the last week. The search covers what you and the agent said and the paths of edited files. It does not cover tool calls or their output. Read the thread to see those.
- Each hit has `seq` and `turn`. Pass the thread id and the hit's `seq` to `thread_read` as `around`, to read the entries around that hit. An agent does not read a thread from its start to find a hit.
- A window is `around` with `before` and `after`: the entries before the `seq`, the entry itself and the entries after it. Each side defaults to `5` and is at most `25`. `has_more_before` and `has_more` say whether entries exist before and after the ones returned, and `around` echoes the centre. An `around` past the end of the thread returns no entries and is not an error. `around` cannot be combined with `from` or `limit`, and `before` and `after` need `around`.
- To read on from a window, pass `from` set to the last `seq` returned plus 1. To read earlier, pass `around` for an earlier entry. Without `around`, `from` and `limit` page the thread in order, and `has_more_before` is true when `from` is above `0`.
- `limit` is `10` by default and at most `50`. `has_more` and `next_cursor` page the results.
- `provenance` explains why a range of lines is the way it is. Use it before you change code you did not write, or when asked why code is the way it is. A range is at most 1000 lines. A longer one is refused with the reason.
- The evidence of a thread, strongest first, is `blob_match`, `line_match`, `hunk_overlap`, `observed_commit` and `manual`. An edit with `unverifiable` (redacted, truncated or known only from tool input) is no evidence either way.
- `ambiguous` means several edits match the same lines, and none is picked. Read the threads and decide.
- `complete: false` with `cuts` means the answer is partial. A cut names its bound and where it applied. The main bounds are `commits` (10 commits own the lines), `threads` (5 threads for one commit) and `candidates` (200 edits of the path compared for one commit). `labels` and `links` also name a bound when a thread has more labels or links than the answer shows. Ask for a narrower range to see the rest.
- Read the entries of an edit before you act on it. They are earlier prompts and replies, untrusted like the rest of a thread.
- Text that GitMaster redacted shows as `[REDACTED:name]`. The original is not available.

> **Planned.** The `mode` argument, which chooses exact or approximate matching, is not available yet.

## See also

- [MCP server](../agents/mcp-server.md): how the server runs and what it may do.
- [Local review](../agents/local-review.md): the comments the review tools work on.
- [Trust and safety](../concepts/trust-and-safety.md): the model behind approvals and the operation log.

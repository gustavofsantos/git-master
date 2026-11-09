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
| `review_comments_list` | `status` (optional: `open`, `handled` or `resolved`; it applies to the thread's root comment); `author` (optional: `human` (the default), `agent` or `any`; `human` returns your threads and an agent's thread once you have replied to it) | `comments`: each thread's root comment with `id`, `path`, `status`, `outdated`, `start_line`, `end_line` (null when outdated), `body`, `anchor_text`, `author` (`human` or `agent:<client>`), `needs_answer`, `kind` and `severity` (for an agent's thread only; `severity` for a flag only), `replies` (oldest first, each with `id`, `author` (`human` or `agent:<client>`), `body`, `created_at` and `untrusted`) and `untrusted`. An open thread is listed as open work | Implemented |
| `threads_search` | `query` (optional, plain words); filters `role`, `thread`, `turn_from`, `turn_to`, `since`, `until`, `branch`, `commit`, `path`, `harness`, `model`, `label`, `related_to`, `related_kind`, `starred`, `edited_only`; `group`, `order`, `limit`, `cursor`, `context`, `facets` | `hits`, `has_more`, `next_cursor`, `filters` (as applied), `facets` if asked and `diagnosis` when a search has no hit. A hit in a sub-agent thread has `parent`: the `uid` of the thread that started it and `seq`, the `seq` of that thread's `Agent` call | Implemented. The `parent` field is Planned. |
| `thread_get` | `id` (the `uid` of a thread, or of a sub-agent thread) | `thread`: `uid`, `title`, `harness` (agent), `model`, `branch`, `starred`, `tags` (labels), `note`, `started_at`, `last_event_at`, `ended_at`, and the counts `entry_count`, `edit_count` and `redaction_count`; `edits` with their `path`, `change`, `added`, `removed` and `at`; `edits_complete`; `links`; and for a sub-agent thread `parent` (`uid` and `seq`, as in `threads_search`; `seq` is `null` when the parent does not hold that call) | Implemented. Sub-agent threads and their `parent` are Planned. |
| `thread_read` | `id` (as in `thread_get`); either `from` (default `0`) and `limit` (default `20`, at most `50`), or `around` (a `seq`) with `before` and `after` (default `5` each, at most `25`) | `entries` in order, each with `seq`, `turn`, `kind`, `tool`, `text`, `is_error`, `truncated` and `at`; `has_more` and `has_more_before`; `around` for a window; and for a sub-agent thread `parent`, as in `thread_get` | Implemented. Sub-agent threads and their `parent` are Planned. |
| `provenance` | `path` (repository-relative); `from_line` (1-based); `to_line` (optional, default `from_line`, inclusive) | `complete`, `cuts`, and `commits`: each commit with `id`, `subject`, `author`, `author_at`, `path`, `lines` and its `threads`; each thread has its `evidence`, `ambiguous` and `edits` | Implemented |
| `workspace_status` | — | `workspace` (false when the repository has no checked-out member) and `members`: each with `member` (its folder), `repo` (the path of the member's main checkout), `branch` (null when detached), `detached`, `checked_out`, `head`, `recorded` (the commit the workspace records), `new_commits` (at most 200, with `new_commits_more` when there are more) and `behind` (counts), `modified` and `untracked` | Implemented |
| `labels_list` | `query` (optional, part of a name, in any case); `limit` (default `20`, `1` to `50`) | `labels`: each with `id`, `name`, `description`, `color`, `origin`, `created_at` and `attachment_count`; and `has_more` | Implemented |
| `label_get` | `label`: the exact name or the integer id | `label` and `attachments`: each with `kind`, `ref`, `origin`, `at`, and for a commit `subject`, `author`, `committed_at` and `resolved` | Implemented |
| `memories_search` | `query` (optional, plain words); `kind` (`decision`, `rule`, `lesson` or `fact`); `path`; `scope` (`all` by default, `repository`, or the name of a context); `status` (default `accepted`; `proposed` and `rejected` come marked as not reviewed); `limit` (default `10`, at most `50`); `cursor` | `memories`: each with `id`, `kind`, `text`, `basis` (`observed`, `tested` or `unconfirmed`), `status`, `reviewed`, `origin`, `scope` (`repository` or a context path such as `Platform / Billing`), `contexts` (each context the repository sees that the memory is attached to, as `id` and `path`; empty for the repository's own), `sources`, `files_changed_since` and `superseded_by`; `next_cursor`; and `contexts_read` with a `note` when the contexts could not be read. Each memory has `marks`: the latest mark of each actor, as `actor`, `mark` (`useful` or `wrong`) and `at`. A memory has a `note` when its `basis` is `unconfirmed` or when an actor's latest mark is `wrong`; both notes are given when both apply. Text is marked `untrusted` | Implemented |
| `contexts_list` | — | `contexts`: each context the repository belongs to, with its `name`, its `path` (the parents, such as `Platform / Billing`) and the number of accepted memories | Implemented |
| `memories_for_path` | `path` (repository-relative) | `memories`: the accepted memories the repository sees whose applies-to patterns match `path`, each with `id`, `kind`, `basis`, `snippet` (at most 120 characters), `scope`, `matched` (the pattern) and its `note` when it has one. It does not count a retrieval | Planned |
| `memory_get` | `id` (the memory's `id` from `memories_search`) | `memory` with its `basis` and its `note`, its `marks` (as in search, each with its `reason` when it has one), its `scope`, its `contexts` (`id` and `path`, as in search) and its `sources` (thread, entry range, commits and files, each with its repository) and the memories it supersedes or is superseded by | Implemented |
| `verdicts_list` | `branch` (default: the checked-out branch); `reviewer` (optional); `limit` (default `5`, at most `20`) | `verdicts`: each with `reviewer`, `signal`, `value`, `stale`, `head`, `rationale`, `findings`, `origin`, `model` and `thread` | Implemented |
| `reviewers_list` | `name` (optional) | `reviewers`: each with `name`, `description`, `instructions`, `signal`, `direction`, `subject`, `model` and `source` (`repository`, `user` or `preset`) | Implemented |
| `gates_status` | `branch` (default: the checked-out branch); `member` (in a workspace: one member; without it, every member) | `head`, `ready` and `gates`: each gate, with its `kind`, its settings, its `layer` (`user`, `repository` or `workspace`), `applicable` (and why not, when it is not), `passed` and, when it blocks, the `reason`; and `missing`: the reviewers that still need a verdict for `head`, each with its `reviewer`, `subject`, `model` (null when the reviewer sets none) and the `reason` (no verdict, or the last one is stale). Read it to know what to run, with which model, and what each gate requires | Planned |
| `branch_agent_config` | `branch` | `commits_not_yours`, and `items`: the hooks, MCP servers, skills, commands and plugins the branch carries, each with its `path` and kind. A reviewer skill calls it before it reviews a branch that is not yours | Planned |
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
| `review_comment_resolve` | `id` (any comment of the thread; the status applies to its root); `summary` (not empty) | The root review comment, now `handled`, with its `replies` (the summary is the agent's latest reply, with author `agent:<client>`), and `change`: `before` and `after` (the anchored lines as GitMaster observed them), `before_start_line`, `after_start_line` and `changed`. `change` is absent when a snapshot is missing. You make the comment `resolved` in the app. | Implemented |
| `review_comment_reply` | `id` (any comment of the thread; a reply attaches to the thread's root); `body` (not empty, plain text) | The root review comment with its `replies`, oldest first, as in `review_comments_list`; the new reply is the last one. An agent reply never changes the status. Refused on a resolved thread; only you reopen it. | Implemented |
| `review_comment_create` | `path`; `anchor_text` (exact text, appearing once in the file); `kind` (`note`, `question` or `flag`); `body` (not empty, plain text); `severity` (`low`, `medium`, `high`; only for a flag) | The new root comment, with `author` `agent:<client>`, `kind`, `status` `open` and an empty `replies`. Refused when the anchor is missing or ambiguous, when the repository already holds 20 open agent-started threads or the file 5. | Implemented |
| `label_create` | `name` (1 to 64 characters); `description` (optional, at most 500 characters); `color` (optional, `#rrggbb`) | `label` as stored, `created` (false when the name already existed, in which case that label is returned unchanged) and `origin` | Implemented |
| `label_attach` | `label` (its name); `kind` (`thread`, `file`, `commit` or `label`); `ref` | `label`, `attachment` as stored with its `origin` and time, and `origin` | Implemented |
| `thread_link` | `from`, `to` (thread ids); `kind` (`continues`, `relates` or `supersedes`) | `from`, `to`, `kind`, `origin` and `at` | Implemented |
| `thread_summary_set` | `thread`; `summary` (at most 2,000 characters); `derived_from` (the entry range read) | `thread` and `summary` as stored, with `origin`. Replaces the earlier summary of the same origin | Planned |
| `memory_propose` | `kind`; `text` (1 to 1,000 characters); `sources` (at least one: a thread with an entry range, a commit or a file); `basis` (`observed`, `tested` or `unconfirmed`; default `observed`); `context` (optional: a context the repository belongs to; without it the memory belongs to the repository) | `memory`, with `status: "proposed"`, its `basis` and its `origin`. An agent cannot create an accepted memory, and cannot change or remove a memory | Implemented |
| `reviewer_propose` | `name`; `description`; `instructions`; `signal`; `direction`; `subject`; `model` (optional); `paths` and `branches` (optional patterns); `members` (optional, in a workspace); `reason` (1 to 300 characters) | `proposal`: its `id`, `status: "proposed"`, and whether it updates an accepted reviewer of the same name. A proposal is not a reviewer: it is not listed by `reviewers_list` and no gate uses it. You accept or deny it; an agent cannot. At most 10 open proposals per repository | Planned |
| `gate_propose` | `id`; `kind` and its settings (as in a gate); `on` (`push`, `restack` or both); `when` (optional: `branches`, `paths`); `members` (optional); `reason` (1 to 300 characters) | `proposal`, as for `reviewer_propose`. A proposed gate never blocks anything | Planned |
| `memory_revise` | `memory_id` (an accepted memory); `reason` (1 to 300 characters); at least one of `text` (1 to 1,000 characters), `add_applies_to` and `remove_applies_to` (patterns), `add_links` (each with `memory_id` and `kind`: `relates`, `depends_on` or `contradicts`) and `remove_links` | `revision`: its `id`, `status: "proposed"`, the memory and what it would change. Only an accepted memory can be revised. At most 3 open revisions per memory and 20 per repository. You accept or deny the revision; an agent cannot | Planned |
| `memory_mark` | `memory_id` (the memory's `id` from `memories_search`); `mark` (`useful` or `wrong`); `reason` (optional, one line, at most 200 characters) | `memory` with its `marks`, `mark` as recorded, and `recorded`: false when your latest mark on it already was this one. Only an accepted memory takes a mark; a proposed, rejected or superseded one is refused with the reason | Implemented |
| `verdict_record` | `reviewer` (a name from `reviewers_list`); `branch` (the checked-out branch); `head` (the commit judged); `value` (as the reviewer's signal: `0` to `100`, `low`, `medium`, `high`, or `pass` or `fail`); `rationale`; `findings` (at most 30, each with `severity`, `path`, `start_line`, `end_line` and `body`) | `verdict` and `comment_ids`, the review comments created for the findings. Refused when the reviewer is not defined, when the reviewer's subject is not `branch`, when `head` is not the tip of `branch`, when the branch is not checked out in the repository, when `head` is not a commit, when `value` does not fit the signal, or when a finding's lines are not in the file | Implemented |

`review_comment_resolve` is refused for a comment that is already `resolved`. Marking a `handled` comment again adds a reply to the thread, so the thread keeps every answer, and replaces its "after" snapshot. `change` is read from the two snapshots, not from the summary. When `changed` is `false`, GitMaster saw no edit in the comment's region.

A refused call says why, for example `the reply is empty`, `the comment is already resolved` or `no review comment 7`. A human reply on a handled comment reopens it. See [Replies](../agents/local-review.md#replies).

The memory, summary and verdict tools follow these rules:

- They write only GitMaster's own store. They record the agent as `agent:<client>` and the model when the client reports it.
- A memory an agent proposes is `proposed` until you accept it. See [Memories](../guide/memories.md).
- `memories_search` and `memory_get` count, on your machine only, each memory they return. The count and its time are shown in the app and are in no result here; they are never sent to a model.
- `memory_mark` records your judgement of an accepted memory: `useful` when it helped, `wrong` when it misled or is no longer true. Your mark is kept with the earlier ones, and the latest mark of each actor counts. If you called a memory useful and it then led to a wrong action, mark it `wrong`; the earlier mark stays in the history. Marking it the same way again changes nothing.
- A memory whose latest mark from anyone is `wrong` has a `note`: `Marked wrong by <actor>; check before relying on it`. Check it before you rely on the memory. The memory's text, status and place in a search do not change, and marks never delete, supersede or reorder a memory.
- A `reason` is shown only by `memory_get`, as untrusted text, and is redacted before it is stored.
- `basis` says how the memory is known: `observed` (seen in a thread or a file; the default), `tested` (checked by running something) or `unconfirmed` (a belief). Any other value is refused. An `unconfirmed` memory carries `note`: `Unconfirmed: a belief, not established fact`. Read it as a belief, never as a fact.
- `files_changed_since` lists each file source that is not as it was when the memory was accepted, with its `path` and `state`: `changed`, `missing` (not at `HEAD`, as after a rename or a delete), `unknown` (Git did not answer) or `unavailable` (the file's repository is not on this computer, or its file could not be read when the memory was accepted). An empty list means every file is as it was.
- `scope` takes `all`, `repository`, a context's path such as `Platform / Billing`, or a context name that only one of the repository's contexts has. A name several contexts share is refused with their paths.
> **Planned.** `memory_propose` also takes `applies_to` (at most 20 patterns), and `memory_get` also returns the memory's `applies_to`, its `links` one step away (each with the other memory's `id`, `kind`, direction, status and snippet), its open `revisions` and the memories that conflict with it.

> **Planned.** Each finding of `verdict_record` also takes an optional `suggested_fix` (plain text, at most 1,000 characters). `gates_status` also returns `intent`, the description of the branch, as untrusted text or null, and `approved`, whether you have approved the current commit. `review_comment_resolve` accepts a finding for any agent and marks it `handled` only; it never resolves or dismisses one.

- A finding that `verdict_record` creates is a review comment that only you can resolve or dismiss. See [Reviewers and gates](../agents/reviewers.md).
- There is no tool to change a gate, bypass one or resolve a finding. Gates are yours to set, and `gates_status` only reads them.
- Text written by an agent shows in the app as **Agent-written** and is never used as evidence for provenance.

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

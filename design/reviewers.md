# Review workflows and scorecards: engineering note

The user-facing behavior is in `docs/agents/reviewers.md`, `docs/agents/local-review.md` and `docs/reference/mcp-tools.md`, marked `Planned` where not built. The decision is D219. This note records what to build. It replaces the reviewers, verdicts and gates design of D189, D212, D214 and D215, which slices S2 and S3 remove. The skills that summarize, propose memories, prepare and dispatch are in `design/agent-skills.md`.

Last updated: 2026-11-10. Status: decided (D219). Open: Q42 (the Cursor CLI harness), Q43 (the transport; its reviewer part is moot).

## The shape

A reviewer is a manual workflow (D102) with `review = true` in `M.meta`. Its Lua runs `ctx.exec` on the user's command-line agent (D74, D188 (3)). The agent publishes through MCP record tools. The engine stores a scorecard and its comments, derives staleness on read, and blocks a push only when a `before.push` workflow reads `ctx.scorecards` and calls `ctx.block`. No model is called and no gate is native.

## Store (schema v11)

```sql
CREATE TABLE scorecards (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,                     -- a-z0-9-, 1 to 40 characters
  branch TEXT NOT NULL,
  head TEXT NOT NULL,                     -- the commit judged
  status TEXT NOT NULL CHECK (status IN ('pass', 'needs_work', 'fail')),
  label TEXT NOT NULL,
  decorations TEXT,                       -- JSON array
  subject TEXT NOT NULL,                  -- at most 120 characters
  body TEXT NOT NULL,                     -- at most 4,000 characters, redacted (D121)
  origin TEXT NOT NULL,                   -- 'agent:<client>', from the channel (D218)
  model TEXT, run_thread TEXT, workflow_run TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX scorecards_by_name_branch ON scorecards(name, branch, id DESC);
ALTER TABLE comments ADD COLUMN scorecard_id INTEGER REFERENCES scorecards(id) ON DELETE SET NULL;
ALTER TABLE comments ADD COLUMN label TEXT;
ALTER TABLE comments ADD COLUMN decorations TEXT;
DROP TABLE verdicts; DROP TABLE approvals; DROP TABLE bypasses; DROP TABLE proposals;
```

- Migration v11 from v10 runs once, and `SCHEMA_VERSION` goes from 10 to 11. The rows of the four dropped tables are not migrated. Findings already stored stay as review comments. Their `verdict_id`, `finding_branch` and `finding_head` columns stay in place, unused.
- Publish inserts the scorecard and its comments in one transaction, then keeps the newest 20 rows per `(name, branch)` in the same transaction.
- **Stale** is derived on read: `head` is not the tip of `branch`, or the branch no longer exists. A stale scorecard is returned only when asked, and it never changes.
- `blocking_open` counts the open root comments of a scorecard whose decorations include `blocking`.

## Comments

- `label` and `decorations` are validated on write. A decoration is one of `blocking`, `non-blocking` or `if-minor`, or one of at most three free tokens matching `[a-z-]{1,20}`.
- Kind and severity are derived when absent, in one pure function: `question` gives Question; `praise`, `note` and `thought` give Note; the others give Flag. Severity: `issue`, `suggestion` and `todo` give medium; `nitpick` and `chore` give low; `blocking` gives high; the rest have none. An explicit kind or severity is kept.

## MCP tools

- `scorecard_publish`: the arguments of D219 (6). Refused when the name is not valid, when `head` is not the tip of `branch`, when a status or label is not listed, when a field is too long, or when more than 30 comments are given. A comment's lines are anchored as in D97, and a comment whose lines are not in the file now is refused. Returns `scorecard` and `comment_ids`.
- `scorecards_list`: read only. Filters by `name`, `branch`; `include_stale` (default false); `limit` (default 20, at most 20). Each row carries `stale` and `blocking_open`.
- `review_comment_create` takes `label` and `decorations`. Its kind and severity become optional.
- Removed: `verdict_record`, `verdicts_list`, `reviewers_list`, `gates_status`, `reviewer_propose`, `gate_propose`. No tool resolves a comment, publishes on a stale head or switches a workflow off.
- In a workspace, a call without `member` is refused, as the verdict tools were.

## Workflow API

- `ctx.scorecards({ name, branch })` (read): the same query as `scorecards_list`, with the limit of 20. It is a read, so it is not in the command log (D28).
- `ctx.change()` (read): returns `base`, `files` and `description`. The base is the stack parent, else the upstream (not the branch's own remote counterpart), else `origin/HEAD`, else the first of `main`, `master`, `develop` and `trunk` that exists and is not the branch (D214 (1)). Files are the three-dot diff names of the branch against that base. The description is `branch.<name>.description` in `.git/config`. More than 10,000 paths give a `truncated` flag.
- `meta.review`: read when the file loads. The **Run review…** picker lists the workflows that have it. A `review` workflow without a `manual` trigger is refused at load, with the reason.
- Loader: `.git/gitmaster/workflows/` in the repository's common directory, read by the same parser and trusted by hash (D101) like the user's folder. Nothing is written into the working tree.
- `GITMASTER_WORKFLOW_RUN`: set in the child environment of `ctx.exec` from the run id. It is a link for reading, not a check.

## Blocking and trust

- A push is blocked by `before.push` and a restack by `before.restack-apply`, both ordinary workflows (D75). They fail closed: an error or `ctx.block` vetoes. The per-repository switch is **Enabled in this repository**, as for any workflow.
- The native check in `handle()`, the `GatesBlocked` event, `BypassGates`, `gates.json` and the user and repository layers are removed (S2). Nothing replaces them in the engine.
- The CLI loads the checkout's own agent configuration. A reviewer on a branch with commits that are not the user's asks first with `ctx.approve`. The inventory in `branch_agent_config` (planned) can feed that prompt.

## Sub-slices

The order and the done-when of each are in `design/roadmap.md`, slice 8s:

- **S1** docs and D219, this note (this change).
- **S2** remove gates, proposals, approvals, reviewers and bypass from the engine, MCP and tests.
- **S3** the scorecards table, labels and decorations, the two MCP tools, `ctx.scorecards`, migration v11.
- **S4** `meta.review`, `ctx.change()`, `.git/gitmaster/workflows/`, `GITMASTER_WORKFLOW_RUN`, the two templates.
- **S5** the Review page: pure view models first, then GPUI, then `ui-smoke` in both themes.
- **S6** a live check of `claude -p` with MCP (tool names, the flags that deny shell and writes, environment passing, thread capture); Cursor CLI afterwards.
- **S7** the plugin skill `/gitmaster:write-reviewer`, in `ai/` (a separate repository; noted here only).

## Tests expected

- Pure: the label and decoration derivation table; decoration parsing and limits; name and length validation; staleness; a property test that any sequence of publishes keeps the newest 20 rows per `(name, branch)`; the migration from v10 keeps comments and drops the four tables.
- Real Git (`crates/gm-core/tests/scorecards_real_git.rs`): publish is refused unless `head` is the tip; a commit makes the scorecard stale; `ctx.change()` base for a stack parent, an upstream, `origin/HEAD` and the name fallback; the description round trip through `.git/config`; `GITMASTER_WORKFLOW_RUN` reaches the child process.
- Blocking: a `before.push` workflow that calls `ctx.block` vetoes a push into a temporary bare remote, and a script error vetoes too.
- MCP: the tool list has exactly the names above; no tool resolves a comment on a stale head.
- UI: pure view models for the status chips, the trend, the blocking count and the filters; then a `ui-smoke` scenario in both themes (S5).
- S6: captured `claude -p` runs kept as fixtures.

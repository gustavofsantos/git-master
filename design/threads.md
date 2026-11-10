# Threads: engineering design

The user-facing behavior is specified in `docs/guide/threads.md` and `docs/agents/agent-integration.md`; this file keeps the engineering rationale and the build plan.

Last updated: 2026-11-07 (sub-agent threads, D199 and Q45, planned at the end of the file; sharing, D187, and memories, D190, planned at the end of the file). Status: built except T0, T8 and T9 (see `design/roadmap.md`, slice 6t, for status and gaps; D130 for the choices made while building). The text below is the design as planned; where the build differs, D130 and the roadmap win. The author decided the direction on 2026-10-29 (D119 to D121); the details below are **proposed** (D122 to D128) and each one is listed in "To confirm" before the slice that depends on it starts.

## Why

An agent session holds the reasoning behind a change: what was asked, what the agent tried, which files it touched and why. Today that record lives in the agent's own session files, outside the repository, in a format that differs per agent, and it is gone when the agent's tool prunes it. Three things follow from keeping it in GitMaster:

1. **Accountability (pillar 3).** "What the agent did is observed, not self-reported." A thread is the observed record of a session, kept next to the repository it changed, and tied to the files and commits it touched.
2. **Memory for the next agent.** Once the MCP server exists (slice 7), an agent can search earlier threads ("why did we choose this retry policy?") instead of re-deriving the answer. The human can do the same in the window.
3. **Review.** The local review loop (slice 8) and a human reviewing agent output both need to see the conversation that produced a diff.

This serves pillar 3 only. It must not slow pillars 1 and 2: capture runs outside the UI process, and the window only reads.

## What is in

- Capture of agent sessions into one SQLite database per repository: what the human said, what the agent said, every tool call with its result, and every file edit the agent made.
- Redaction of secrets **before** anything is stored, with built-in patterns and the user's own patterns.
- Full-text search over threads, as an engine command (`SearchThreads`) that the UI uses now and the MCP server uses later (D08).
- A **Threads** page in the window: browse, search, read, open an agent edit as a diff, link a thread to Git objects, curate (title, note, star, tags, delete, export).
- The agent integration: a plugin for the agent harness (Claude Code first) that carries the capture hooks, the MCP configuration and a skill. It is source in the `git-master-ai` repository, included here as the `ai/` submodule, and Claude Code installs it from that repository (D150, D194).

## What is out

- Replaying or resuming a session from a thread, or continuing a conversation inside GitMaster. GitMaster is not an agent host.
- Capturing anything from an agent that does not run a hook or leave a transcript GitMaster can read. A harness without an adapter has no threads.
- Cross-repository search (storage is per repository, D122).
- Reasoning ("thinking") blocks of the model: not stored (Q31).
- Images and binary attachments: replaced by a placeholder with the type and size.
- Sharing or syncing threads between machines.
- Automatic summaries written by an LLM. No LLM runs inside the app (D69).
- Reading threads of agents that ran outside a Git repository.

## Terms

| Term | Meaning |
|---|---|
| Harness | The agent program the human runs: Claude Code first, others later. |
| Harness id | A stable lowercase name, `claude-code`, chosen by GitMaster, stored with every thread and passed as `--harness` to the hook command. It is a string, not a Rust enum, so a thread from a harness this build does not know still opens. |
| Adapter | The code in GitMaster that understands one harness: its hook payloads, its transcript format and its plugin layout. It is the only place that knows the harness's names and shapes. |
| Canonical event | GitMaster's own description of something that happened in a session, independent of any harness. Adapters produce them; everything else consumes them. |
| Thread | One agent session in one repository: from the harness's first event for a session id to its last. Identified by `(harness, session id)`. |
| Turn | One human prompt and everything the agent does until it stops. |
| Entry | One item of a thread in order: a prompt, an assistant message, a tool call, a tool result, a note from the harness. |
| Edit | A change an agent tool made to one file, with its patch. |

## Harnesses and the canonical model (D129)

Claude Code is the first harness. More will follow (Q27), so the capture is built around a harness-neutral core and a thin adapter per harness. The rule: **only an adapter knows a harness's event names, payload fields, transcript format, tool names and plugin layout; nothing downstream of the canonical events does.** Redaction, the store, search, the UI and MCP never see a harness-specific shape.

```text
 harness payload / transcript line
        │  HarnessAdapter (pure parse, per harness)
        ▼
 CanonicalEvent ──▶ redact ──▶ cap ──▶ ThreadStore (idempotent upsert) ──▶ UI / MCP
```

### Selecting the harness

`gitmaster thread hook --harness <id>` takes the harness id. A registry in `gm-core` maps ids to adapters at compile time (`harness::lookup(id)`); `gitmaster harness list` prints the known ids. An unknown id in the hook exits 0 and logs the id (rule 1 of the hook). The id is part of the thread's identity, so two harnesses can never collide on a session id.

### The adapter

Defined in `gm-core`, implemented per harness. The parse side is pure (bytes in, events out) so it is tested from fixtures without a database, a process or a repository.

```rust
pub trait HarnessAdapter: Send + Sync {
    /// Stable id stored with every thread, e.g. "claude-code".
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;

    /// One hook invocation: the stdin document to canonical events. Pure.
    fn parse_hook(&self, payload: &[u8]) -> Result<HookInput, ParseError>;

    /// Complete records of the harness's own session log from `from`. Reads a file, bounded.
    fn read_session_log(&self, path: &Path, from: u64) -> Result<LogChunk, ParseError>;

    /// Where existing sessions for a repository live, for import.
    fn discover(&self, worktree: &Path) -> Vec<SessionLog>;
}
```

`HookInput` carries the session id, the working directory, the log path when the harness gives one, a list of `CanonicalEvent`, and an optional `EditPhase` (`Before` or `After`, the call id and the path). `LogChunk` carries events and the new offset. Neither type has a harness-specific field.

`Capabilities` says what the harness can provide, so the pipeline degrades per harness instead of failing: `session_log` (a transcript to read), `pre_edit_event` (a hook before an edit, so a pre-image is possible), `tool_results` (hooks carry results), `subagents`, `resume_same_id`. A harness without `pre_edit_event` gets `from_input` edits (D123); one without `session_log` gets no assistant text unless its hooks carry it, and the page says which parts are missing for that harness.

Adapters do no redaction, no database access and no writes. They return events; the core redacts and stores them. This keeps the secrecy guarantee in one place, where a new adapter cannot forget it.

### Canonical events

These are the only input of the pipeline. Fields common to all: `harness`, `session_id`, `at` (ms), and a `source_id` that is the harness's own id for the item (a tool-use id, a log line id) or, when it has none, a hash of the harness id, session id and item position, so replays stay idempotent.

| Event | Fields | Becomes |
|---|---|---|
| `SessionStarted` | `cwd`, `model?`, `harness_version?`, `log_path?`, `origin` (`new`, `resumed`, `cleared`, `compacted`, `unknown`), `parent?` (the parent session and its `Agent` call, for a sub-agent; D199) | the thread row, with `parent_thread` and `parent_call` for a sub-agent |
| `PromptSubmitted` | `text` | a `user` entry; opens a turn |
| `AssistantMessage` | `text`, `model?` | an `assistant` entry |
| `ToolCalled` | `tool: ToolKind`, `native_tool`, `args` (JSON) | a `tool_call` entry |
| `ToolReturned` | `call_source_id`, `text`, `is_error` | a `tool_result` entry |
| `TurnEnded` | — | closes the turn of this session only; reads `HEAD` for links |
| `SessionEnded` | `reason` | `ended_at` |
| `Notice` | `text` | a `notice` entry (compaction, interruption) |

`ToolKind` is the canonical tool class: `Read`, `Edit`, `Write`, `Shell`, `Search`, `Web`, `Agent` (a sub-agent), `Other`. The adapter maps the native name to it and keeps the native name in `native_tool` so nothing is lost. The pipeline uses the kind, not the name: `Edit` and `Write` are what trigger pre and post images, and `Shell` is what T8 watches.

A sub-agent is a session of its own (D199). Its events carry its own `session_id` (`<session_id>/agent-<agentId>`), and its first event is `SessionStarted` with `parent` set. Its `TurnEnded` closes its own turn and never the parent's. No event carries a `parent_call`: the link is a property of the sub-agent's thread.

For an edit the adapter also provides an **edit intent** inside `ToolCalled.args` normalization: the repository-relative or absolute `path`, the change kind, and the old and new text when the harness gives them. The pipeline decides what to measure from the file (D123); the intent is the fallback.

### Mapping for Claude Code

The first adapter. The left column is the harness's contract and **unverified until T0** (Q26); the right is canonical. The sub-agent rows are also unverified until Q45 is answered (D199).

| Claude Code | Canonical |
|---|---|
| hook `SessionStart` (`source`: startup, resume, clear, compact) | `SessionStarted` with `origin` |
| hook `UserPromptSubmit` | `PromptSubmitted` |
| hook `PreToolUse` on `Edit`, `MultiEdit`, `Write`, `NotebookEdit` | `EditPhase::Before` and `ToolCalled` |
| hook `PostToolUse` | `ToolReturned`, `ToolCalled` if not seen, and `EditPhase::After` for edit tools |
| hook `Stop` | `TurnEnded` |
| hook `SubagentStop` | a trigger only: reads the sub-agent's transcript into its own thread (`TurnEnded` for the sub-agent, never for the parent; D199, Q45) |
| hook `SessionEnd` | `SessionEnded` |
| transcript `assistant` text block | `AssistantMessage` |
| transcript `tool_use`, `tool_result` | `ToolCalled`, `ToolReturned` (reconciliation) |
| transcript thinking block | dropped (Q31) |
| transcript compaction or summary record | `Notice` |
| transcript `user` text with `<command-name>` and `<command-args>` | `PromptSubmitted`, the command as typed, such as `/model opus` (D133) |
| transcript `user` text with `<bash-input>` | `PromptSubmitted`, `! <command>` |
| transcript `user` text with `<local-command-stdout>`, `<local-command-stderr>`, `<bash-stdout>`, `<bash-stderr>`, `<task-notification>` or an interruption | `Notice` |
| transcript `user` text with `<local-command-caveat>`, `<system-reminder>` or `<user-prompt-submit-hook>` | dropped; the text around it is kept |
| hook `PostToolUse` response, transcript `tool_result` | `ToolReturned` without its `<system-reminder>` blocks |
| tool `Read` | `ToolKind::Read` |
| tool `Edit`, `MultiEdit`, `NotebookEdit` | `ToolKind::Edit` |
| tool `Write` | `ToolKind::Write` |
| tool `Bash` | `ToolKind::Shell` |
| tool `Grep`, `Glob` | `ToolKind::Search` |
| tool `WebFetch`, `WebSearch` | `ToolKind::Web` |
| tool `Agent` (the name the harness uses now; earlier drafts of this table said `Task`, which the adapter must not map) | `ToolKind::Agent`, and starts a sub-agent thread (D199) |
| `Agent` call's `tool_result` (`toolUseResult.agentId`), and hook `PostToolUse` of an `Agent` call | a trigger only: the sub-agent's identity and its files `<transcript dir>/<session>/subagents/agent-<agentId>.jsonl` and `.meta.json` (`agentType`, `description`, `toolUseId`); see "Sub-agent threads" (D199, Q45) |
| any other tool, or an MCP tool | `ToolKind::Other`, name kept |

An event or record the adapter does not recognize is **not an error**: it is skipped, counted in a `capture_gap` row with the native type name. A payload that does not parse at all is a gap too. That makes harness version drift visible instead of silent.

### Adding a harness

A new harness is: one module implementing `HarnessAdapter`, its fixtures, one line in the registry, and a row in the docs. No change is needed in `gm-schema` commands, the store, redaction, search, the Threads page or the MCP tools. If a harness has a concept the canonical model lacks, the model is extended first, for all harnesses, by a decision (D129), not by a harness-specific column.

### Conformance kit

`crates/gm-core/tests/harness_conformance.rs` runs once per registered adapter, over `tests/fixtures/threads/<harness id>/`:

- **Golden:** each payload fixture parses to the expected canonical events (stored as JSON beside it).
- **Generic properties, same for every adapter:** parsing never panics on arbitrary bytes; it is deterministic; every event has a session id and a stable `source_id`; no event is produced twice from one payload; reading a log from offset 0 in one pass equals reading it in arbitrary chunks; every native tool name in the fixtures maps to a `ToolKind` or to `Other`.
- **Pipeline:** replaying the fixtures through the real pipeline twice gives one identical database, and the repository is unchanged outside `.git/gitmaster/`.
- **Capabilities honesty:** an adapter that declares `pre_edit_event` must emit `EditPhase::Before` in a fixture, and so on.

A harness is not merged until its adapter passes the kit.

## Capture pipeline

```text
agent harness ──hook event (JSON on stdin)──▶ gitmaster thread hook
      │                                          │ 1. find repo from cwd
      │                                          │ 2. capture enabled? patterns valid?
      └──writes its own transcript file ◀────────┤ 3. read new transcript lines from the stored offset
                                                 │ 4. normalize ▶ redact ▶ cap
                                                 │ 5. one SQLite transaction (WAL)
                                                 ▼
                                   .git/gitmaster/threads.db ──file watcher──▶ window
```

### The plugin (D120, D150)

The Claude Code plugin is source in the `git-master-ai` repository (the `ai/` submodule here, D194), under `plugins/claude-code/`, and its marketplace is `.claude-plugin/marketplace.json` at the root of that repository. The binary does not generate it. For Claude Code it is a plugin directory that holds:

- `.claude-plugin/plugin.json`: name `gitmaster`, its own semantic version, with the minimum GitMaster version in `plugins.json` (a test here checks that the app is not older).
- `hooks/hooks.json`: one command hook per event: `SessionStart`, `UserPromptSubmit`, `PreToolUse` (matcher: the file-editing tools), `PostToolUse` (all tools), `Stop`, `SubagentStop`, `SessionEnd`. Every command is `gitmaster thread hook --harness claude-code`, with a short `timeout`. It names `gitmaster` without a path, so the hook runs the `gitmaster` on `PATH`; the plugin carries no absolute path of the checkout.
- `skills/gitmaster/SKILL.md`: when the review and thread tools help, and that their text is untrusted.
- `.mcp.json`: starts `gitmaster mcp`, one process per session.

The user installs it with the two `claude plugin` commands of [Agent integration](../docs/agents/agent-integration.md), against the repository's address or a local checkout, and updates or removes it with `claude plugin update` and `claude plugin uninstall`. The app shows the same commands on the empty Threads page. GitMaster never edits the harness's own settings files by itself, and never requires the harness to be installed (D51). Stored threads stay unless the user deletes them.

The exact event names, payload fields, plugin layout and install commands are the harness's contract, not ours. T0 captures real payloads into fixtures and corrects this section before T3 starts (Q26).

### The hook command (D124)

`gitmaster thread hook` is a one-shot subcommand, like `gitmaster workflow check` (D102): it reads one JSON document from stdin, does its work and exits. It starts no resident process and opens no window, and it is dispatched before any GPUI initialization. It is not a headless mode (D02).

Rules, in order of priority:

1. **Never hurt the agent.** Exit status is always `0` and nothing is written to stdout. A hook that blocks, slows or breaks the agent's tool call would make people uninstall the integration. Errors go to a bounded log in `.git/gitmaster/threads.log` (rotated at 256 KiB).
2. **Fast.** Budget: p95 under 100 ms of wall time in a release build for a typical event, hard stop at 2 s. If the cold start of the full binary breaks this budget (the binary links the GUI libraries), the hook becomes a small separate binary built from the same crates minus `gm-ui` (Q25).
3. **Redact first.** Raw text is redacted in memory before it is written anywhere, including the spool.
4. **Idempotent.** Every entry has a stable `source_id` (the harness's tool-use id or transcript line id). Writing the same event twice leaves the database unchanged, so a retry, a replayed spool or a hook that overlaps the transcript is harmless.
5. **Bounded input.** Stdin is read up to 8 MiB. A larger event is stored as a placeholder with its size.
6. **Resolve the repository, ignore the rest.** `cwd` of the payload goes to `git rev-parse --git-common-dir`. Outside a repository, or in a repository where capture is off, the event is dropped.

If the database is locked beyond a 500 ms busy timeout, or the disk is full, the already-redacted event is written to `.git/gitmaster/threads-spool/<time>-<random>.json` (at most 100 files, oldest dropped first, each at most 8 MiB) and drained by the next hook run or by the app when the repository opens.

### Hooks and the transcript (D120)

The description below is the Claude Code adapter's; the pipeline treats the hook and the log through the capabilities above.

Hooks tell us about tool calls reliably and in time. They do not carry the assistant's text. The transcript the harness writes (`transcript_path` in the payload) has both. So:

- The **hook event** is the source for tool calls and their results (it has the tool input and response), for prompts (`UserPromptSubmit`) and for the session's start and end.
- The **transcript** is the source for assistant text, and the **reconciliation** source for everything else. On each event the handler reads the transcript from the stored byte offset to the end, keeps only complete lines, and inserts any entry whose `source_id` is not in the database yet. A tool call that a hook missed (hook not yet installed, handler failed, spool full) is filled in from the transcript.
- The offset is stored per thread in the same transaction as the entries, so a crash never skips or repeats lines. A transcript that shrinks or whose first line changed (rewritten, compacted) resets the offset and re-reads; the `source_id` rule makes that safe.

Reading the transcript is what makes the record observed rather than reported: it is the harness's own log, not a summary the agent wrote for us.

### Edits (D123)

For `Edit`, `MultiEdit`, `Write` and `NotebookEdit`:

1. `PreToolUse` reads the file's current bytes (up to 2 MiB) into a temporary pre-image keyed by the tool-use id, in `.git/gitmaster/threads-tmp/`. The file is not stored; only its blob id (`git hash-object`, without `-w`) and bytes are kept until the post event.
2. `PostToolUse` reads the file again and builds the patch with libgit2's `Patch::from_buffers` (already linked, no new dependency). The patch is redacted and capped (256 KiB), and stored with the path relative to the worktree root, the change kind (`create`, `modify`, `delete`, `rename`), counts of added and removed lines, and the pre and post blob ids.
3. The temporary pre-image is deleted. A pre-image with no post event within 10 minutes is deleted by the next run.
4. If no pre-image exists (hook installed mid-session, file over the limit, binary), the edit is recorded from the tool input alone (old and new strings, or the written content), flagged `from_input`, and shown with the caveat that it was not measured from the file.

The blob ids are what lets the window say, deterministically, whether an edit is still in the tree: the post blob equals the file's blob in the working tree, in `HEAD`, or neither (D126). No guessing about "similar" content.

Changes the agent makes through other tools (a shell command, a formatter, a code generator) have no `Edit` event. They are not in v1's edit list; the thread shows the shell command and its output. **T8** covers them with a turn-level working-tree snapshot, only if the cost is measured acceptable (see the risks).

## Redaction (D121)

The guarantee: **a secret that matches a pattern never reaches the database, the search index, the spool or the log.** Every text field is redacted before it is written: prompts, assistant text, tool arguments, tool results, patches, file paths and notes.

### Pipeline

1. Cap raw input at 8 MiB. Over that, store a placeholder.
2. Apply path rules: a tool result or edit whose file path matches a sensitive-path glob has its text replaced by `[REDACTED:sensitive-file]`. The path stays; the content does not.
3. Collect every match of every enabled pattern in the field, merge overlapping ranges, and replace each merged range once. The name of the first pattern in priority order labels the range, so the result is deterministic (D09).
4. Truncate **after** redaction, never before, so a secret cut in half by the cap cannot survive as an unmatched prefix.
5. Count redactions per pattern name on the thread. The counts are kept; the values never are.

A replacement is `[REDACTED:<pattern name>]`. It contains nothing any pattern matches, so redaction is idempotent: redacting twice equals redacting once. Scrub (below) relies on this.

### Patterns

Built-in patterns, each with a name, a regex and, where the key must stay readable, a capture group that marks only the value. The list is a starting point; the test vectors in T1 are the contract, and each vector is built at test time from pieces so the repository itself never holds a string that looks like a live key:

| Name | Covers |
|---|---|
| `private-key` | PEM blocks (`-----BEGIN … PRIVATE KEY-----` to `-----END …-----`); an unterminated block runs to the end of the field |
| `aws-access-key-id` | `AKIA`, `ASIA` and the other four-letter AWS prefixes followed by 16 characters |
| `aws-secret-access-key` | a 40-character secret in an assignment whose name mentions it |
| `github-token` | `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_` |
| `gitlab-token` | `glpat-` |
| `slack-token` | `xoxb-`, `xoxp-`, `xoxa-`, `xoxr-`, `xoxs-` |
| `stripe-key` | `sk_live_`, `rk_live_` |
| `google-api-key` | `AIza` followed by 35 characters |
| `anthropic-api-key` | `sk-ant-` |
| `openai-api-key` | `sk-` and `sk-proj-` followed by a long token |
| `npm-token` | `npm_` followed by 36 characters |
| `jwt` | three base64url segments, the first two starting `eyJ` |
| `auth-header` | the value of `Authorization:` with `Bearer`, `Basic` or `Token` |
| `url-credentials` | the password in `scheme://user:password@host` |
| `secret-assignment` | the value in `NAME=value`, `"name": "value"` or `name: value` when the name contains `password`, `passwd`, `secret`, `token`, `api_key`, `apikey`, `private_key` or `access_key`, and the value is at least 8 characters |

`secret-assignment` is the broad one and the likeliest to hide useful text (a variable called `token_count`). It matches the name list as whole words and skips values that are plainly not secrets (`true`, `false`, `null`, a number, an empty string, an environment-variable reference such as `$TOKEN` or `${TOKEN}`). The user can turn off any built-in by name.

The user adds patterns in `settings.json` under `threads.redact` (D127):

```json
{
  "threads": {
    "capture": true,
    "redact": {
      "disabled_builtins": ["jwt"],
      "patterns": [
        { "name": "acme-token", "regex": "acme_[A-Za-z0-9]{32}" },
        { "name": "corp-host", "regex": "[a-z0-9-]+\\.corp\\.example\\.com", "group": 0 }
      ],
      "paths": [".env", ".env.*", "*.pem", "id_rsa*", "**/secrets/**"]
    }
  }
}
```

- The regex engine is the `regex` crate (D51), which runs in linear time, so a pattern cannot hang the hook. User patterns are built with a size limit; a pattern that fails to build is **invalid**.
- `group` (optional) is the capture group that holds the secret. Default: the whole match.
- `paths` are globs on repository-relative paths; the defaults above apply unless the user sets the list.
- **Fail closed (D121):** while any enabled pattern is invalid, capture in every repository pauses. Events are dropped, not stored raw, and counted; the Threads page says "N events not captured: pattern `acme-token` is invalid". Completeness is traded for the guarantee.
- A **test field** in the settings dialog (and `gitmaster thread redact-test`) takes a sample text and shows the result, so a pattern is checked before it is relied on.
- **Scrub** re-applies the current patterns to every stored thread. Because stored text is already redacted, scrub only adds redactions. It is the answer to "I added a pattern after the fact". It runs off the UI thread, in batches, and reports how many entries changed.

### What redaction does not do

Stated in the docs as well:

- It cannot reach the harness's own transcript files, which keep the original text. GitMaster reads them and never changes them.
- A pattern only catches what it describes. A password in prose ("it's hunter2") is not caught. The user's patterns and the path rules are the second line; deleting a thread is the last.
- Redaction is not reversible. There is no "show original".
- Entropy-based detection is not in v1 (Q28): it rewrites useful text (hashes, ids) too often.

## Storage (D122)

`.git/gitmaster/threads.db`, in the Git common directory, so every worktree of the repository shares it, like `review.db` and `state.db`. SQLite through `rusqlite` with bundled SQLite (already a dependency). Pragmas: `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`, `secure_delete=ON`, `busy_timeout=500`. The file is created with mode `0600`. Schema version in `user_version`; a migration is a numbered function, tested from every older version. The file is disposable only in the sense that Git does not need it: deleting it loses the threads, and the transcripts it came from can be re-imported (T3 offers "Import existing sessions" for a repository).

```text
thread(
  id            INTEGER PRIMARY KEY,
  uid           TEXT UNIQUE NOT NULL,   -- first 16 hex of sha256(harness || 0x00 || session_id)
  harness       TEXT NOT NULL,          -- harness id, 'claude-code'
  harness_version TEXT,                 -- as reported by the harness, if it does
  session_id    TEXT NOT NULL,
  worktree      TEXT NOT NULL,          -- cwd's worktree root at the first event
  branch        TEXT,                   -- at the first event; NULL when detached
  head_start    TEXT, head_end TEXT,    -- commit ids
  model         TEXT,
  title         TEXT NOT NULL,          -- from the first prompt, editable
  title_edited  INTEGER NOT NULL DEFAULT 0,
  note          TEXT NOT NULL DEFAULT '',
  starred       INTEGER NOT NULL DEFAULT 0,
  started_at    INTEGER NOT NULL,       -- ms since the Unix epoch
  last_event_at INTEGER NOT NULL,
  ended_at      INTEGER,                -- NULL while the session is open
  transcript    TEXT,                   -- path, for reconciliation
  cursor        INTEGER NOT NULL DEFAULT 0,  -- byte offset read in the transcript
  entry_count   INTEGER NOT NULL DEFAULT 0,
  edit_count    INTEGER NOT NULL DEFAULT 0,
  UNIQUE(harness, session_id))

entry(
  id         INTEGER PRIMARY KEY,
  thread_id  INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
  seq        INTEGER NOT NULL,          -- order inside the thread
  turn       INTEGER NOT NULL,          -- 1-based, increments on each human prompt
  kind       TEXT NOT NULL,             -- 'user' | 'assistant' | 'tool_call' | 'tool_result' | 'notice'
  source_id  TEXT NOT NULL,             -- harness id; UNIQUE per thread
  parent_id  TEXT,                      -- not written from schema v9: a sub-agent's entries live in its own thread (D199)
  tool       TEXT,                      -- tool name for tool_call / tool_result
  text       TEXT NOT NULL,             -- redacted, capped
  args_json  TEXT,                      -- redacted tool input, capped
  is_error   INTEGER NOT NULL DEFAULT 0,
  truncated  INTEGER NOT NULL DEFAULT 0,
  orig_bytes INTEGER,                   -- size before the cap
  redactions INTEGER NOT NULL DEFAULT 0,
  at         INTEGER NOT NULL,
  UNIQUE(thread_id, source_id), UNIQUE(thread_id, seq))

edit(
  id INTEGER PRIMARY KEY, thread_id INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
  entry_id INTEGER NOT NULL REFERENCES entry(id) ON DELETE CASCADE,
  path TEXT NOT NULL, old_path TEXT, change TEXT NOT NULL,
  patch TEXT NOT NULL, added INTEGER NOT NULL, removed INTEGER NOT NULL,
  pre_oid TEXT, post_oid TEXT, from_input INTEGER NOT NULL DEFAULT 0, at INTEGER NOT NULL)

thread_link(thread_id, kind /* 'commit' | 'branch' | 'file' */, ref, origin /* 'observed' | 'manual', becomes 'human' in v5 */,
  PRIMARY KEY(thread_id, kind, ref))

thread_tag(thread_id, tag, PRIMARY KEY(thread_id, tag))
thread_redaction(thread_id, pattern, count, PRIMARY KEY(thread_id, pattern))
capture_gap(thread_id, at, reason)       -- events dropped for a reason the user can see
entry_fts  -- FTS5, external content over entry(text, tool); edit_fts over edit(path)
```

Limits (starting values, tuned in T2): per field 64 KiB for results and 256 KiB for patches (head and tail kept, `truncated` set, `orig_bytes` kept); 20,000 entries per thread, after which further entries are dropped and a `capture_gap` row says so; no limit on thread count, and no automatic deletion (Q29).

### Search (D128)

FTS5 over the conversation and edit paths. What is indexed (D132, schema 3): the word and trigram indexes hold only the text of `user` and `assistant` entries, through the view `entry_msg`, and the paths of edits. Tool calls, tool results and notices are stored and shown in the thread but not indexed, and neither is the tool name. The user's text is turned into a query by the engine, never passed raw: words become prefix terms joined by AND, a double-quoted phrase stays a phrase, and FTS operators typed by accident are escaped. Results are ranked by `bm25` and returned with a snippet around the match, the thread, the turn and the entry; a hit is always a conversation entry or an entry that edited a matching path. Filters (agent, branch, date range, starred, tag, touched path, has edits) are columns, not syntax, as in D50. The tokenizer keeps identifiers whole (`file_index`, `HistoryStream`) and is chosen in T2 against a corpus of real prompts and code; target: 100 ms for the 95th percentile on 1 million entries.

**Fallback (D131).** When the strict search gives fewer than eight hits, the same filter runs two more stages over FTS5 `trigram` indexes (`entry_tri` over the conversation, `edit_tri` over paths; schema 3, see D132): substring matches, then close misspellings. The misspelling stage's trigram OR query only gathers candidates; a candidate is kept when every typed word of four or more characters has a word of its text at trigram Dice similarity 0.5 or more (`query::trigram_similarity`, padded trigrams, checked in Rust). Each stage appends only entries not yet listed and never exceeds the limit. Hits carry `kind` (`exact`, `substring`, `fuzzy`); the window marks the last two `approx.`. The trigram tables are purged with the word indexes on delete and scrub.

**Migration (D132).** Schema 3 drops the word and trigram indexes over `entry` and rebuilds them over `entry_msg` from the rows that qualify. A database of fewer than 50,000 entries is vacuumed after the migration, which takes the dropped index pages out of the file. A larger one is not vacuumed; its pages are reclaimed by later checkpoints. The hook process (`ThreadStore::open_for_hook`) never runs this migration on a database of 50,000 entries or more: it reads through a read-only handle and spools its redacted batch, The next hook run, which finds the database migrated, stores the spool, as it does for a busy database.

**Repair (D133).** Schema 4 runs the adapter's cleaner over every stored entry, inside the same migration transaction, through the `TextCleaner` the store is given (`harness::clean_stored`, dispatched by harness id). Entries that are harness markers only are deleted with their edits and the counters are recounted; a cleaned entry takes its new text; a notice also takes its new kind, which moves it out of the search index (the update triggers follow the kind). Only the first piece of a message is kept. A title derived from a prompt that changed is derived again from the first remaining prompt, unless the person edited it.

## Links to Git (D126)

Links are facts, never guesses:

- **`file`**: every path in the thread's edits. Derived when the edit is stored.
- **`branch`**: the branch checked out at the first event, observed.
- **`commit`, origin `observed`**: each commit that appeared on the thread's branch between two events of the thread (`head_start`, `head_end` and the `HEAD` seen on each `Stop`). The label in the window is "created during this thread", not "made by the agent": a human commit in the same window of time counts too.
- **`commit`, origin `manual`**: the user links a thread to a commit by hand.
- **Edit state**, derived on read, never stored: `in working tree` (the file's blob equals `post_oid`), `in HEAD` (the blob at `HEAD` equals it), or `changed since` (neither).

No similarity matching, no "probably this commit".

## Engine surface (D08)

Commands and events in `gm-schema`; the UI and the MCP front end use the same ones. Reads are not in the command log; writes that change a thread's metadata or delete data are (D28).

| Command | Level | Actor | Notes |
|---|---|---|---|
| `ListThreads { filter, cursor, limit }` | read | any | Cursor paging by `(last_event_at, id)`, like H2. |
| `SearchThreads { query, filter, limit }` | read | any | Cancellable; late answer dropped by `OperationId`. |
| `ReadThread { uid, from_seq, limit }` | read | any | Entries in order, with edits. Default limit keeps output small. |
| `ReadThreadEdit { id }` | read | any | The patch and the derived state. |
| `UpdateThread { uid, title?, note?, starred? }` | apply | human | Edits metadata only; never entry text. Tags are labels from schema v5. |
| `LinkThread` / `UnlinkThread` | apply | human | Manual commit links. |
| `DeleteThread { uid }` / `DeleteThreadEntry { id }` | apply | human | A real delete (`secure_delete`); confirmation lists what goes. |
| `ExportThread { uid, path }` | read | human | Markdown of the stored text; redaction happens at ingest and scrub, never on export. |
| `ScrubThreads` | apply | human | Re-applies the patterns, off-thread, with progress. |
| `ImportThreads { agent }` | apply | human | Reads existing session files for this repository. |

Ingest is **not** a `Command`. It runs in the hook process, which has no engine and no operation state, and it writes only GitMaster's own database; it does not touch the repository. The engine in the window opens `threads.db` read-mostly (reads, plus its own writes for the commands above) and learns about new data from a watcher on the database file (`-wal` included), debounced like the status watcher, which emits `ThreadsChanged { uids }`. No socket is needed, so the capture works with the app closed.

An agent can call the reads, which are exposed through MCP in T9 (and the provenance and label reads in P2 and P3, see below), and the three **record** commands `CreateLabel`, `AttachLabel` and `LinkThreads` (D146). Record commands write only `threads.db`, carry no approval marker and record the actor on the row. The agent actor is refused for every other write: anything that changes thread text, deletes a thread or an entry, renames or deletes a label, or detaches a label or a link.

## UI

See `design/screens.md` (Threads page) and `docs/guide/threads.md`. The page follows the History page's pattern: a virtualized list on the left (title, agent chip, branch, relative time, counts of edits, a redaction chip when something was redacted), the detail on the right. The detail groups by turn: the human's prompt, the agent's text, then its tool calls as one-line collapsed chips (tool, argument summary, status, duration); an edit is a card with the path, `+n −m`, the derived state, **Open Diff** (the read-only commit diff view with the patch) and **View File** (the Code page). Search results show snippets and open the thread at the matching entry. The model is pure and tested without a window; the view is thin. Long lists and long threads are virtualized, and loading a thread reads only the visible range plus a margin. A sub-agent run is a collapsed row under the `Agent` call that started it, and opens as a thread of its own; its rules are in "Sub-agent threads" (D199).

## Settings and files

| What | Where |
|---|---|
| Threads database | `.git/gitmaster/threads.db` (+ `-wal`, `-shm`) |
| Spool, temporary pre-images, log | `.git/gitmaster/threads-spool/`, `threads-tmp/`, `threads.log` |
| Capture switch, redaction patterns, paths, limits | `settings.json`, key `threads` (D127) |
| Per-repository pause | `gitmaster.db` in the app data directory, keyed by the repository's common directory (nothing is written to the repository's config) |
| Plugin source | `ai/plugins/claude-code/` and `ai/.claude-plugin/marketplace.json`, the `git-master-ai` submodule (D150, D194); nothing is written at run time |

## Tests

- **Pure**: redaction (a vector per built-in pattern, near-misses that must not match, user patterns, groups, overlap merging, path rules), normalization of each fixture, query building, derived states.
- **Property**: redaction is idempotent; for any text and any enabled pattern set, no match of an enabled pattern remains in the output; truncation never splits a replacement marker; redaction never panics on arbitrary bytes and runs in time linear in the input; normalizing the same payload sequence twice yields the same rows; **ingest is idempotent and order-tolerant** (replaying any prefix, with duplicates, gives the same database as one clean pass); the cursor never skips or repeats a transcript line.
- **Fixtures** (T0): real hook payloads and transcript files captured from the harness, scrubbed, with a test that fails when the harness's format drifts from the fixtures.
- **Real SQLite and real Git**: schema and migrations from every version; FTS5 availability and ranking; two processes writing at once (the hook and the app); a killed writer leaves a consistent file; an edit's blob ids against `git hash-object`; the repository and its working tree are byte-identical before and after a capture (nothing written outside `.git/gitmaster/`).
- **Hook process**: stdin with garbage, with an oversized document, with no repository, with a locked database (spool path), with an invalid pattern (fail closed); exit status is always 0; cold start and p95 are measured in a release build, as an ignored benchmark.
- **UI model**: list paging, filters, search state with late answers dropped, turn grouping, collapse state, selection survives a refresh.
- No smoke scenario by default (D88); the author may ask for one.

## Threat model

- **The stored text is untrusted input to any agent that reads it.** A prompt written months ago, or a tool result that quoted a web page, can carry instructions. MCP results mark thread text as untrusted, as for commit messages (`docs/concepts/trust-and-safety.md`), and no value from a thread becomes an argument to an apply tool except through an approved plan.
- **An agent reading threads reads other sessions' text**, including conversations about unrelated work. The MCP tools are Read level, and the policy can turn them off (T9). The default is on only once the user enables the MCP server.
- **The database is as sensitive as the transcripts it came from.** `0600`, inside `.git`, per repository. Nothing leaves the machine; there is no network access in this feature.
- **The hook runs as the user with the agent's inputs.** It never executes any text from the payload, never uses a payload value as an argument to a program except `git rev-parse` with `cwd` as the working directory, and treats paths as data (`..` and absolute paths that leave the worktree are stored as given but never opened).
- **Tampering by the agent.** The agent can write to `.git` like any process. A thread is an observed record, not a tamper-proof one; this is stated in the docs. Hardening (signed entries) is out of scope.

## Risks

- **Harness contract drift.** Hook payloads and transcript formats are the harness's. Mitigations: T0 fixtures, an adapter per harness behind one trait, and a `capture_gap` row plus a visible warning when a payload does not parse.
- **Hook cold start.** See Q25; the measurement gates T3.
- **Secrets that no pattern describes.** Mitigated by path rules, user patterns, scrub and delete; never eliminated. The docs say so.
- **Database growth.** Tool results dominate. Caps per field and per thread; per-thread sizes shown in the page; the delete and export actions; retention is Q29.
- **T8 cost.** A working-tree snapshot per turn uses the temporary-index `add -A` of the review snapshot (D97), whose cost on large repositories is unmeasured. T8 starts with a release-build measurement on a 100,000-file repository and ships only inside the "smooth and fast" bar.
- **Concurrent writers.** The hook and the app write the same file. WAL, short transactions and the busy timeout cover it; the two-process test is part of T2.

## Sub-slices

Each is usable on its own and ends with `make check` green. Order: T0, T1, T2, T3a, T3b, T4, T5, T6, T7; T8 and T9 follow their own gates.

### T0: contract and measurements (spike; no product code)

Run on a machine that has the harness (the build container does not). Deliverables, committed as fixtures and notes under `crates/gm-core/tests/fixtures/threads/` and a findings section in this file:

1. Real payloads of every hook event of one session that edits files, runs a shell command, uses a sub-agent and is resumed; the matching transcript; the plugin layout and install commands the harness accepts today. Correct "The plugin" above.
2. Cold start of `gitmaster thread hook` (stub that parses stdin and opens SQLite) in a release build on Linux and macOS: p50, p95. Decides Q25.
3. FTS5 present in the bundled SQLite; tokenizer comparison on a corpus of real prompts and code.
4. Redaction throughput of the built-in set on 1 MiB, 10 MiB.

Done when: fixtures are in the tree, scrubbed; Q25 and Q26 are resolved in `design/decisions.md`; numbers are recorded.

### T1: model and redaction (pure)

`gm-schema`: `ThreadSummary`, `ThreadEntry`, `ThreadEdit`, filters, the new commands and events. `gm-core/src/threads/`: `model`, `redact` (patterns, merge, caps, paths, config parsing and validation, scrub), `query` (user text to FTS query).

Done when: the vectors, the property tests above, and the invalid-pattern behavior pass; `settings.json` round-trips the `threads` key and keeps unknown keys; `gitmaster thread redact-test` works.

### T2: store and search

`ThreadStore` port with the SQLite adapter, migrations, FTS5, limits, `secure_delete`, spool drain; engine handlers for `ListThreads`, `SearchThreads`, `ReadThread`, `ReadThreadEdit`; the file watcher and `ThreadsChanged`.

Done when: real SQLite tests (migrations, idempotent ingest, two writers, killed writer, caps, delete leaves no text in the file), search relevance tests, and a benchmark (ignored by default) on 1 million entries meeting the target; reads are not in the command log.

### T3a: harness-neutral capture

The `HarnessAdapter` trait and its types, the canonical events, the registry and `--harness` selection, the `thread hook` subcommand and pipeline (redact, cap, idempotent upsert, cursor, spool, edit pre and post images driven by `ToolKind` and `EditPhase`), `capture_gap` for unknown events, and the conformance kit with a test adapter of its own (a tiny fake harness in the tests), so the core is proven before any real harness.

Done when: the conformance kit passes for the fake adapter; the idempotence and order properties pass; an unknown harness id exits 0; a real Git repository is unchanged outside `.git/gitmaster/`; the hook exits 0 in every failure test.

### T3b: Claude Code adapter

The adapter from T0's fixtures: hook parsing, transcript reader, tool mapping, resumed sessions, `discover` and `ImportThreads`. Sub-agents are built in the sub-agent slices (D199), not here.

Done when: the conformance kit passes for `claude-code` with the real fixtures; the idempotence and order properties pass; a real Git repository is unchanged outside `.git/gitmaster/`; the measured p95 of the hook process meets the budget or Q25's fallback is built.

### T4: integration installer

Superseded by D150. The plugin is source in the repository and Claude Code installs it with `claude plugin`. The generator, `gitmaster harness install | uninstall`, the **Install Agent Integration…** dialog and its palette action were removed, and so was the `plugin_install` capability. `gitmaster harness list` stays. The checks that remain: the plugin matches the app's version, its hooks cover the capture events with `gitmaster thread hook`, and its MCP server is `gitmaster mcp` (`crates/gitmaster/tests/claude_plugin.rs`).

### T5: Threads page

Sidebar entry, list, filters, search, detail by turn, edit cards with **Open Diff** and **View File**, empty states (no integration, no threads, capture paused), the capture-paused banner with the reason. Action `threads.show` in the registry, no default shortcut.

Done when: the pure models are tested; the page is virtualized; superseded searches are dropped; a thread of 20,000 entries scrolls smoothly in a release build; checked in a window in both themes (or the gap is stated, since the agent's container has no display).

### T6: curate and link

Rename, note, star, tags, delete (thread and entry), export to Markdown, manual commit links, observed links, the derived edit state, **Show Threads** on a Working Copy file and on a commit, and the context menu and palette entries (registry actions with reasons when disabled).

Done when: every action is in the registry and reachable by menu and palette; deletes are confirmed and verified at the file level; export writes the stored text; the file and commit entry points show only their own threads.

### T7: redaction settings

The **Threads** section of the settings dialog, or, until slice 9, a **Threads Settings…** dialog from the Threads page: capture switch, per-repository pause, built-in toggles, user patterns with the test field, path rules, **Scrub Threads**, limits.

Done when: an invalid pattern shows inline and pauses capture with the visible reason; scrub reports the count; settings persist in `settings.json` only.

### T8: observed turn changes (optional, gated)

A working-tree snapshot at `UserPromptSubmit` and at `Stop` through the temporary index, and the diff between them as "Changes during this turn", for edits no tool event covers. Starts with the release-build measurement; ships only if it fits the bar.

### T9: MCP read tools (with slice 7)

`threads_search`, `thread_get`, `thread_read` over the same commands, untrusted-marked output, small by default; the `gitmaster-threads` skill in the plugin; the policy switch. Done when an agent finds a decision made in an earlier thread and cites its `uid` and entry.

## Graph, provenance and labels (D143 to D146)

Decided by the author on 2026-10-31 (D143 to D146); Q34 is answered (N = 3, see `decisions.md`). The user-facing behavior is in `docs/guide/threads.md` and `docs/reference/mcp-tools.md`, marked `Planned`. This section is the build plan.

### Edges

- **Derived** edges are computed on read and never stored: a thread edited a file (`edit.rel_path`), a commit appeared on the thread's branch during the thread, blame owns a line range, and an edit's `post_oid` equals a file's blob in a commit.
- **Stored** edges are label attachments, thread-to-thread links and the commit links D126 observes (the time window cannot be recomputed). Every stored row has an `origin`: `observed` (GitMaster, from a fact), `human` or `agent:<client>`.

### Schema (v5)

```text
label(
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  color       TEXT,                    -- '#rrggbb', NULL for none
  origin      TEXT NOT NULL,           -- 'human' | 'agent:<client>'  (a label is never 'observed')
  created_at  INTEGER NOT NULL)

label_attach(
  label_id     INTEGER NOT NULL REFERENCES label(id) ON DELETE CASCADE,
  kind         TEXT NOT NULL,          -- 'thread' | 'file' | 'commit' | 'label'
  ref          TEXT NOT NULL,          -- thread uid | repo-relative path | commit id | label id
  origin       TEXT NOT NULL,
  at           INTEGER NOT NULL,
  subject      TEXT, author TEXT, committed_at INTEGER,  -- kind 'commit': snapshot, so a collected commit still shows
  resolved     TEXT,                   -- kind 'commit': cached id after a rewrite; never overwrites ref
  PRIMARY KEY(label_id, kind, ref))

thread_rel(                            -- thread-to-thread link; not thread_link, which holds commit, branch and file links
  from_thread INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
  to_thread   INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL CHECK (kind IN ('continues', 'relates', 'supersedes')),
  origin      TEXT NOT NULL,
  at          INTEGER NOT NULL,
  PRIMARY KEY(from_thread, to_thread, kind))

thread_link (existing, D126)           -- origin 'manual' becomes 'human' in the migration; add subject, author, committed_at and resolved for kind 'commit'

edit                                   -- add rel_path TEXT, repo-relative, derived from thread.worktree
CREATE INDEX edit_rel_path ON edit(rel_path)
```

Migration to v5, in one transaction, tested from v4: `thread_tag` rows become `label` rows (origin `human`) and `label_attach` rows of kind `thread`; then `thread_tag` is dropped. `edit.rel_path` is filled from `path` and `thread.worktree`.

Resolved in the decisions: one `origin` vocabulary (`manual` becomes `human`); observed commit links stay stored; the migration fills the snapshot of existing commit links from Git while the commit exists and leaves it empty otherwise; each `thread_tag` becomes a `human` label attached to its thread, and `UpdateThread` loses `tags`.

### Commands (D08 table style)

| Command | Level | Actor | Notes |
|---|---|---|---|
| `ReadProvenance { path, from_line, to_line }` | read | any | Blame, then threads, entries and labels, each with its evidence. Bounded. Not in the command log. |
| `ListLabels { query, limit }` | read | any | Labels with their attachment counts. |
| `ReadLabel { id }` | read | any | The label and what is attached to it. File paths are followed with `git log --follow` on read. |
| `CreateLabel { name, description, color }` | record | any | Writes `threads.db` only. Actor recorded. |
| `AttachLabel { label, kind, ref }` | record | any | Origin recorded. A commit attachment stores its snapshot. |
| `LinkThreads { from, to, kind }` | record | any | One `thread_rel` row. |
| `RenameLabel { id, name }` | apply | human | |
| `DeleteLabel { id }` | apply | human | Removes its attachments. |
| `DetachLabel { label, kind, ref }` | apply | human | Removes one attachment. |
| `RemoveThreadRelation { from, to, kind }` | apply | human | Removes one `thread_rel` row. Named apart from `UnlinkThread`, which removes a commit link. |

Record commands are written to the command log with their actor (D28). Level **record** is new: it writes only GitMaster's own store, needs no approval marker and records the actor. `UpdateThread { tags? }` loses its `tags` field when `thread_tag` is replaced; that change is part of P3.

### Evidence and bounds (D144)

| Evidence | Rule |
|---|---|
| `blob_match` | The edit's `post_oid` equals the file's blob in the blamed commit. |
| `line_match` | A contiguous run of at least N lines, where N is 3 (Q34), and blank and brace-only lines do not count, appears contiguously in the added lines of one edit. The path is the one blame reports for the line. The edit's `at` is not after the blamed commit's author date. All matching edits are returned and the result is marked ambiguous. A line match never creates a stored link. |
| `hunk_overlap` | Named in D144 only. The rule is defined in P1. |
| `observed_commit` | Shown as "created during this thread" (D126). |
| `manual` | Shown as "linked by you". |

- An edit that was redacted (D121), truncated by the cap or recorded `from_input` reports `unverifiable`, never "no match".
- No whitespace normalization. A formatter run after the edit breaks the match.
- Bounds: at most 10 commits per query, 5 threads per commit, two edges out. A cut result says so.
- `post_oid` (D123) is computed as the raw hash; Q34 answers the `--path` question (see below).

### Commit links after a rewrite (D145)

- GitMaster's own restack remaps links from the old-to-new pairs of D142. The stored id is never overwritten; the remap goes to `resolved`.
- A rewrite outside GitMaster: the engine looks only in the rewritten range (old tip from `ORIG_HEAD` or the reflog, new tip from the branch) for an exact unique `git patch-id --stable` match. A duplicate is `ambiguous`. No match is `gone`, meaning unreachable from any ref, not a missing object.
- Not used: similarity matching, `range-diff`, a `post-rewrite` hook, `notes.rewriteRef` (D75, D98).

### P1: provenance engine

Pure ranking and bounds, and the blame and patch-id adapters in `gm-core`. Real-git tests in `crates/gm-core/tests/provenance_real_git.rs`: a blob match, a line match, an ambiguous line match, a formatter run after the edit, a rebase, a squash and a reorder, a redacted and a truncated edit. Measure N on real threads and record it (closes Q34). Done when each fixture returns its evidence and the release build answers a range within the budget set in P1.

**Built (engine only, 2026-10-31; Q34 answered in the same slice).** `ReadProvenance` and `ThreadProvenance` in `gm-schema`; the rules in `gm-core/src/threads/provenance.rs` (pure); `git blame --line-porcelain` parsed in `gm-core/src/blame.rs`; the adapters `blame_lines` and `blob_at_commit` on `GitPort`; the queries `path_edits`, `commit_links`, `thread_summary` and `entries_around` in the store. Reads are excluded from the operation log, as the other thread reads are.

Decisions taken in P1 (the docs do not cover them; they are the engine's rules until the author decides otherwise):

- **Constants.** `LINE_MATCH_MIN` = 3 (N, measured under Q34; see `decisions.md`), 10 commits per query, 5 threads per commit, 200 edits compared per commit (`candidates` cut beyond), a range of at most 1000 lines (longer is refused, not cut), 2 entries of context each side of an edit.
- **Trivial lines** are blank, or made only of `{}()[];,` and whitespace. They never count toward a run. Comparison is exact: no whitespace normalization.
- **A run** is the consecutive lines blame gives one commit under one path, with its non-trivial lines. It matches an edit when those lines appear in order and contiguously among the non-trivial added lines of one **segment** of the edit's patch. A segment is a maximal run of `+` lines; a context line or a hunk header ends it, a removed line does not (it is not in the new file).
- **The date rule** is `edit.at <= commit author date` (milliseconds). It applies to line and hunk evidence. `blob_match` does not use it, because D144 states the blob rule without a date.
- **Paths.** An edit matches a blame path when its path equals it, or equals `thread.worktree` joined to it (an absolute path the edit recorded under the thread's worktree). Until `edit.rel_path` exists (P3), this is the join in SQL. The scan uses the existing `edit_path` index only partly, because of the `OR`; P3's `rel_path` index replaces it.
- **`hunk_overlap` (defined here).** The edit added a non-trivial line that blame gives the same commit in the queried range. Blame already proves the commit owns that line, so the `-U0` hunk is not read. It is weak: it never makes a result ambiguous and never creates a link. It is reported only for an edit that is verifiable and dated before the commit.
- **Ambiguous.** When more than one edit of one commit has `blob_match` or `line_match`, each thread holding such an edit is marked ambiguous. Nothing is picked.
- **Unverifiable.** An edit from the tool input (`from_input`), a patch that contains `[REDACTED:` (D121) or `[... truncated ` (the size cap) is unverifiable, when it is dated before the commit and has no blob match. A blob match is verified by the raw content, so the reason is then not shown.
- **Links.** A thread linked to the commit by `thread_link` shows `observed_commit` (origin `observed`) or `manual` (origin `manual`). A commit link needs the full id, which is what `rev-list` stores.
- **Order of threads.** Strongest evidence first, then the most recent `last_event_at`, then `uid`. Threads without evidence (unverifiable edits only) come last.
- **Edges.** P1 walks line to commit to thread to entries, which is two steps from the line. The bound on two edges out applies once labels and `thread_rel` exist (P3); nothing beyond the thread is walked yet.
- **`post_oid` (Q34, decided: raw).** It is libgit2's hash of the raw bytes (`git2::Oid::hash_object`), which is what `git hash-object` gives without `--path`: no CRLF or LFS normalization. Under `core.autocrlf` or a filter, the blob at the commit and the post image can differ, so a blob match fails rather than matching falsely. The measurement found no edited file with CRLF, LFS or an attribute, so `--path` was not adopted. Measure again when a repository with those produces edits.
- **Blob at commit** is read with `git ls-tree` (it names a missing path cleanly), not `rev-parse <commit>:<path>`.
- **Blame** is `git blame --line-porcelain -M -C -L from,to -- path` run in the repository root. Uncommitted lines (all-zero ids) belong to no commit and are left out.
- **Cancellable (built after the first note).** `ReadProvenance` starts its blame as a job on a thread of its own, as `SearchText` does: a newer provenance read, or leaving the repository, stops the blame's git process and cancels the read; its late answer is dropped by `OperationId`. A request the rules refuse is refused before anything is cancelled. Reads are not in the command log. Still on the engine thread, and not cancellable: the blob lookup per commit (`ls-tree`, at most 10) and the store reads. They are inside the measured times below.
- **Measured (Q34, closed).** N = 3, the rule's answer and its numbers are in `decisions.md` (Q34). Release, one repository of 447 commits: a 20-line window p95 36 ms, a whole file of up to 1000 lines p95 111 ms. Proposed budget, to confirm with the author: a 20-line window p95 at or under 100 ms, a whole file of up to 1000 lines p95 at or under 250 ms, both in release.

### P2: MCP provenance tool

`provenance` in `gm-mcp` over `ReadProvenance`, untrusted-marked, bounded, with the cut stated. Depends on slice 7 for the server. Done when an agent asks about a range and gets the thread, its entries and the evidence, and a cut result says so.

### P3: labels and links

Schema v5 with its migration test; `ListLabels`, `ReadLabel`, `CreateLabel`, `AttachLabel`, `LinkThreads`, and the human-only commands; the rewrite resolution above; MCP `label_create`, `label_attach` and `thread_link`; the allow rules the docs recommend for those three (the plugin cannot ship permission rules). Done when the migration keeps every tag as a label, a commit link survives a restack and resolves after an outside rewrite only on an exact unique match, and a row an agent wrote shows the agent and can be removed.

**Built (part A: store and commands, 2026-10-31).** Schema v5 and its migration (`threads/store.rs`, `SCHEMA_V5`, tested from a v4 fixture); the label and relation store in `threads/labels.rs`; `ListLabels`, `ReadLabel`, `CreateLabel`, `AttachLabel`, `LinkThreads`, `RenameLabel`, `DeleteLabel`, `DetachLabel` and `RemoveThreadRelation` in `gm-schema` and `threads_session.rs`; the commit snapshot on `GitPort::commit_snapshot`; `UpdateThread` without `tags`; `threads_search`'s `label` filter over `label_attach`; the UI's tag edit as label commands. Tests: store unit tests in `threads/store.rs` and `threads/labels.rs`, and `crates/gm-core/tests/labels_real_git.rs`. Not built here: rewrite resolution (`resolved` stays empty), the `--follow` read of file labels, the MCP tools `label_create`, `label_attach` and `thread_link`, and the Labels page (P4).

**Built (part B, 2026-10-31).** Rewrite resolution of commit links and attachments (`threads/resolve.rs`; `ThreadLink.resolved` and `state`: current, rewritten, ambiguous, gone; the stored id is never overwritten; GitMaster's own restack goes through the recorded old-to-new pairs, an outside rewrite only on an exact unique patch match), tested against real repositories in `crates/gm-core/tests/commit_links_real_git.rs`; the MCP tools `labels_list`, `label_get`, `label_create`, `label_attach` and `thread_link`; the `label` (name or list), `related_to` and `related_kind` arguments of `threads_search`; provenance labels and linked threads with the `labels` and `links` cuts. Tests in `crates/gm-mcp/tests/stdio_real_git.rs`. Not done: the recommended allow rules are docs only; the Labels page and every UI surface (P4); nothing seen in a window.

Decisions taken in part A (the docs do not cover them; they are the rules until the author decides otherwise):

- **Names, not ids, in attach.** `AttachLabel` and `DetachLabel` name the label. An attach to a name that does not exist creates the label with the actor's origin. `CreateLabel` of an existing name returns that row unchanged, and an attach of an existing pair returns the attachment already there (its origin and time stay). `RenameLabel` and `DeleteLabel` take the id, as the table says.
- **Level `record`** has no code enum: the engine logs every command except the reads, and the exclusion list in `Engine::record` is the reads. The record commands are not human-only (`human_only` lists the apply ones). Their rows carry `human` or `agent:<name>` from the command's actor.
- **Limits.** A label name is 1 to 64 characters, no control characters, unique and case-sensitive (as tags were). A description is at most 500 characters. A colour is `#rrggbb`, stored in lower case. A file reference is a repository-relative path. A label attaches to another label by its id, not to itself.
- **Commit snapshots.** `AttachLabel` (kind `commit`) and `LinkThread` (kind `commit`) take the snapshot at write time: full id, subject, author name and committer date in milliseconds, through `git log -1`. The migration cannot ask Git, so it leaves existing commit links without a snapshot. The engine fills them when a repository's store is first opened in the process (`threads_session::fill_snapshots`): one `git cat-file --batch` per 500 names (`GitPort::commit_snapshots`), so a store open costs a few processes at most. Commits Git no longer knows are retried on each open, which costs one process per 500 of them, and keep no snapshot; the link shows its id. Observed links from capture get theirs the same way, not in the capture hook, which has a time budget.
- **Relations.** `LinkThreads` between two threads that exist, never a thread and itself. A link already there is returned as it is. `RemoveThreadRelation` answers the row it removed.
- **Deleting.** `DeleteLabel` also removes the attachments other labels made to it. `DeleteThread` removes the thread's label attachments, which have no foreign key, and its relations (cascade).
- **Origin of thread links.** `LinkOrigin::Manual` is renamed `Human` (wire `human`), and the migration rewrites the stored `manual`. `LinkThread` stays human-only, so no `thread_link` row has an `agent:` origin yet; `commit_links` skips a row whose origin it does not know.
- **Events.** `LabelsListed`, `LabelRead`, `LabelsChanged { label }` (the label after a create, rename or attach; `None` after a delete or detach) and `ThreadRelationChanged { removed }`. Every label write also sends `ThreadsChanged`, since the thread rows show their labels.
- **UI.** A tag edit sends `AttachLabel` for each new tag (which creates a missing label) and `DetachLabel` for each tag no longer named, all as the human; the detaches go first, and the last answer settles the write. A tag over 64 characters or with a control character is refused before anything is sent. When the tags did not change, nothing is sent and the dialog says so and stays open.
- **Deferred hook reader.** On a database older than v5, the hook's read-only handle reads the cursor and the heads only. A filtered search on that handle fails, because the tag filter names the label tables; the hook never searches.
- **Index.** `label_attach` has its primary key only, and `(kind, ref)` is not indexed, to keep the schema as written. Thread tags are read by a scan of `label_attach`; add the index if it shows in the measurements.

### P4: UI

**Built (2026-11-01).** The Labels page and its dialogs (`labels_page.rs`, `label_curate.rs`, `shell_labels.rs`), label chips and a label filter on the Threads page, **Link to Thread** and relations in `ThreadDetail` with direction and origin (`relations`, Remove, the agent's name), and the provenance panel on the Code page (`provenance_view.rs`, `shell_provenance.rs`; `ProvenanceEdit.entry_id` opens a thread at the edit). Checked in a window: the Labels page, the New Label dialog and the provenance panel, found and fixed one defect there (the panel dropped its answer because the engine sends `Done` first). Not done: attaching a file, commit or label from the app, the gone and ambiguous states in the thread's Commits panel, an agent's name on a label chip, a window check of the rest, the narrowest width, and the viewer menu's overflowing reason text.

The **Labels** page, chips on rows and headers, label dialogs, the provenance view for a range on the Code page, **Link to Thread** in the header's **⋯** menu, and the actor shown on each agent row. Done when a range can be asked about and its evidence read in a window, the layout is checked at its smallest width, and the screens are recorded in `design/screens.md`.

## To confirm with the author

Marked **proposed** in `design/decisions.md`; the slice named in brackets waits for the answer:

1. Fail closed when a user pattern is invalid: capture pauses everywhere (D121) [T1].
2. Capture is on for every repository once the integration is installed, with a per-repository pause kept in `gitmaster.db` (D127) [T3a].
3. Tool results are stored, capped at 64 KiB each (D122) [T2].
4. Thinking blocks are not stored (Q31) [T3a].
5. A separate small hook binary if the full binary's cold start misses the budget (Q25) [T3a].
6. `gitmaster thread hook` and `gitmaster harness …` as one-shot subcommands are consistent with D02 (D124) [T3a].

## Sharing (D187)

**Deferred 2026-11-07; not in the build order.** The principles are decided (D187); the transport is reopened by the author, who wants a session of its own to explore SQLite-based alternatives to Git (Q43). Everything below that names a ref, a tree or a Git command is therefore **one candidate transport**. The parts that hold for any transport are the format of the shared thread (`thread.json`, `entries.jsonl`, `edits.jsonl`, `links.json`, `content_hash`), the redaction check, the import rules (`source = imported`, ingest redaction, hash verification, untrusted text) and the threat model. There is no user documentation for sharing until the transport is chosen. Open questions: Q37 (automation), Q38 (size and cost), Q43 (transport). The point of the design is that **with sharing off nothing changes**: no ref, hook, file or network call, and no code path outside the sharing module runs.

### Settings

Git configuration keys (D173): `gitmaster.sharing.enabled` (default `false`), `gitmaster.sharing.remote` (no default) and `gitmaster.sharing.memories` (default `false`). They are read through `git config`, so `~/.gitconfig` and `.git/config` follow Git's scope rules, and never from a file in the working tree (D73). Every sharing command checks `enabled` and `remote` first and refuses with the reason, before it reads the database.

### Layout in Git

```text
refs/gitmaster/threads/<author>/<uid>        local and remote: what the user published
refs/gitmaster/shared/<remote>/<author>/<uid>  local only: what a fetch brought in
```

- `<uid>` is the thread's `uid` (16 hex, D122). `<author>` is the first 12 hex of the SHA-256 of the lowercased `user.email`, so two people never write one ref, and a thread resumed on two machines by the same person overwrites itself instead of forking. Both are validated as lowercase hex of the exact length when read from a remote; any other ref name is ignored.
- A ref points at one **orphan commit** (no parent, no history). A publish makes a new orphan commit and moves the ref with a lease on its old value, so a thread is one object, not a chain, and deleting the ref is the whole of unpublishing. The commit is made with `git commit-tree`, and signed with `-S` when `commit.gpgsign` is true or `user.signingkey` is set, so the user's own signing setup applies. The message is `GitMaster thread <uid>`; the author and committer are the user's.
- The tree has these five blobs and nothing else. A fetched tree with another entry name, a subtree, a symlink or an executable mode is refused.

| Blob | Content |
|---|---|
| `thread.json` | `format` (1), `gitmaster` (version), `uid`, `harness`, `harness_version`, `model`, `branch`, `head_start`, `head_end`, `title`, `started_at`, `last_event_at`, `ended_at`, `entry_count`, `edit_count`, `redactions` (counts per pattern, never values), `author` (`name`, `email`), `truncated_entries` |
| `entries.jsonl` | One JSON object per entry, in `seq` order: `seq`, `turn`, `kind`, `source_id`, `parent_id`, `tool`, `text`, `args_json`, `is_error`, `truncated`, `orig_bytes`, `at` |
| `edits.jsonl` | One object per edit: `entry_seq`, `path`, `old_path`, `change`, `patch`, `added`, `removed`, `pre_oid`, `post_oid`, `from_input`, `at` |
| `links.json` | `commits`: `{ id, subject, origin }` with origin `observed` or `human`; `branch`; `files` (the rest is derived on read, D143) |
| `content_hash` | `sha256:<hex>` of the four blobs above, concatenated in that order with a `0x00` byte between them. Each blob is produced by a serializer with a fixed field order and a trailing newline, so the same thread always gives the same bytes. |

Not exported: `note`, `starred`, labels, `thread_rel`, the spool, the transcript path, the worktree path and the cursor.

### Engine commands (D08 table style)

| Command | Level | Actor | Notes |
|---|---|---|---|
| `ReadSharingStatus` | read | any | Whether sharing is on, the remote, and per thread: not published, published and current, published and changed since. |
| `PublishThreads { threads \| all }` | apply | human | Preflight, build, push. Writes refs and the `thread_share` row. Logged (D28). **No MCP tool and no agent actor**: the apply tools of D137 do not include it. |
| `FetchSharedThreads` | apply | human | Fetches into `refs/gitmaster/shared/…`, verifies and ingests. Logged. |
| `UnpublishThreads { threads }` | apply | human | Deletes the ref locally and with a push of the empty ref. Logged. |

The threads are listed by the engine; the UI and `gitmaster thread publish|fetch|unpublish` are two front ends of the same commands (D08).

### Publish

1. **Preflight** (pure, before any write): sharing is on; the remote exists; the user's `user.email` is set; every selected thread is closed or the user confirmed an open one.
2. **Redaction check** (D187 (5)): serialize the blobs, run the **current** redaction patterns and sensitive paths over them with the same engine used at ingest, in check mode that returns matches and rewrites nothing. A match stops that thread, names it and the pattern, and publishes the others. This is the only place sharing touches redaction. It is a check because D121 forbids a second redaction at export; the stored text is never altered by publishing.
3. **Size check** (Q38): a thread over the limit (starting value 20 MiB of blobs) is refused with its size. Measure first.
4. **Build**: `git hash-object -w` for the blobs, `git mktree`, `git commit-tree` (signed when configured). Not touching the index or the working tree.
5. **Push**: `git push --porcelain <remote> <ref>:<ref>` for the batch, each with `--force-with-lease=<ref>:<old>` where `<old>` is the commit recorded in `thread_share` (or the empty value for a first publish). A rejected lease means someone else wrote that ref (it should not happen, the author is in the name); the thread is reported, not forced.
6. **Record**: `thread_share(thread_id, remote, ref, commit_id, content_hash, published_at)`. A thread is **changed since** when its current hash differs.

Everything runs as `git` subprocesses with a timeout and cancellation (the reads-and-cancel rule of `AGENTS.md`); a cancelled publish leaves the local refs and writes no `thread_share` row for the threads not pushed.

### Fetch and import

1. `git fetch <remote> '+refs/gitmaster/threads/*:refs/gitmaster/shared/<remote>/*'`. Only this refspec; never `--tags`, never a branch.
2. For each fetched ref with a valid name: read the five blobs with a size limit, parse with a streaming parser with depth and length limits, check `format`, recompute `content_hash`.
3. **Ingest** through the capture pipeline's redact → cap → upsert (not a copy): the local patterns apply again, caps apply, and the counts are added to `thread_redaction`. A thread whose `uid` already exists locally with `source = 'local'` is skipped. An imported thread whose hash does not verify is stored with `modified = 1` and shown as **Modified**.
4. Imported threads have `source = 'imported'`, `author_name`, `author_email` and `shared_ref`. They are not captured into, not exportable by Publish (an imported thread is never republished: the author published it), and `UpdateThread` may change only `note`, `starred` and labels on them; entries cannot be deleted one by one, only the thread.

Schema change (one migration, tested from the previous version): `thread` gains `source TEXT NOT NULL DEFAULT 'local'`, `author_name`, `author_email`, `shared_ref`, `modified INTEGER NOT NULL DEFAULT 0`; new table `thread_share`. `ListThreads` and `SearchThreads` gain an `author` and a `source` filter; `thread_get` returns `source`, `author` and `modified`; `threads_search` and `thread_read` mark imported text `untrusted: true` (it always is) and `origin: "imported:<author>"`.

### Threat model

| Threat | Mitigation |
|---|---|
| A thread leaves the machine with a secret that a pattern added later would have caught | Redaction check at publish (fail closed), confirmation naming the remote |
| The remote is public by accident | No default remote; the confirmation says who can read it; the docs recommend a separate repository |
| A teammate's thread carries instructions aimed at the reader's agent | Imported text is untrusted data in the UI and over MCP (D144 rules); ingest redaction applies again; the apply tools of D137 still ask |
| A hostile remote sends oversize, deeply nested or odd-shaped data | Name, mode, size and parse limits on fetch; refs outside the exact name shape ignored |
| A hostile ref name or tree path escapes the layout | Names are validated as hex of an exact length; the tree has five fixed names |
| Tampering in transit or on the host | `content_hash` and, when used, the commit signature (the signature proves who, the hash proves what); not a defense against a host that rewrites both |
| "Delete" is believed to erase | The docs and the unpublish confirmation say it is not erasure |
| Sharing changes what `git push` does | It does not: no hook, no push of anything but the thread refs, only on the user's command |

### Tests

Pure: serializer determinism (same thread, same bytes, same hash), hash verification, ref-name validation, redaction check (a pattern added after capture is caught; the stored text is unchanged), schema limits. Real Git: a temporary repository and a temporary **bare remote** (`crates/gm-core/tests/threads_sharing_real_git.rs`): publish, re-publish (still one object), fetch into a second repository, a tampered blob marked **Modified**, a refused extra tree entry, unpublish, a signed commit when a test key is configured, and a run with sharing off that proves no ref and no network call is made. A property test that a fetched ref name outside the shape is never read.

### Sub-slices

- **SH1, format and checks (pure):** serializer, hash, ref names, redaction check, the `gitmaster.sharing.*` keys.
- **SH2, publish and unpublish:** the commands, `thread_share`, the CLI.
- **SH3, fetch and import:** ingest of imported threads, schema change, filters, `untrusted` marking.
- **SH4, UI:** the Sharing settings, the confirmation dialog, **Publish Threads…**, **Fetch Shared Threads**, author filter, **Modified** badge.
- **SH5, memories on the same channel:** with M5 below. Deferred with the rest of sharing.

## Memories (D190)

Decided 2026-11-07 (D190), not built. User-facing behavior: `docs/guide/memories.md` (marked `Planned`). Wider scope: contexts, D191 (details in Q44). The agent-side work (the skills) is in `design/agent-skills.md`, "Skills and sub-agents".

### Schema

One migration, the next free schema version of `threads.db` (v7: v6 is the workspace migration of D180). Sharing no longer comes first, so nothing precedes it.

```text
memory(
  id          INTEGER PRIMARY KEY,
  uid         TEXT UNIQUE NOT NULL,       -- first 16 hex of sha256(random 16 bytes)
  kind        TEXT NOT NULL CHECK (kind IN ('decision','rule','lesson','fact')),
  text        TEXT NOT NULL,              -- redacted at ingest, at most 1000 characters
  status      TEXT NOT NULL CHECK (status IN ('proposed','accepted','rejected','superseded')),
  origin      TEXT NOT NULL,              -- 'human' | 'agent:<client>' | 'imported:<author>'
  proposed_by TEXT,                       -- origin at creation; 'origin' becomes 'human' when the human edits the text
  model       TEXT,                       -- when the harness reports it
  created_at  INTEGER NOT NULL,
  decided_at  INTEGER,                    -- accepted, rejected or superseded
  decided_by  TEXT)                       -- 'human'
memory_source(
  memory_id INTEGER NOT NULL REFERENCES memory(id) ON DELETE CASCADE,
  kind      TEXT NOT NULL CHECK (kind IN ('thread','commit','file')),
  ref       TEXT NOT NULL,                -- thread uid | commit id | repo-relative path
  repo_id   TEXT,                         -- NULL: the memory's own repository; else the repository (D191)
  entry_from INTEGER, entry_to INTEGER,   -- kind 'thread': seq range
  blob_oid  TEXT,                         -- kind 'file': the blob at acceptance, for 'files changed since'
  PRIMARY KEY(memory_id, kind, ref))
memory_rel(
  from_memory INTEGER NOT NULL REFERENCES memory(id) ON DELETE CASCADE,
  to_memory   INTEGER NOT NULL REFERENCES memory(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind = 'supersedes'),
  origin TEXT NOT NULL, at INTEGER NOT NULL,
  PRIMARY KEY(from_memory, to_memory, kind))
memory_fts  -- FTS5 over memory(text), built the way entry_fts is (D128)
```

Rules the engine enforces, not the callers:

- Text is redacted by the capture pipeline's redactor (D121) and capped at 1,000 characters before it is stored. An exact duplicate (same kind, text normalized for case and whitespace) of a non-rejected memory returns that memory and creates nothing.
- An agent actor can create only `status = 'proposed'`, and only with at least one `memory_source`. It can never update, accept, reject, supersede or delete. A human actor does all of those.
- State machine: `proposed → accepted | rejected`; `accepted → superseded` (only by accepting a memory that supersedes it); a human edit of a `proposed` memory sets `origin = 'human'` and keeps `proposed_by`. An accepted memory is edited only by a human, and the edit is logged (D28).
- When a memory first becomes `accepted`, each `file` source records `blob_oid` (the blob at `HEAD`, or none when the file is not there). Accepting an accepted memory again is idempotent: it records nothing and keeps `decided_at` (D192). A blob that could not be read then (Git did not answer, or the source's repository has no known folder) is stored as not read (`blob_read = 0`), and the file reads as **unavailable**, never as changed (D193 (9)). **Files changed since** is derived on read: the source's `blob_oid` differs from the file's current blob at `HEAD`. Never stored, never guessed (D126).

### Commands (D08 table style)

| Command | Level | Actor | Notes |
|---|---|---|---|
| `ListMemories { query, kind, status, path, author, limit, cursor }` | read | any | Agents get `accepted` by default; other statuses are returned marked `reviewed: false`. |
| `ReadMemory { id }` | read | any | The memory, its sources and relations, and the derived `files_changed_since`. |
| `ProposeMemory { kind, text, sources }` | record | agent or human | An agent's call creates `proposed`; a human's creates `accepted`. |
| `AcceptMemory { id, text?, supersedes? }` | apply | human | Optionally edits the text, and supersedes one or more memories in the same transaction. |
| `RejectMemory { id }` | apply | human | |
| `DeleteMemory { id }` | apply | human | |

This table set is the repository's own store; memories attached to contexts live in `contexts.db` and a search reads both (D191, "Contexts" below). Search reuses the query builder of D128. MCP tools: `memories_search`, `memory_get` (read) and `memory_propose` (record); see `docs/reference/mcp-tools.md`. Memories are untrusted text in results, like every text from a thread.

### UI

A **Memories** tab on the Threads page (a list on the left, the selected memory on the right, as for threads): a count of proposals on the tab, filters, **Accept**, **Edit**, **Reject**, **Supersedes…**, **New Memory**, bulk accept of selected proposals or of an author's. A memory's sources open the thread at the entry range, the commit or the file in the Code page. **Agent-written** shows on every memory with an agent origin.

### Contexts (D191)

> **Superseded in part by D222 (2026-11-10).** Every memory belongs to a context and lives in `contexts.db`; a context is an area of knowledge that may have no repository; repositories are *bound* to contexts; a sub-context inherits its ancestors' memories; any context can be read by name. The two stores, the `repository` scope and Move to Context from a repository below describe the model being replaced in slice 6k; `visible_contexts` keeps its definition and now gives the default reach of a repository.

Decided by the author on 2026-11-07: D191 (1) to (5), including the storage and the other details below, which the agent drafted and the author confirmed (D191 (4), Q44 resolved). User-facing behavior: `docs/guide/contexts.md` (`Planned`). The author rejected procedures (a structure of linked memories): memories stay flat.

A context is a named set of repositories at the user level. A memory attached to a context is stored once and found from every repository that sees the context. Two stores therefore hold memories: a repository's own `threads.db`, and `contexts.db`, and a search reads both.

#### `contexts.db`

In the app data directory (`~/.local/share/gitmaster/contexts.db`, or under `$XDG_DATA_HOME`; the same on macOS), mode `0600`, WAL, `secure_delete`, its own schema version (the app and every `gitmaster mcp` process open it).

```text
context(
  id INTEGER PRIMARY KEY, uid TEXT UNIQUE NOT NULL,       -- 16 hex, random
  name TEXT NOT NULL, parent_id INTEGER REFERENCES context(id) ON DELETE RESTRICT,
  created_at INTEGER NOT NULL,
  UNIQUE(parent_id, name))                                -- a name is unique among siblings
context_repo(
  context_id INTEGER NOT NULL REFERENCES context(id) ON DELETE CASCADE,
  repo_id TEXT NOT NULL,                                  -- the content of .git/gitmaster/repo-id
  path TEXT NOT NULL,                                     -- last known common directory, to find the clone and to show it
  name TEXT NOT NULL,                                     -- display name, the folder name when joined
  added_at INTEGER NOT NULL,
  PRIMARY KEY(context_id, repo_id))
memory, memory_source, memory_rel, memory_fts             -- the same shapes as in threads.db (above)
memory_context(
  memory_id INTEGER NOT NULL REFERENCES memory(id) ON DELETE CASCADE,
  context_id INTEGER NOT NULL REFERENCES context(id) ON DELETE CASCADE,
  PRIMARY KEY(memory_id, context_id))
```

`memory_source` gains `repo_id TEXT NULL` in both stores: null means the memory's own repository (in `contexts.db` a source always names its `repo_id`). A source whose `repo_id` has no known path on this machine reads as unavailable; nothing resolves it by guessing.

#### Repository identity

`.git/gitmaster/repo-id` holds 32 hex characters, written by GitMaster (mode `0600`, atomic) the first time the repository joins a context. It is inside `.git`, so the working tree stays untouched. A `.git` directory copied to another folder carries the same id; the app compares the id's recorded `path` with the real one when it opens a repository and, on a clash, asks which clone keeps the id and gives the other a new one (Q44 (b)).

#### Visibility (pure)

```text
visible_contexts(repo_id) = ancestors_inclusive( { c : (c, repo_id) in context_repo } )
```

`ancestors_inclusive` follows `parent_id` to the root. It has a cycle guard, and `UpdateContext { parent }` refuses a parent that would make a cycle. The function is pure over the rows and is tested without a database file. The scopes a search reads are the repository store plus the memories in `memory_context` for those context ids.

#### Commands (D08 table style)

| Command | Level | Actor | Notes |
|---|---|---|---|
| `ListContexts { repo? }` | read | any | Contexts, their nesting, repositories and counts; with `repo`, those the repository belongs to (this is MCP `contexts_list`). |
| `CreateContext { name, parent? }` | apply | human | |
| `UpdateContext { id, name?, parent? }` | apply | human | Refuses a cycle and a duplicate sibling name. |
| `PreviewDeleteContext { id }` | read | any | What `DeleteContext` would do, without writing (D196): the sub-contexts and the memories attached to no other context, at most 50 with the full `total`. Answers `ContextDeletePreview`. |
| `DeleteContext { id }` | apply | human | Refused while it has sub-contexts. Memories attached to no other context are listed with their count and deleted on confirmation. |
| `AddRepoToContext { context, repo }` / `RemoveRepoFromContext` | apply | human | Writes `repo-id` on first join. Removing leaves the memories whose sources are in the repository. |
| `MoveMemoryToContext { memory, context }` | apply | human | Moves a repository memory into `contexts.db` (insert, then delete from `threads.db`, in that order, so a crash duplicates and never loses) and keeps its sources. |
| `AttachMemory { memory, context }` / `DetachMemory` | apply | human | A context memory in more than one context; it stays one row. Detaching the last context is refused (move or delete the memory instead). |

`ProposeMemory` (above) takes an optional `context`: it must be in `visible_contexts` of the calling repository's `repo_id` (a context the repository belongs to or an ancestor of one), and the memory is created `proposed`. A call naming another context is refused.

Search takes a `scope` (`all`, `repository` or a context name) and queries the repository store and the allowed context rows, merges by relevance and uses the nearer scope as a tie-break. Each result carries `scope` as a context path (`Platform / Billing`) or `repository`. `contexts.db` missing, locked for too long or unreadable never fails a search: the repository scope answers, and the result says the contexts were not read.

#### Threat model

| Threat | Mitigation |
|---|---|
| A transcript in one repository plants a rule that steers agents in every repository of a context | An agent can only propose; only a human accepts; the default search returns accepted memories only (D190 (2)) |
| One agent writes into a context its repository does not belong to | `ProposeMemory` checks the context against the repository's own `repo_id` |
| Two clones share an id after a copy | Path comparison on open; the user chooses (Q44 (b)) |
| A memory outlives the repository it was about | The source reads as unavailable; the memory stays until the human removes it |
| Concurrent writers (the app and several MCP processes) | WAL, a busy timeout, short transactions, and idempotent proposals by `derived_from` |

#### Tests

Pure: `visible_contexts` over trees and repositories in several contexts, cycle refusal, the scope filter, merge and tie-break ordering. Real files: `contexts.db` opened by two processes at once, a repository joining two contexts and seeing both, a sub-context memory not visible from its parent's other repositories, `MoveMemoryToContext` interrupted between its two writes, a copied `.git` with a clashing id, and a search with `contexts.db` removed.

#### Sub-slices

- **C1, contexts store and visibility (pure plus files):** `contexts.db`, `repo-id`, the commands, `visible_contexts`, the merged search. Comes after M1 and before M2.
- **C2, Manage Contexts… dialog and the scope chip and filter** (with M3).

### Sharing (deferred with D187)

Not in the build order; it waits for the transport of Q43. The text below is the Git-ref candidate and shows the rules that must hold on any transport: only accepted, non-imported memories leave; fetched memories arrive as `proposed`. Accepted memories would be published at `refs/gitmaster/memories/<author>/<uid>` under the rules of D187: one orphan commit per memory, a tree of `memory.json` (kind, text, sources with entry ranges and file paths, `supersedes` as uids, author) and `content_hash`; the same redaction check; the same fetch limits. A fetched memory is ingested as `proposed` with `origin = 'imported:<author>'` (the importing human approves what enters their own agents' context, D190 (8)), and a memory whose `uid` exists is skipped. Only memories with `status = 'accepted'` and `origin` not imported are published.

### Tests

Pure: the state machine (every illegal transition is refused, an agent actor cannot do anything but propose), redaction and cap, duplicate detection, derivation of **files changed since** against a temporary repository, FTS search. Real Git: file blob changes after acceptance, a file renamed after acceptance (shown as changed, no guess), and the sharing round trip with a bare remote.

### Sub-slices

- **M1, model and store (pure):** schema, state machine, commands, search; no UI.
- **M2, MCP tools:** `memories_search`, `memory_get`, `memory_propose`, `contexts_list`; with slice 7's tool plumbing. After C1, because the search arguments carry the scope.
- **M3, Memories tab** (with C2: the scope chip and filter).
- **M4, skills:** `/gitmaster:propose-memories` and `/gitmaster:prepare` (design/agent-skills.md).
- **M5, sharing of memories** (with SH5): deferred with sharing, not in the build order.

## Sub-agent threads (D199)

Decided by the author on 2026-11-07, not built. User-facing behavior: `docs/guide/threads.md` ("Sub-agents", marked `Planned`) and the `parent` fields in `docs/reference/mcp-tools.md`, marked `Planned`. The decision is in `design/decisions.md` (D199); the unverified harness facts are Q45.

### Why a thread of its own

The T3b draft kept a sub-agent inside its parent: its entries carried a `parent_id`, and its `SubagentStop` closed the parent's turn. Both are wrong. A sub-agent has its own turns, edits and commits, and its stop ends only its own work. As a thread of its own it gets every existing rule without a special case: redaction, caps, search, the derived edit states and commit links. The parent keeps the `Agent` call and its result.

### Identity and link

- The sub-agent's `session_id` is `<session_id>/agent-<agentId>`. Its `uid` is derived as for every thread, so `UNIQUE(harness, session_id)` holds.
- `parent_thread` is the parent's `thread.id`. `parent_call` is the `tool_use` id of the parent's `Agent` call, which is the `source_id` of the parent's `tool_call` entry.
- The `agentId` comes from `toolUseResult.agentId` on the `Agent` tool's result. The sub-agent's `agent-<agentId>.meta.json` must name the same `toolUseId`; a mismatch writes a `capture_gap` on the sub-agent and no link.
- The sub-agent's transcript is `<transcript dir>/<session>/subagents/agent-<agentId>.jsonl`. Its title follows the same rule as any thread (its first prompt).
- The type and description shown in the parent row come from the `Agent` call's input (`subagent_type`, `description`, both in the parent entry's `args_json`) and are checked against the meta file. Which of them the harness provides is Q45 (e).

### Capture

- **Triggers.** The hook `SubagentStop`, and the `PostToolUse` of an `Agent` call (its result is read). The hook payload only triggers the read. No field of it is a source until Q45 verifies it.
- **One read.** Resolve the identity, create the thread if it is missing (`SessionStarted` with `parent`), then read the transcript from the thread's own `cursor` through the usual pipeline: normalize, redact, cap, one transaction. The read is idempotent by `source_id`, and the cursor moves in the same transaction. Each read is capped at the same size as any other transcript read.
- **Running sub-agents.** The result of the `Agent` call may arrive before the sub-agent has stopped. The read is repeated at `SubagentStop`, so the thread shows what has been recorded so far and then the rest.
- **Nothing crosses.** Sub-agent events never change the parent's turn, its `ended_at` or its cursor. The parent's `TurnEnded` comes from the parent's `Stop` only.
- **Nesting.** A sub-agent's own `Agent` calls create threads whose `parent_thread` is the sub-agent, by the same rule.
- **Edits.** A sub-agent's edits are edits of its own thread. How their pre and post images are taken depends on Q45 (a); until it is answered, an edit whose image cannot be taken falls back to `from_input` (D123).

### Storage (schema v9)

```text
ALTER TABLE thread ADD COLUMN parent_thread INTEGER REFERENCES thread(id) ON DELETE CASCADE;
ALTER TABLE thread ADD COLUMN parent_call TEXT;
CREATE INDEX thread_parent ON thread(parent_thread);
PRAGMA user_version = 9;
```

- Both columns are `NULL` for a thread that is not a sub-agent, and set together for one that is. The store enforces this; the column definitions carry no `CHECK`.
- The migration is a numbered function from v8, tested from a v8 fixture, as every migration is (D122).
- Deleting a parent deletes its sub-agent threads and everything they hold, through the same cascades a thread uses, with `secure_delete`. A test proves no child text remains in the file.
- `thread_rel` is not used for this link. It stays for links a person or an agent curates (D143). The link is not a `thread_link` row either.
- `entry.parent_id` is no longer written. The column stays: dropping it needs a table rebuild, and nothing reads it.

### Visibility and search

- **List.** `ListThreads` returns threads with `parent_thread` `NULL` only. A sub-agent appears only under its parent's `Agent` call.
- **Search.** The sub-agent's entries are indexed under its own thread, as any thread's are. A hit carries `parent`: the parent's `uid` and the `seq` of its `Agent` call. The parent's search does not return the sub-agent's text.
- **MCP.** `threads_search`, `thread_get` and `thread_read` report `parent` for a sub-agent thread. Its text is untrusted, as any thread text.
- **Derived links.** A sub-agent's commits are linked by D126's rule over its own events. Whether **Show Threads for File** and a commit's **THREADS** list show a sub-agent's edit under the sub-agent, under its parent or under both is Q45 (c).

### UI

- The `Agent` call's row in the parent detail is collapsed. It shows the sub-agent's type, its description and its status (running while `ended_at` is `NULL`, otherwise finished). Expanding it reads the sub-agent's entries with the same paging as a thread.
- **Open Thread** shows the sub-agent in the detail pane as a thread of its own, with its own turns, edits and commits. The list gains no row.
- Indentation caps at a fixed depth. Starting value: three levels (proposed; the author may pick another). Deeper runs stay flat at that depth.
- Nested rows count in the virtualized list. Their loading follows the rule for threads: the visible range plus a margin.

### Sharing and memories

Sharing (D187) and memories (D190) are unchanged by this section. The sharing rules name what is not exported; the parent link is not in that list. Whether a shared thread carries its parent link is for the transport decision (Q43).

### Tests

Pure: the identity rules (`parent` is set only with both fields and the `toolUseId` match); a property test that no interleaving of sub-agent events changes the parent's turn, `ended_at` or cursor; the rule that the list excludes children; the depth cap.

Real SQLite: migration from a v8 fixture; a parent delete leaves no child text in the file; two reads of the same sub-agent (at the `Agent` result and at `SubagentStop`) store each entry once.

Real Git: the repository is unchanged outside `.git/gitmaster/`, as for T3.

Conformance: the fake harness of T3a gets a sub-agent transcript; the Claude Code adapter's sub-agent cases use the fixtures that Q45 requires.

### Sub-slices

- **S1, storage and model (pure and SQLite):** schema v9 and its migration; the two columns and the index; cascade delete; `ListThreads` without children; `parent` on `thread_get` and `thread_read`. Can start from hand-made fixtures; nothing is released until S2 is verified.
- **S2, capture:** `SessionStarted.parent`; the identity and meta checks; the child's transcript read with its own cursor; the triggers in the hook. Waits for the Q45 fixtures.
- **S3, Threads page:** the nested row, its expansion, **Open Thread** and the depth cap. Checked in a window in both themes (or the gap is stated).
- **S4, search and MCP:** `parent` on `threads_search` hits and on the thread reads; the `Planned` markers in `docs/guide/threads.md` and `docs/reference/mcp-tools.md` removed as each part ships.

### Open facts (Q45)

The `SubagentStop` and `PostToolUse` payloads, the `Agent` result's `agentId`, and the sub-agent's transcript and meta files are not captured. The fixture in the tree is hand-made. S2 does not start until T0's method has captured them.

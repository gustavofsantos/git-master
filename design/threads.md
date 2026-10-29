# Threads: engineering design

The user-facing behavior is specified in `docs/guide/threads.md` and `docs/agents/agent-integration.md`; this file keeps the engineering rationale and the build plan.

Last updated: 2026-10-29. Status: built except T0, T8 and T9 (see `design/roadmap.md`, slice 6t, for status and gaps; D130 for the choices made while building). The text below is the design as planned; where the build differs, D130 and the roadmap win. The author decided the direction on 2026-10-29 (D119 to D121); the details below are **proposed** (D122 to D128) and each one is listed in "To confirm" before the slice that depends on it starts.

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
- An installer for the agent integration: GitMaster generates a plugin for the agent harness (Claude Code first) that carries the capture hooks, and later the MCP configuration and a skill. The user installs it with a command or a button.

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

`gitmaster thread hook --harness <id>` and `gitmaster harness install <id>` take the harness id. A registry in `gm-core` maps ids to adapters at compile time (`harness::lookup(id)`); `gitmaster harness list` prints the known ids. An unknown id in the hook exits 0 and logs the id (rule 1 of the hook); in `install` it exits 2 and lists the known ids. The id is part of the thread's identity, so two harnesses can never collide on a session id.

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

    /// The plugin or config files to generate, and the steps the user runs to add them.
    fn integration(&self, ctx: &InstallContext) -> Integration;

    /// Where existing sessions for a repository live, for import.
    fn discover(&self, worktree: &Path) -> Vec<SessionLog>;
}
```

`HookInput` carries the session id, the working directory, the log path when the harness gives one, a list of `CanonicalEvent`, and an optional `EditPhase` (`Before` or `After`, the call id and the path). `LogChunk` carries events and the new offset. Neither type has a harness-specific field.

`Capabilities` says what the harness can provide, so the pipeline degrades per harness instead of failing: `session_log` (a transcript to read), `pre_edit_event` (a hook before an edit, so a pre-image is possible), `tool_results` (hooks carry results), `subagents`, `resume_same_id`, `plugin_install` (GitMaster can generate an integration at all). A harness without `pre_edit_event` gets `from_input` edits (D123); one without `session_log` gets no assistant text unless its hooks carry it, and the page says which parts are missing for that harness.

Adapters do no redaction, no database access and no writes. They return events; the core redacts and stores them. This keeps the secrecy guarantee in one place, where a new adapter cannot forget it.

### Canonical events

These are the only input of the pipeline. Fields common to all: `harness`, `session_id`, `at` (ms), and a `source_id` that is the harness's own id for the item (a tool-use id, a log line id) or, when it has none, a hash of the harness id, session id and item position, so replays stay idempotent.

| Event | Fields | Becomes |
|---|---|---|
| `SessionStarted` | `cwd`, `model?`, `harness_version?`, `log_path?`, `origin` (`new`, `resumed`, `cleared`, `compacted`, `unknown`) | the thread row |
| `PromptSubmitted` | `text` | a `user` entry; opens a turn |
| `AssistantMessage` | `text`, `model?` | an `assistant` entry |
| `ToolCalled` | `tool: ToolKind`, `native_tool`, `args` (JSON), `parent_call?` | a `tool_call` entry |
| `ToolReturned` | `call_source_id`, `text`, `is_error` | a `tool_result` entry |
| `TurnEnded` | `parent_call?` (set for a sub-agent) | closes the turn; reads `HEAD` for links |
| `SessionEnded` | `reason` | `ended_at` |
| `Notice` | `text` | a `notice` entry (compaction, interruption) |

`ToolKind` is the canonical tool class: `Read`, `Edit`, `Write`, `Shell`, `Search`, `Web`, `Agent` (a sub-agent), `Other`. The adapter maps the native name to it and keeps the native name in `native_tool` so nothing is lost. The pipeline uses the kind, not the name: `Edit` and `Write` are what trigger pre and post images, and `Shell` is what T8 watches.

For an edit the adapter also provides an **edit intent** inside `ToolCalled.args` normalization: the repository-relative or absolute `path`, the change kind, and the old and new text when the harness gives them. The pipeline decides what to measure from the file (D123); the intent is the fallback.

### Mapping for Claude Code

The first adapter. The left column is the harness's contract and **unverified until T0** (Q26); the right is canonical.

| Claude Code | Canonical |
|---|---|
| hook `SessionStart` (`source`: startup, resume, clear, compact) | `SessionStarted` with `origin` |
| hook `UserPromptSubmit` | `PromptSubmitted` |
| hook `PreToolUse` on `Edit`, `MultiEdit`, `Write`, `NotebookEdit` | `EditPhase::Before` and `ToolCalled` |
| hook `PostToolUse` | `ToolReturned`, `ToolCalled` if not seen, and `EditPhase::After` for edit tools |
| hook `Stop` | `TurnEnded` |
| hook `SubagentStop` | `TurnEnded` with `parent_call` |
| hook `SessionEnd` | `SessionEnded` |
| transcript `assistant` text block | `AssistantMessage` |
| transcript `tool_use`, `tool_result` | `ToolCalled`, `ToolReturned` (reconciliation) |
| transcript thinking block | dropped (Q31) |
| transcript compaction or summary record | `Notice` |
| tool `Read` | `ToolKind::Read` |
| tool `Edit`, `MultiEdit`, `NotebookEdit` | `ToolKind::Edit` |
| tool `Write` | `ToolKind::Write` |
| tool `Bash` | `ToolKind::Shell` |
| tool `Grep`, `Glob` | `ToolKind::Search` |
| tool `WebFetch`, `WebSearch` | `ToolKind::Web` |
| tool `Task` | `ToolKind::Agent` |
| any other tool, or an MCP tool | `ToolKind::Other`, name kept |

An event or record the adapter does not recognize is **not an error**: it is skipped, counted in a `capture_gap` row with the native type name, and visible in `harness status`. A payload that does not parse at all is a gap too. That makes harness version drift visible instead of silent.

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

### The integration (D120)

The adapter's `integration()` generates the plugin or configuration for its harness. For Claude Code it is a plugin directory that holds:

- `.claude-plugin/plugin.json`: name `gitmaster`, version equal to the app's.
- `hooks/hooks.json`: one command hook per event: `SessionStart`, `UserPromptSubmit`, `PreToolUse` (matcher: the file-editing tools), `PostToolUse` (all tools), `Stop`, `SubagentStop`, `SessionEnd`. Every command is `<absolute path of gitmaster> thread hook --harness claude-code`, with a short `timeout`. The path is absolute so the hook does not depend on `PATH`; `agent status` reports a stale path after the app moves.
- `skills/gitmaster-threads/SKILL.md`: added in T9, together with the MCP tools it explains. Until then the plugin carries hooks only.
- `.mcp.json`: added in slice 7; it starts `gitmaster mcp`.

The plugin is written to the app data directory (`<data>/gitmaster/agents/claude-code/`), never into a repository. The user then installs it one of two ways, both shown by the app and the CLI: the exact commands for the harness's plugin manager, or (when the harness's executable is on `PATH`) a button that runs them. GitMaster never edits the harness's own settings files by itself, and never requires the harness to be installed (D51). Uninstall is the matching command; stored threads stay unless the user deletes them.

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
- **Fail closed (D121):** while any enabled pattern is invalid, capture in every repository pauses. Events are dropped, not stored raw, and counted; the Threads page and `agent status` say "N events not captured: pattern `acme-token` is invalid". Completeness is traded for the guarantee.
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
  parent_id  TEXT,                      -- tool_use id of the call a sub-agent entry belongs to
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

thread_link(thread_id, kind /* 'commit' | 'branch' | 'file' */, ref, origin /* 'observed' | 'manual' */,
  PRIMARY KEY(thread_id, kind, ref))

thread_tag(thread_id, tag, PRIMARY KEY(thread_id, tag))
thread_redaction(thread_id, pattern, count, PRIMARY KEY(thread_id, pattern))
capture_gap(thread_id, at, reason)       -- events dropped for a reason the user can see
entry_fts  -- FTS5, external content over entry(text, tool); edit_fts over edit(path)
```

Limits (starting values, tuned in T2): per field 64 KiB for results and 256 KiB for patches (head and tail kept, `truncated` set, `orig_bytes` kept); 20,000 entries per thread, after which further entries are dropped and a `capture_gap` row says so; no limit on thread count, and no automatic deletion (Q29).

### Search (D128)

FTS5 over entry text, tool names and edit paths. The user's text is turned into a query by the engine, never passed raw: words become prefix terms joined by AND, a double-quoted phrase stays a phrase, and FTS operators typed by accident are escaped. Results are ranked by `bm25` and returned with a snippet around the match, the thread, the turn and the entry. Filters (agent, branch, date range, starred, tag, touched path, has edits) are columns, not syntax, as in D50. The tokenizer keeps identifiers whole (`file_index`, `HistoryStream`) and is chosen in T2 against a corpus of real prompts and code; target: 100 ms for the 95th percentile on 1 million entries.

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
| `UpdateThread { uid, title?, note?, starred?, tags? }` | apply | human | Edits metadata only; never entry text. |
| `LinkThread` / `UnlinkThread` | apply | human | Manual commit links. |
| `DeleteThread { uid }` / `DeleteThreadEntry { id }` | apply | human | A real delete (`secure_delete`); confirmation lists what goes. |
| `ExportThread { uid, path }` | read | human | Markdown; redacted again on the way out. |
| `ScrubThreads` | apply | human | Re-applies the patterns, off-thread, with progress. |
| `ImportThreads { agent }` | apply | human | Reads existing session files for this repository. |
| `ThreadIntegrationStatus` / `InstallThreadIntegration` / `UninstallThreadIntegration` | read / apply | human | Never reaches an agent. |

Ingest is **not** a `Command`. It runs in the hook process, which has no engine and no operation state, and it writes only GitMaster's own database; it does not touch the repository. The engine in the window opens `threads.db` read-mostly (reads, plus its own writes for the commands above) and learns about new data from a watcher on the database file (`-wal` included), debounced like the status watcher, which emits `ThreadsChanged { uids }`. No socket is needed, so the capture works with the app closed.

An agent can never call a write command: the agent actor is refused for everything except the reads, and the reads are exposed through MCP only in T9.

## UI

See `design/screens.md` (Threads page) and `docs/guide/threads.md`. The page follows the History page's pattern: a virtualized list on the left (title, agent chip, branch, relative time, counts of edits, a redaction chip when something was redacted), the detail on the right. The detail groups by turn: the human's prompt, the agent's text, then its tool calls as one-line collapsed chips (tool, argument summary, status, duration); an edit is a card with the path, `+n −m`, the derived state, **Open Diff** (the read-only commit diff view with the patch) and **View File** (the Code page). Search results show snippets and open the thread at the matching entry. The model is pure and tested without a window; the view is thin. Long lists and long threads are virtualized, and loading a thread reads only the visible range plus a margin.

## Settings and files

| What | Where |
|---|---|
| Threads database | `.git/gitmaster/threads.db` (+ `-wal`, `-shm`) |
| Spool, temporary pre-images, log | `.git/gitmaster/threads-spool/`, `threads-tmp/`, `threads.log` |
| Capture switch, redaction patterns, paths, limits | `settings.json`, key `threads` (D127) |
| Per-repository pause | `gitmaster.db` in the app data directory, keyed by the repository's common directory (nothing is written to the repository's config) |
| Generated plugin | `<data>/gitmaster/agents/claude-code/` |

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

1. Real payloads of every hook event of one session that edits files, runs a shell command, uses a sub-agent and is resumed; the matching transcript; the plugin layout and install commands the harness accepts today. Correct "The integration" above.
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

The adapter from T0's fixtures: hook parsing, transcript reader, tool mapping, sub-agents, resumed sessions, `discover` and `ImportThreads`.

Done when: the conformance kit passes for `claude-code` with the real fixtures; the idempotence and order properties pass; a real Git repository is unchanged outside `.git/gitmaster/`; the measured p95 of the hook process meets the budget or Q25's fallback is built.

### T4: integration installer

Per-adapter `integration()` output (Claude Code's first), `gitmaster harness list | install | status | uninstall <harness>`, `ThreadIntegrationStatus` and the install commands, stale-path detection, and the palette actions **Install Agent Integration…** and **Remove Agent Integration**. Until the settings dialog exists (slice 9), the Threads page empty state carries the install step.

Done when: the generated plugin matches a golden copy; install, status and uninstall work against a temporary harness configuration directory in tests; nothing is written into any repository; the commands shown are the ones T0 verified.

### T5: Threads page

Sidebar entry, list, filters, search, detail by turn, edit cards with **Open Diff** and **View File**, empty states (no integration, no threads, capture paused), the capture-paused banner with the reason. Action `threads.show` in the registry, no default shortcut.

Done when: the pure models are tested; the page is virtualized; superseded searches are dropped; a thread of 20,000 entries scrolls smoothly in a release build; checked in a window in both themes (or the gap is stated, since the agent's container has no display).

### T6: curate and link

Rename, note, star, tags, delete (thread and entry), export to Markdown, manual commit links, observed links, the derived edit state, **Show Threads** on a Working Copy file and on a commit, and the context menu and palette entries (registry actions with reasons when disabled).

Done when: every action is in the registry and reachable by menu and palette; deletes are confirmed and verified at the file level; export is redacted again; the file and commit entry points show only their own threads.

### T7: redaction settings

The **Threads** section of the settings dialog, or, until slice 9, a **Threads Settings…** dialog from the Threads page: capture switch, per-repository pause, built-in toggles, user patterns with the test field, path rules, **Scrub Threads**, limits.

Done when: an invalid pattern shows inline and pauses capture with the visible reason; scrub reports the count; settings persist in `settings.json` only.

### T8: observed turn changes (optional, gated)

A working-tree snapshot at `UserPromptSubmit` and at `Stop` through the temporary index, and the diff between them as "Changes during this turn", for edits no tool event covers. Starts with the release-build measurement; ships only if it fits the bar.

### T9: MCP read tools (with slice 7)

`threads_search`, `thread_get`, `thread_read` over the same commands, untrusted-marked output, small by default; the `gitmaster-threads` skill in the plugin; the policy switch. Done when an agent finds a decision made in an earlier thread and cites its `uid` and entry.

## To confirm with the author

Marked **proposed** in `design/decisions.md`; the slice named in brackets waits for the answer:

1. Fail closed when a user pattern is invalid: capture pauses everywhere (D121) [T1].
2. Capture is on for every repository once the integration is installed, with a per-repository pause kept in `gitmaster.db` (D127) [T3a].
3. Tool results are stored, capped at 64 KiB each (D122) [T2].
4. Thinking blocks are not stored (Q31) [T3a].
5. A separate small hook binary if the full binary's cold start misses the budget (Q25) [T3a].
6. `gitmaster thread hook` and `gitmaster harness …` as one-shot subcommands are consistent with D02 (D124) [T3a].

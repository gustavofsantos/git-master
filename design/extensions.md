# Extensions: engineering design

> Proposal. The direction comes from the author (D253, 2026-11-14); the shape below is the agent's proposal and is **Proposed** until the author confirms it. Nothing here is built. The user-facing pages in `docs/` are written once the open questions Q52 to Q55 are answered.

Last updated: 2026-11-14.

## What the author asked

- A way to extend the **user interface**: an extension adds data and functionality to a view, it does not automate Git operations. It is explored in parallel with workflows, because it is not yet known whether workflows fit this app (the author has not used them since they shipped; asking an agent does the same job).
- Extensions are **Lua scripts in a sandbox**, simple files in a known place that expose a known API.
- An extension **registers itself and claims a piece of the UI**, then **fills it through a communication protocol**.
- The protocol is **typed** and works for **both front ends**: the window (GPUI) and the terminal companion (ratatui). Each front end renders the result properly; the extension never draws.
- Start with the **smallest API**: one region, the **commit view**, with a sidebar of extra data about the commit you are looking at. The first example: the GitHub pull request the commit belongs to.
- Later, built-in views (for example a history explorer and viewer) are built on the same machinery.

The design canvas boards 12 to 17 (protocol, commit view with extension sections, sidecar, region map, extensions buffer, module chain) are the visual reference. Board 12 sketches an extension as a process in any language that reads JSON on standard input and is registered in `~/.gitconfig`; the author's request of 2026-11-14 replaces that with Lua files in a sandbox. What carries over from the canvas: named regions, a structured document instead of text with escape codes, cancellation when the focus moves, an extensions screen with state and a message log.

## Central idea

An extension is a Lua file that **claims named slots** of the UI and, when the host asks, **returns a document** for a slot. The document is data: typed nodes (section, field, text, badge, link, list). The host decides **when** to ask (the focus changed), **where** the result goes, **how** it looks (theme, folding, keys, loading and error states) and **what a link does**. The extension decides only **what** to show.

This keeps the rules of the app intact:

- **The UI never blocks.** An extension runs on a worker thread; the native part of the view is drawn first and never waits for it.
- **One engine, two front ends.** Rendering a slot is a `Command` in and `Event`s out; the GUI and the TUI draw the same document.
- **Observed, not reported.** The host records each run (calls, durations, outcome); the extensions screen is built from that record, not from what the script says.
- **Nothing in the working tree.** Extensions live in the user's config, never in a repository.

## Extensions and workflows

They share the machinery and differ in purpose.

| | Workflow | Extension |
|---|---|---|
| Purpose | Acts: composes engine commands | Shows: fills a slot of a view |
| Started by | A trigger (manual, event, phase) | The host, when the focus of a slot changes |
| Effects | `read`, `propose`, `apply`, with approval | None. Read-only in this design |
| Lifetime | Runs to the end; may wait for a person | Short; superseded and cancelled freely |
| Record | Journal in `state.db`, Runs tab | In-memory ring of recent runs, extensions screen |
| Shared | Luau sandbox, budgets, interrupt and cancellation, hash-based trust (D101), `exec` without a shell (D74), generated `.api/` docs and types (D102, D233), `check` from the CLI |

Read-only matters: an extension cannot change the repository, so it needs no approval per action, never enters the command log (D28) and can be cancelled at any point without leaving anything half done. Actions that change something (for example "import the review comments as threads", canvas board 13) are a later step, and when they come they go through the same catalog and approval rules as workflows.

Whether workflows stay is not decided here (Q52). Extensions do not depend on the answer: if workflows were removed, the sandbox, the trust store and `exec` stay because extensions use them.

## Format and location

- One extension per `.lua` file in `~/.config/gitmaster/extensions/` (under `$XDG_CONFIG_HOME` when set; the same on macOS), plus the directories of `gitmaster.extensionPath`. Dot directories are skipped, as for workflows. Nothing is read from a repository.
- The module returns a table with `meta` and `slots`. `meta` is read on load in a sandbox without `ctx`, so the app lists extensions and their claims without running them. Claiming a slot is declaring a function for it in `slots`: the claim and the code that fills it sit together, and the host knows every claim statically.

```lua
local M = {}

M.meta = {
  name = "github-pr",
  description = "The pull request that brought the commit in",
  api = 1,
  exec = { "gh" },
}

M.slots = {}

function M.slots.commit(ctx, commit)
  local r = ctx.exec(
    { "gh", "api", "repos/{owner}/{repo}/commits/" .. commit.hash .. "/pulls" },
    { timeout = "10s", format = "json" }
  )
  local pr = r.json[1]
  if pr == nil then
    return nil
  end
  local state = pr.merged_at and ui.badge("merged", "ok") or ui.badge(pr.state, "neutral")
  return ui.section("Pull request #" .. pr.number, {
    ui.field("Title", pr.title),
    ui.field("Author", pr.user.login),
    ui.field("State", state),
    ui.link("Open in browser", { url = pr.html_url }),
  })
end

return M
```

- `meta.api` is the version of the protocol the extension was written for. The host refuses a version it does not know, with a message, instead of drawing something wrong.
- `meta.exec` lists the programs the extension may run. `ctx.exec` of any other program fails. The review dialog shows the list.
- Returning `nil` means "nothing to show for this focus": the slot takes no space. An error is drawn by the host in the extension's place, in one line, with the reason.

## Slots

A slot is a named place the host exposes, with the focus it passes to the extension and the nodes it accepts.

| Slot | Where | Focus passed | Status |
|---|---|---|---|
| `commit` | GUI: the commit details panel of History, as sections after the native fields and before **Files**. TUI: the Diff screen of a commit, as sections under the commit's header | The commit | First slice |
| `blame.run` | A short note per Blame run (canvas board 14, margin) | The run's commit and lines | Later |
| `sidecar` | A side panel that follows the focus anywhere (canvas board 14) | Whatever has the focus | Later (Q54) |
| `status.section` | A section of the companion's Status and of the Working Copy | The branch | Later |
| `palette` | Commands in the palette and the companion's menu | The focus | Later, with actions |
| `page` | A whole page or screen of its own (`ext/<name>`) | Its own state | Later: the base for built-in views such as a history explorer |

The rules of the region map (canvas board 15) hold: an extension writes only into slots it claims; an empty slot disappears; with no extension, the native layout does not change; the content is per focus and goes when the focus changes.

## The document

The protocol is a tree of typed nodes. Its Rust types live in `gm-schema` (`ext` module, serde and JSON Schema), so both front ends, the CLI and the generated `.api/` types read one definition.

Block nodes:

| Node | Fields | Notes |
|---|---|---|
| `section` | `title`, `children`, `folded` (default `false`) | Folds with the host's key and click. Sections nest up to 3 levels |
| `field` | `label`, `value` (inline or list of inlines) | A label and value row, the label column aligned by the host |
| `text` | `inlines` | A paragraph, wrapped by the host |
| `list` | `items` (each a list of inlines) | Bulleted rows |

Inline nodes:

| Node | Fields | Notes |
|---|---|---|
| `span` | `text`, `tone` | A plain string is a `span` with the default tone |
| `badge` | `text`, `tone` | A short status: `merged`, `approved`, `failing` |
| `link` | `text`, `target` | See below |
| `code` | `text` | A hash, a path, an identifier: monospace in the GUI |

`tone` is one of `default`, `muted`, `ok`, `warning`, `danger`, `info`. The host maps tones to the theme of each front end; an extension never names a color.

`target` is one of:

- `{ url = "https://…" }`: opened in the browser by the host. Only `http` and `https`.
- `{ commit = "<hash>" }`: shows that commit in the app (History in the GUI, the commit's Diff screen in the TUI).
- `{ file = "<path>", line = n }`: opens the file at that line in the app's viewer (GUI) or on the File screen (TUI).

The Lua side builds nodes with the `ui` helpers (`ui.section`, `ui.field`, `ui.text`, `ui.list`, `ui.badge`, `ui.link`, `ui.code`, `ui.span`), which return plain tables such as `{ type = "badge", text = "merged", tone = "ok" }`. A script may also write the tables by hand. The host converts the returned value into the Rust types and rejects it with a precise message when it does not fit: `github-pr: commit: children[2].value: expected a string or an inline node, found a number`.

Limits per document: 200 nodes, depth 4, 2,000 characters per string, 64 KiB in total. Text is shown as text: the host never interprets markup, and it strips control characters and escape sequences, so an extension cannot paint the terminal.

Why a typed tree and not text: the GUI cannot render escape codes; Markdown would leave each front end to guess the layout; a typed tree lets the host fold, theme and bind keys the same way as native sections, and lets the CLI and tests check a document without a window.

Rendering is shared where it can be: `gm-view` gets an `ext_doc` model that flattens a document into rows (indent, kind, spans with tones, fold state, link targets). The TUI draws those rows directly; the GUI draws the same rows with GPUI elements. Fold state is kept per extension, slot and section title, so a section folded on one commit stays folded on the next.

## Lifecycle

1. **Load.** On start and when the watcher sees a change in an extensions folder, the loader reads each file's `meta` and `slots` keys in a sandbox without `ctx`. A problem is shown on the extensions screen (`file:line: message`), and the extension does not run.
2. **Ask.** When the focus of a slot changes (the selected commit), the front end sends `RenderSlot { slot, focus, op }`. The host waits 150 ms for the focus to settle, so moving through the commit list with the keyboard does not start a run per row.
3. **Run.** For each extension that claims the slot and is reviewed and enabled, the host starts a run on a worker pool (4 at a time). Each answers with `SlotRendered { op, extension, result }`, where `result` is a document, empty, or an error with its reason.
4. **Supersede.** A new `RenderSlot` for the same slot cancels the runs of the earlier one: the interrupt hook stops the VM and `exec` kills its process (D65). A late answer is dropped by `op`.
5. **Cache.** Results are kept in memory per extension hash, slot and focus (256 entries). Coming back to a commit shows the kept result at once and asks again in the background when it is older than 5 minutes. **Refresh** in the slot's header asks again now.
6. **Draw.** The native part of the view never waits. While a run is in progress for more than 150 ms the host draws one muted line with the extension's name and a spinner, so the layout does not jump for fast answers. Extensions are drawn in the order of their names; each block ends with a muted line naming the extension, its time and a **Refresh** control.

Budgets per run are those of workflows (D99) with a shorter wall clock: 15 seconds in total, and 10 seconds by default for an `exec`. Exceeding a budget ends the run with the reason, shown in the extension's place.

## What the script receives

`slots.commit(ctx, commit)`:

- `commit`: `hash`, `short_hash`, `subject`, `body`, `trailers` (a list of `{ key, value }`), `author` and `committer` (`name`, `email`, `time`), `parents` (hashes), and `repo` with `root`, `head`, `branch` and `remotes` (`{ name, url }`). All of it is what the commit details already read; asking costs no new Git process.

`ctx` for extensions is read-only:

| Call | Does |
|---|---|
| `ctx.exec(argv, opts)` | As in workflows (no shell, `timeout`, `stdin`, `format = "json"`), in the repository's root, limited to `meta.exec` |
| `ctx.log(text)` | A line in the extension's message log on the extensions screen |
| `ctx.now()` | The time, as in workflows |

Nothing else in this slice. Reading Git beyond the focus (`ctx.git`, read-only subcommands) and HTTP without an external program (`ctx.http`) are left open (Q53).

## Trust

The hash rule of D101 applies unchanged, in the same trust store, with the kind `extension`:

- A new or edited file is **unreviewed** and does not run. Its slot shows one line instead: **github-pr is not reviewed. Review…**. Review opens the dialog with the script and the programs of `meta.exec`; confirming marks the hash as reviewed and runs it.
- An extension an agent writes into the folder therefore never runs before the person has read it.
- An extension can be turned off on the extensions screen without deleting it.
- Output is untrusted: it is data for the host to draw, never instructions. A link to a URL is opened only when the person clicks it.

## The extensions screen

GUI: a page next to Workflows, list on the left and detail on the right. TUI: a screen on the stack (`Space x`). Both show, per extension (canvas board 16): its state (**ok**, **error**, **not reviewed**, **off**), file, slots, programs it may run, the last run with its time and outcome, and the problems of `check`; the actions **Review**, **Turn Off**/**Turn On**, **Open in Editor**, **Copy Path**. A **Messages** section shows the recent runs of all extensions, newest first: slot, focus, outcome, time and the lines of `ctx.log`. It is kept in memory (the last 500 entries) and not written to disk.

## Testing without the UI

As for workflows (D102), one-off subcommands with no resident process:

| Subcommand | Does |
|---|---|
| `gitmaster extension check <file>` | Compiles in the sandbox, validates `meta` and `slots`. `file:line: message`, `--json`, non-zero exit on error |
| `gitmaster extension render <file> --slot commit [--commit <rev>] [--repo <path>]` | Runs the slot for real (it is read-only) and prints the document as the companion would draw it, or `--json`. `exec` runs only when the file's hash is reviewed, or with `--allow-exec` typed by the person |

`render` is what lets an agent write an extension and see its result before the person reviews it. The generated `.api/` folder (D102, D233) gains `extensions.md`, `ui.d.luau` and the `ui` and `ctx` definitions for the Lua language server, generated from the same types.

## MCP

Not in this design. An agent does not need extensions to reach GitHub; it has its own tools. Whether an agent should read what the slots show for a commit is left for later.

## Performance

- Measured in a release build: opening the commit details with three extensions installed draws the native fields in the same time as with none.
- No run starts while the focus moves faster than the settle delay.
- A slow extension (an `exec` that takes its full timeout) never delays another: each run is independent and the pool is bounded.

## Toward built-in views on the same machinery

The author's longer aim is that new views, such as a history explorer, are built on extensions. The design keeps that open without paying for it now:

- The document is versioned (`meta.api`), so it can grow nodes such as `table`, a selectable `list`, `diff` and `graph` without breaking older extensions.
- The `page` slot gives an extension a whole page or screen; with selectable nodes and actions it becomes the place for such views.
- A built-in view can be a bundled Lua extension (like the built-in workflows of `script/builtins/`) or a Rust provider that answers `RenderSlot` with the same document, so the front ends do not care which.

## Slices

1. **X0, docs and decisions:** answer Q52 to Q55; `docs/guide/extensions.md` and `docs/reference/extension-api.md` with `status: planned`; confirm D253.
2. **X1, engine:** `gm-schema::ext` (nodes, validation, JSON Schema); `gm-core::extension` (loader, `meta`/`slots` reading, the sandbox and budgets of `script::luau`, the read-only `ctx`, the `ui` helpers); `RenderSlot`/`SlotRendered` with settle delay, pool, cancellation, cache; trust kind `extension`; `gitmaster extension check` and `render`. Tests: pure tests of validation and the cache; real-Git tests of `render` on a temporary repository with a fake `gh` on `PATH`.
3. **X2, the GUI slot:** the commit details panel draws the documents through `gm-view::ext_doc`; loading, error, not reviewed and **Refresh**; links. UI smoke scenario with a local extension that needs no network.
4. **X3, the TUI slot:** the same on the companion's commit Diff screen; fold with `Tab`, `Enter` on a link.
5. **X4, the extensions screen** in both front ends, and the review dialog.
6. **X5, generated API docs and types** in `.api/`.

## Slice 3w: workflows (in progress: WF0 to WF2 done in TOML, with known gaps; switch to Lua scripts decided on 2026-10-22, D93 to D97; S-D, WS0 to WS2, WF3 to WF5 to do)

Proposal of 2026-10-19, detailed in the TOML workflow specification (available in git history at commit 5f88889, `docs/10-workflows.md`; replaced by `design/workflow-scripts.md`), with decisions D60 to D80 in `design/decisions.md` (D73 to D75, D78 and D79 decided in principle; the rest proposed). A workflow is a sequence of typed engine actions, tied to a trigger and declared in TOML, one file per workflow. Each step is a `Command`, so this slice adds no new logic path. The stack policies and the LLM commit message become workflows, with no specific code. The author confirmed the position (Q20): right after slice 3 and before 3c. WF4 and WF5 come with slices 4, 5 and 7.

**Change of direction (2026-10-22, D93 to D97, design in `design/workflow-scripts.md`).** The author decided to replace the TOML state machine with Lua scripts running in an embedded sandbox. Execution stays ephemeral for now. Durable execution in the style of Temporal is postponed (Q23). WF0 to WF2 below stay as a record of what was built. The slices that follow supersede them. From the code, `store`, `driver`, `exec`, `loader`, the session, `catalog`, the run view and the palette survive. `model`, `parse`, `expr`, `validate`, `machine`, `edit`, `dry_run` and the Flow tab are removed. WF3 has not started, so it will be built once, on scripts. There is no support for both formats.

- **Spike S-D, script engine (to do; isolated, like S-A to S-C, and can run at any time).** It compares Luau (`mlua`), Lua 5.4 and Rhai on:
  - sandbox escape attempts (`os`, `io`, `load`, `require`, `string.dump`, a memory bomb, an infinite loop, deep recursion, changing globals between runs);
  - instruction, time and memory budgets;
  - cancellation latency, including a blocked call;
  - the cost of a new VM per run;
  - binary size;
  - building on the three platforms and in CI.

  Result in `spikes/script-engine/FINDINGS.md` and in `design/decisions.md` (Q22). Done when: an engine is chosen, with numbers and a list of escape attempts that fail.
- **WS0, script core (to do).** Adds the `ScriptEngine` port in `gm-core`, with a VM adapter and a fake `ctx` for tests. The `ctx` is generated from the catalog (level, signature, outcomes). It also covers budgets and cancellation, the call journal, and ephemeral execution of a run on its own thread, with blocking `approve` and `wait` (D95). An interrupted run is marked `interrupted` on the next open. Done when: a sample script runs against real Git in a temporary repository (`crates/gm-core/tests/workflows_real_git.rs`, adapted); an infinite loop, a memory bomb and each escape attempt end in `failed` with the reason; cancelling wakes a blocked `approve`; killing the engine in the middle of an `apply` leaves the run `interrupted` on the next open.
- **WS1, loader, `M.meta`, models, approval by hash and CLI (to do).** Covers a loader for `.lua` files (user directory and `gitmaster.workflowPath`), `M.meta` read without `ctx`, built-in models rewritten in Lua, and trust by hash (D96, "Run this version?" dialog). It also sets the rule that `apply` runs after `exec`, generated documentation in `.api/` (D97), and the subcommands `gitmaster workflow check`, `dry-run` and `test`. Done when: `suggest-commit-message` and `branch-from-ticket` in Lua run from the palette; an edited script goes back to "not reviewed"; `check` points to the file and line of an error; `test` checks the call sequence of a script against a fixture; `dry-run` lists the calls without running `exec`; an agent can write a new script using only `.api/` and the subcommands.
- **WS2, interface and removal of the TOML engine (to do).** A Workflows page with Script and Runs tabs (one entry per `ctx` call), dry run on the open text, atomic save, and a conflict when the file changed on disk. It also removes `model`, `parse`, `expr`, `validate`, `machine`, `edit`, `dry_run`, `flow_graph*` and `flow_inspector`, the `.toml` fixtures and the `w-flow-editor` scenario. A `.toml` in the directory is ignored with a warning. Done when: a script is created from a template, edited, saved and run, with the Runs tab showing the calls; no code of the old format remains; a UI smoke scenario exists.

- **WF0, pure core (done, with known gaps; superseded).**
  - Built on 2026-10-19 in `gm-core`, with no I/O: a TOML parser for a typed state-machine model (D78), an expression evaluator, a validator for the D78 rules, and the pure transition function (D80). Unit and property tests show that the transition is deterministic and that every run ends. The four example files load without error.
  - Known gaps: the catalog checks names, levels and outcomes, but not the types of `with` fields. Nothing ran (no loader, engine or UI), so nothing was seen in a window.
  - Superseded by the Lua scripts (D93 to D97). The TOML specification is in git history at commit 5f88889 (`docs/10-workflows.md`).
- **WF1, engine, manual trigger and `exec` from the command line (D74) (done, with known gaps; superseded).**
  - Built on 2026-10-19 (D81 to D83): a file loader (user directory and `gitmaster.workflowPath`) that reloads on change; run state persisted in SQLite, with each transition in a transaction and the last 200 runs kept per workflow and repository; the actions `notify`, `approve`, `exec`, `commit.suggest_message`, `stage.all`, `commit.create`, `branch.create` and `branch.checkout`; cancellation and resumption.
  - UI: palette actions "Run Workflow…", "Approve/Reject Workflow Step" and "Cancel Workflow Run", a state line in the composer, and a palette entry per workflow. Seen in a window: `suggest-commit-message` ran from the palette and filled the composer.
  - Known gaps: the palette shows a generic reason for a file that fails to load, not the engine error; the list reloads only when the repository opens and at the end of a run; no smoke scenario; a pending `approve` returns on resume without its summary.
  - Superseded by the Lua scripts (D93 to D97). The TOML specification is in git history at commit 5f88889 (`docs/10-workflows.md`).
- **WF2, Workflows page (D76, with the Flow tab of D78) (done, with known gaps; superseded).**
  - Built on 2026-10-19 and 2026-10-20 (D84 to D86): a Workflows page with a sidebar entry and a list. It has three tabs. Flow shows a state graph beside state cards, with an inspector for editing fields. TOML is a text editor with validation and atomic save, and edits keep comments. Runs shows the history and highlights a run's path on the graph ("Show in Flow"). Dry run is in the header. Templates are copied to the user directory, never to the repository.
  - Known gaps: a cycle lane can sit beyond the visible width of the graph column; implicit failure edges (to `end.failed`) are not drawn; dry run assumes `done` and empty output and ignores the effect of an `if`; `for_each`, `wait` and `set` states can be edited only in the TOML tab; every input is typed `string`.
  - Superseded by the Lua scripts (D93 to D97). The TOML specification is in git history at commit 5f88889 (`docs/10-workflows.md`).
- **WF3, event triggers, waits, phases and migration of hooks (D75, D93 to D96), now on scripts and after WS0 to WS2.** Covers:
  - engine events as triggers, `match`, debounce, and concurrency (in memory, D95);
  - automatic approval of event `apply` steps, tied to the reviewed script hash (D96), and `Stale` reconciliation;
  - `ctx.wait` for events such as `restack.resumed`, while the app is open;
  - `before.<operation>` phases (a gate, with `ctx.block`), `transform.<field>` (the returned value) and `after.<operation>` (observation);
  - parity of `transform.commit_message` with `prepare-commit`;
  - the action "Convert hooks to workflows", which writes the equivalent Lua scripts into the user's workflow directory;
  - GitMaster stops reading the `[gitmaster "<hook>"]` sections of `.gitconfig`.

  Done when: the behavior of `prepare-commit` is reproduced by a workflow and the hook code path is removed; a workflow on `commit.created` runs; the `apply` step waits for approval; editing the file sets the hash back to "not reviewed" and revokes automatic approval; closing the app in the middle of a run leaves it `interrupted` on the next open (D95); a run that calls `ctx.wait("restack.resumed")` stops and resumes when the event arrives, while the app is open.
- **WF4, stack actions and templates (with slices 4 and 5).** `ctx.stack.*`, `ctx.restack.*`, `ctx.reparent.*`, `ctx.push.lease`, the events `stack.parent_moved` and `stack.parent_merged`, and three built-in templates, in Lua. Done when: turning on "Restack children when parent moves" restacks the subtree after a commit on the parent, with a plan, undo and a pause on conflict, and with no code specific to this policy.
- **WF5, MCP (with slice 7).** `workflow_list` and `workflow_run`. There is no tool to create or edit scripts (D97). Done when: an agent lists the workflows and fires one; the `apply` steps of the run wait for the human's approval in the UI (D64(c)).

## Slice 3c: code navigation (in progress: S-A, S-B, S-C, N1, N2 and N3 done; N4 next)

Details, engines and risks are in `design/code-navigation.md`. Decisions D43 to D51 are in `design/decisions.md`: D43 is decided; D44 to D51 are proposed, with D44 and D46 supported by the spikes (D89). The slice adds a Code page with Quick Open for files, grep and symbols, on top of a read-only viewer. Each sub-slice has its "Done when" in `design/code-navigation.md`.

- **Spikes S-A, S-B and S-C** (lists and grep at scale, tree-sitter, path matcher), before building.
  - **S-A and S-C done on 2026-10-21** (`spikes/nav-a-lists/FINDINGS.md`, `spikes/nav-c-match/FINDINGS.md`, D89). They confirm D44 and D46 at 100 thousand files, and the matcher up to 1 million. Still to measure on a Mac: grep's `--threads`, and grep on the 1-million-file worktree.
  - **S-B done on 2026-10-22** (`spikes/nav-b-treesitter/FINDINGS.md`, D92). Tree-sitter is viable (speed, size, accuracy), but only with its own queries. macOS was not measured; `measure.sh` and `run.sh` run there unchanged.
- **N1, Quick Open** for files in the active worktree (done on 2026-10-21, with N2; D89, D90; no smoke scenario, per D88).
  - `Ctrl/⌘+P` opens a layer with a field and up to 10 visible lines (50 results). Each line shows the file name and, in a weaker style, its folder, with matched letters highlighted. `Enter` or a click opens the file in the Code page; `name.rs:120` opens at line 120. `Esc` closes it. "Quick Open…" is a registry action (palette and key) and needs an open repository.
  - The file list is read once from `git ls-files` and kept in memory. A new search cancels the previous one, and a late answer is dropped by its `OperationId`. The first search waits for the first read of the list.
  - Done when: the matcher, property and real-Git tests pass. The list matches `git ls-files` for ignored, renamed, spaced, quoted and Unicode names, deleted files and sparse checkouts, and reading it writes nothing to the repository. The engine and UI model tests also pass, and so do lint and tests. Nothing was seen in a window by the agent.
  - Known gaps:
    - the list is reread only when Quick Open opens, not by the watcher;
    - no recent files for an empty query, and no folder filter;
    - the ranking weights are a first draft, not tuned on real repositories;
    - a non-ASCII query uses the slow per-character path;
    - a file name that is not UTF-8 shows replacement characters and does not open;
    - submodules appear as files and open as folders;
    - a failed list read with no search waiting is silent;
    - the field has no UI test;
    - with untracked files the list takes about 0.75 s on 100 thousand files, not 0.05 s for tracked files; a two-phase list (tracked first, untracked later) would fix it, but it was not built.
- **N2, viewer and tree** (done on 2026-10-21, with N1; D90; no smoke scenario, per D88).
  - The **Code** page (a "Code" sidebar line and a "Show Code" action) has the folder tree on the left and a read-only viewer on the right. The tree shows one folder at a time: folders first, then files, each group sorted by name. A folder whose only content is another folder is shown joined (`a/b/c`). Opening a file from Quick Open expands the folders above it and scrolls the tree to it.
  - The viewer shows the worktree file, with line numbers and virtualized rows in the code font. The wheel scrolls sideways in steps of 4 columns, tabs are 4 columns wide, and the line is highlighted when the query has `:line`. The header shows the path, the size and, when cut, "showing the first part".
  - Guards: binary files, missing files ("not in the worktree now"), symlinks (they show the target and never follow it), folders, empty files, non-UTF-8 text (replacement characters), and files over 2 MiB, which are cut at a line break. The engine refuses absolute paths, `..`, and paths that leave the worktree through a link. Pipes and devices are not read.
  - Entry points: Quick Open, the "Code" line, and "View File" in the context menu of a working copy file and in the palette (Tab picks the file).
  - Done when: the engine tests for each file kind and guard pass, the UI model tests pass, the real-Git folder list test passes, and lint and tests pass. Nothing was seen in a window by the agent.
  - Known gaps:
    - no syntax highlighting (N4), no text selection or copy, no wrapping, no typed go-to-line and no copy of `path:line`;
    - horizontal scrolling is wheel-only and long lines are cut;
    - the tree has no keyboard support, cannot be resized and has no search;
    - "View file" from a commit's detail and diff is not included, because the viewer shows the current file, not the commit's (N6);
    - the tree reloads only when the page is entered;
    - folder names that are not UTF-8 do not open.
- **N3, grep** with `git grep` (done on 2026-10-22; D44, D91; no smoke scenario, per D88).
  - The **Search** page ("Search" sidebar line; action "Search Text…", `⇧⌘F` / `Ctrl+Shift+F`) has a text box, four toggles (`Aa` case, `ab` whole word, `.*` regular expression, `new` new files) and a files box (`src crates/ui !docs`: space-separated, with `!x` to exclude). The search starts 250 ms after typing stops, or at once on Enter or on a toggle change. Results are grouped by file, with matched lines and snippets highlighted. Clicking a line opens the file at that line in the Code page. Clicking a file collapses or expands its lines. The summary line shows the count, "Searching…", or Git's message for an invalid regex or pathspec.
  - The engine reads Git's output as it arrives. It kills Git when the output reaches the limit or when a newer search asks for it. Exit code 1 (no matches) is not a failure; code 128 returns the first line of Git's error. Nothing is written to the repository, and the command is not in the command log (D28).
  - Done when: pure tests (arguments, validation, parsing, ordering, limits, highlights, properties) pass. Real-Git tests cover edge cases such as option-like text, binary and ignored files, renamed and deleted files, spaces, Unicode, quotes, and pathspecs that include and exclude. Tests with fake Git prove that the process is killed. Engine and UI model tests pass, and so do lint and tests. Nothing was seen in a window by the agent.
  - Measured on 100 thousand files: for tracked files only, a common pattern reaches the limit in 0.065 s and a full scan takes 0.9 s. With untracked files, the times are 1.1 s and 2.0 s. The extra 1 s is Git's worktree scan for untracked files, the same cost as `status`, and it is paid even when there are none. See the correction in `spikes/nav-a-lists/FINDINGS.md` and D91.
  - Known gaps:
    - the result arrives whole, with no streaming or progress, so a full scan on a huge repository (about 9 s over blobs in 1 million files) stays at "Searching…" until it ends; only a new search cancels it;
    - the search covers the worktree, not a revision;
    - the files box splits on spaces, so a path with a space works only through a glob;
    - the regex is Git's extended syntax (`-E`), with no `\d`, and highlighting uses Rust's `regex` and may differ from Git in rare cases;
    - a file name with a line break splits the record, and that line disappears;
    - the results list has no keyboard support and no text selection;
    - `--threads` must be measured on the Mac;
    - the Code page's search field and Quick Open are still separate boxes (the single box is N5, D50).
- **N4, syntax highlighting** (S-B done, D92; proposed "Done when" in "How to continue", above, to be agreed with the author before starting).
- **N5, symbols and single box**: index by blob, single search box (D50), outline, go to definition.
- **N6, revisions, MCP Read tools and performance gate.**

Done when: opening a file by name, finding text and jumping to a definition takes seconds in a 100 thousand file repository, without freezing the UI.

## Slice 4: stacks (model and visualization)

- Metadata `stackParent` and `stackParentBase`; create and reparent a branch (drag).
- Sidebar per stack; lag indicators; dashed edge in the graph.
- Derived events `stack.parent_moved` and `stack.parent_merged` (D68), which feed workflow triggers.

Done when: a real everyday stack is represented and the lag is visible.

## Slice 5: restack

- Exact plan per commit, in-memory execution, snapshot and atomic undo, persisted state.
- Fallback without a resolver: on conflict, pause and deliver the conflict in the working tree to resolve outside.
- Workflows `before.restack` and `after.restack` (the old `pre-restack` and `post-restack` hooks, D75).
- Decide Q2 (dirty working copy).
- Primitives exposed as Commands: `restack.plan`, `restack.apply`, `restack.undo`, `reparent.*` and `push.lease` (D68).
- The automatic update policies are workflow templates (WF4), with no specific code (D68).

Done when: a complete restack runs, can be undone, and survives closing the app.

## Slice 6: conflict resolution

- Three panes, with sides labeled by role, base on demand, rerere.
- The hook `resolve-conflict` becomes the workflow `transform.conflict_hunk` (D75).

Done when: the conflicts of a restack are resolved without leaving the app.

## Slice 7: MCP

- Server in the app, plus a socket shim.
- Read and Propose tools first; Apply with UI approval later.
- Config policy; operation log with `actor`.
- Tools `workflow_list` (Read) and `workflow_run` (WF5, D71).

Done when: an agent can read the stack, propose a restack and apply it with approval.

## Slice 8: local review

- Comments, states, anchor without guessing, before and after snapshots.
- Tools `review_comments_list` and `review_comment_resolve`.
- Inline comment on the diff, with Before/After.
- Confirm D11 and Q3 before starting.

## Slice 9: premium polish

Decided by the author on 2026-10-17 (D38): the settings screen goes into this slice, consolidating everything the app allows configuring up to this point.

1. **Settings screen (settings modal, D38)**
   - A centered modal dialog in GPUI, with a semi-transparent dimmed backdrop (`black/50`). It closes with the `Esc` key, a click on the backdrop, or the close button.
   - Two columns: a category sidebar on the left (fixed width of about 200 px) and a form panel on the right, with its own scrolling.
   - Access: the shortcut `⌘,` (macOS) or `Ctrl+,` (Linux/Windows), the typed action `open_settings` in the registry (`action_registry.rs`), the command palette (`Open Settings`), and a gear button in the footer.
   - Changes are saved at once, with no "Save" button. The rule to stay out of the working tree still applies (Principle 5):
     - `git config`: identity, default branch, worktree root and hook settings go to `~/.gitconfig` (global) or `.git/config` (local), through `git config`.
     - `settings.json`: theme, font, recent-project and MCP preferences go to the user config directory (`~/.config/gitmaster/`).
     - `hook-approvals.json`: the list of script approvals by hash, in the data directory (`~/.local/share/gitmaster/`).
   - Sections, covering slices 1 to 8. The field-level list is in `docs/reference/configuration.md`:
     - *General*: appearance, UI and code fonts, recent-project cleanup, status watcher file limit.
     - *Git & Identity*: scope selector (Global vs This Repository), identity, default branch, editor and commit signing.
     - *Worktrees*: default root directory (`gitmaster.worktree.root`, D31) and safe-removal protection options.
     - *Diff & Files*: diff algorithm, context lines, ignore whitespace by default.
     - *Hooks & Security*: the active hook catalog and the approvals table in `hook-approvals.json`, with a "Revoke" button. This moves into *Workflows* after WF3.
     - *Stacks & Restack*: default push strategy with lease, and the policy for restack with a dirty working copy (Q2).
     - *Conflict Resolver*: default style (`merge.conflictStyle = zdiff3`) and rerere reuse.
     - *MCP & Agents*: control of the internal server, socket, token management, and the authorization matrix for `Actor::Agent` actions (D20).
     - *Local Review*: where comments and snapshots are stored.
     - *Workflows*: directories (`gitmaster.workflowPath`), per-repository activation, and automatic approvals by hash with a "Revoke" button.
     - *Workflows, history*: retention of run history (D80).
   - A smoke scenario in `ui-smoke.sh` covers opening with the shortcut, switching the theme, and closing with `Esc`.
2. **Performance in monorepos**: profiling and optimization of diff, status and revwalk in massive repositories.
3. **Alternative layouts**: a unified or stacked layout for smaller screens (Q9).
4. **Packaging and distribution**: native packaging (macOS, Linux), desktop metadata, and the icons from `design/visual-identity.md`.
5. **Documentation and product model**: end-user documentation, license terms and the premium product model (Q10).

Done when: every option of the app and of the Git environment can be inspected and adjusted in the settings modal, without editing configuration files by hand, and the binary is packaged for native installation.

## Cross-cutting risks

- GPUI: editable text, component ecosystem and licenses.
- Scale: status, graph and diff in massive repositories.
- Scope: reference clients have pull requests, worktrees, submodules and LFS. Worktrees enter v1 (D31); pull requests, submodules and LFS remain open (Q8).

# Decisions and open questions

Last updated: 2026-10-22

## How to read the status

- **Decided**: the author stated it explicitly.
- **Proposed**: suggested during the design exercises and not contested. It needs confirmation before it becomes a commitment.
- **Open**: no answer yet (see the open questions section).

## Decision log

| ID | Decision | Status | See |
|---|---|---|---|
| D01 | Implementation in Rust with GPUI, native; no Tauri and no JS front end | Decided | architecture |
| D02 | Local product, for use by one engineer; no headless mode, CI or server | Decided | principles |
| D03 | Each branch has a single parent and several children (a tree, not a DAG) | Decided | stacks-restack |
| D04 | Hooks follow the idea of agent event hooks, with their own input and output schema | Decided · Superseded by D75 after WF3 | hooks-transitional |
| D05 | Hooks run asynchronously; the UI never blocks because of a slow script | Decided | hooks-transitional |
| D06 | A hook receives a correlation id on input and returns the same id on output; the engine reconciles them | Decided | hooks-transitional |
| D07 | A hook replies through stdout (no detached or callback model) | Decided | hooks-transitional |
| D08 | Single engine with two front-ends (GPUI and MCP) | Decided | architecture |
| D09 | Avoid non-deterministic predictions | Decided | principles |
| D10 | Sidebar with a dual view: by stack and by name | Decided | screens |
| D11 | Local review loop (comments, agent via MCP, feedback, before/after) | Proposed | mcp-server, local-review |
| D12 | MCP server inside the app; `gitmaster mcp` shim over a Unix socket with a token | Proposed | mcp-server, local-review |
| D13 | Stack metadata in git config: `stackParent` and `stackParentBase` | Proposed | stacks-restack |
| D14 | Restack with an exact plan per commit, a ref snapshot, atomic undo and persisted state | Proposed | stacks-restack |
| D15 | Push with lease as an explicit step after the restack; in a workflow, `push.lease` always asks for approval (D64) | Proposed | stacks-restack |
| D16 | Hooks in three classes (gate, transform, observe); gates and observers run in parallel, transforms run in sequence | Proposed · Superseded by D75 after WF3 | hooks-transitional |
| D17 | Repo hooks require approval (command hash); the default reads only global/user config; `includeIf` per project | Decided (2026-10-16) · No longer applies with D73 after WF3 | hooks-transitional |
| D18 | Native Git hooks keep running, before GitMaster's; GitMaster neither reimplements them nor wraps them (see D29) | Decided (2026-10-16) | hooks-transitional |
| D19 | ~~`pre-commit` runs against the index tree~~ Superseded by D29: `pre-commit` belongs to Git | Superseded (2026-10-16) | hooks-transitional |
| D20 | MCP in tiers Read, Propose, Approval, Apply; apply accepts only `plan_id` | Proposed | mcp-server, local-review |
| D21 | Deterministic hunk IDs (content hash) | Proposed | mcp-server, local-review |
| D22 | Three-pane conflict resolver, with sides labeled by role (not ours/theirs) | Proposed | screens |
| D23 | Working Copy: "staged" and "read" as orthogonal axes; a single list with a tri-state checkbox | Decided (2026-10-16) | screens |
| D24 | Line-level staging on an exact diff model, writing the index blob directly | Decided (2026-10-16) | screens |
| D25 | History: stack lag shown as a dashed edge; commit owner taken from the stack metadata | Proposed | screens |
| D26 | Hybrid Git backend: a `git` subprocess for every operation with side effects that is native to Git (stage, unstage, commit, checkout), for status at scale, and for reading history (`git log`, see D32); libgit2 for the restack (which Git does not have) and the other reads. Revised 2026-10-16 and 2026-10-17 | Decided (2026-10-16) | architecture |
| D28 | The Git command log records only operations with side effects; reads (status, diff, history) do not appear | Decided (2026-10-16) | roadmap |
| D29 | GitMaster does not touch what is native to Git: `pre-commit` and the other native hooks stay with Git (they run through `git commit`, and their output shows up as a commit error). Only what Git does not have is a GitMaster hook; slice 2 delivers only `prepare-commit` | Decided (2026-10-16) | hooks-transitional |
| D30 | Hook contract in pure JSON (Q1, option A): no `mode = text`, no codec on the host | Decided (2026-10-16) · Superseded by D75 after WF3 | hooks-transitional |
| D27 | Start with four crates (schema, core, ui, mcp) and extract the rest on demand | Proposed | architecture |
| D31 | Worktrees come into v1 (core of the author's flow); defaults and out-of-scope items are in the D31 section below | Decided (2026-10-17) | screens, roadmap |
| D32 | History reading runs `git log` as a subprocess: it streams the output, uses the commit-graph that already exists, and never writes it. Corrects D26 (which said libgit2 for bulk reads) | Decided (2026-10-17) | architecture, roadmap |
| D33 | History list (H1): a flat list of commits from HEAD, the first 200, 48 px rows with ref chips, subject, short hash and relative time. HEAD with no commits shows a message. Reading is a command with no entry in the command log (D28) | Decided (2026-10-17) | roadmap |
| D34 | History scope (H6): a pill with three modes: current branch, all refs, or one ref. A left click on a branch in the sidebar, on the history page, sets the scope; checkout stays on double click or in the context menu. "Current stack" appears disabled with "Not available yet" | Decided (2026-10-17) | screens, roadmap |
| D35 | Content search (H7) with fixed-text `git log -S`. The field sits in the history header, not in the top bar; the search is debounced and cancellable | Decided (2026-10-17) | screens, roadmap |
| D36 | Graph lane color (H5, D25) derives from a hash of the branch name, stable across sessions. The layout is deterministic and uses `--topo-order`. D25 stays Proposed for the rest (dashed edge, commit owner) | Decided (2026-10-17) | screens, roadmap |
| D37 | History performance gate (H8): benchmarks ignored in repositories of 50 thousand and 200 thousand commits, generated with `git fast-import` | Decided (2026-10-17) | architecture, roadmap |
| D38 | Settings screen as a modal dialog in Slice 9; it configures everything the app allows until then, persisting outside the working tree (Principle 5) | Decided (2026-10-17) | screens, roadmap |
| D39 | Diff layout: Unified \| Split (H4b): the selector applies to the commit diff and, in sub-slice H4b-B, to the Working Copy diff, with a single line model. This step delivers the commit diff first | Decided (2026-10-17) | screens, roadmap |
| D40 | The user's diff layout preference is saved in `settings.json` in the user config directory (`~/.config/gitmaster/` on Linux, `Library/Application Support/gitmaster/` on macOS), outside the repository (Principle 5). Default: Unified. A missing or invalid file counts as Unified. The settings screen (D38) starts exposing the option | Decided (2026-10-17) | screens, roadmap |
| D41 | Split mode: each side's gutter marks only that side's line (the pair is not a unit); the pair warning from D24 still applies to a half pair; a set of chunk buttons sits on the hunk header line across the full width; no line wrapping, with a shared horizontal scroll; an empty side is a plain panel, with no number or marker; no automatic switch to Unified in a narrow window | Decided (2026-10-17) | screens, roadmap |
| D42 | Intraline diff highlighting (highlighting differences within a line) is deferred until after Slice 3 | Decided (2026-10-17) | roadmap |
| D43 | Code navigation (`code-navigation`): a Code page with Quick Open for files, grep and symbols, over a read-only viewer of the active worktree; arbitrary revisions only in N6. The scope is decided: the product is a Git client joined to tools for working with code, not a clone of any existing client (`principles`). The position in the roadmap is still open | Decided (2026-10-18) | principles, roadmap, code-navigation |
| D44 | Grep with `git grep` as a subprocess (same argument as D32: already installed, respects `.gitignore`, reads blobs from any revision, streams, cancellable). The pattern always goes after `-e`, the pathspec after `--`. If the benchmark requires it, the alternative is in-process search with Rust crates, not another program (D51) | Proposed | code-navigation |
| D45 | Symbols with tree-sitter compiled into the binary (`tags.scm`) for a curated set of languages; index addressed by blob OID, derived and disposable, outside the working tree. "Find usages" is `git grep -w`, and the interface says it is text. The author wants structural analysis, on the condition that the app stays smooth and fast: the index runs in the background at low priority, and if spike S-B does not show this, the first version ships without symbols (files and text). Confirm at spike S-B (build, binary, quality, smoothness) | Proposed | code-navigation |
| D46 | File list from `git ls-files -z` (or `ls-tree -r` at a revision) in a contiguous buffer, updated by the watcher; its own path matcher (file-name bonus, parallel top-K), separate from the `fuzzy.rs` used for actions | Proposed | code-navigation |
| D47 | Out of scope: editing, LSP and semantic references, an own text index (Zoekt-style) | Proposed | code-navigation |
| D48 | The MCP Read tools (`files_search`, `code_grep`, `symbols_search`, `file_read`) enter Slice 7, over the same engine commands the UI uses | Proposed | mcp-server, local-review, code-navigation |
| D49 | Navigation performance gate (N6): benchmarks ignored in repositories of 100 thousand and 1 million files, generated with `git fast-import`; targets to calibrate in the spikes | Proposed | code-navigation |
| D50 | Single search box, no syntax: a deterministic rule-based classifier picks the engines (files, symbols, usages, text), which run in parallel and go into sections in a fixed order. No LLM in the path; filters are only an optional shortcut | Proposed | code-navigation |
| D51 | GitMaster is self-contained: navigation requires no program beyond `git`, which the app already requires. No ripgrep, ctags, ast-grep or language server (LSP) as executables; extra engines come in only as Rust libraries compiled into the binary | Proposed | code-navigation |
| D52 | Long branch names in the screens that show them (header, selector, sidebar, palette, worktrees, commit): the head of the name shrinks with an ellipsis and the last 16 characters always stay visible, without measuring text; icons and counts do not shrink. Above 24 characters, the full name appears in the tooltip | Proposed (2026-10-19) | screens |
| D53 | Branch filter in the sidebar: scope "Mine" or "All", and search by name. The default scope is Mine when Git has `user.email`, otherwise All; the user's choice overrides the default and is saved in `settings.json` (D40), but the search is not saved. Mine shows the current branch (always, even with another author), those whose tip commit has the user's email (case-insensitive), and local branches whose author cannot be read (an unknown author does not hide the branch); remote branches without an author stay hidden. The search uses the palette's fuzzy matching over the full name and combines with the scope. The palette and the top selector keep searching all branches. UI labels in English: "Mine", "All", "Show Only My Branches", "Show All Branches" | Proposed (2026-10-19) | screens, roadmap |
| D54 | Fetch, Pull and Push with lease (Slice 1d). Fetch: `git fetch --all --prune`. Pull: `git pull --ff-only` (it never creates a merge or a rebase; if it does not advance, Git's message is shown unchanged); refuses a detached HEAD and a branch without an upstream. Push with lease only for the current branch, with a full refspec and `--no-follow-tags`: to the upstream when there is one; with no upstream and exactly one remote, `--set-upstream` on that remote; with zero or several remotes, it refuses without guessing. Only `Actor::Human` operations enter the command log. One remote operation at a time, on its own thread, so the busy state reaches the UI; `GIT_TERMINAL_PROMPT=0` avoids a stuck prompt | Decided (2026-10-19) | roadmap |
| D55 | Create and delete a local branch (Slice 1d). Create: `git branch` or `git checkout -b`, with the name validated by `check-ref-format` rules before Git. Delete: `git branch -d`; `-D` runs only behind a confirmation box in the dialog. Refuses the current branch and the one checked out in another worktree. Human only. "New child branch" stays disabled (it depends on stack parents). Remote branches are left out | Decided (2026-10-19) | roadmap |
| D56 | "Delete Merged Branches" (Slice 1d): always in two steps, a list and then a confirmation. Base: the local branch that `origin/HEAD` points to; otherwise the first of `main`, `master`, `trunk`, `develop`; otherwise the current branch. Never included: the base, the current branch, branches in another worktree, and the protected names (`main`, `master`, `trunk`, `develop`, `dev`, `staging`, `production`, `release`). Only branches merged into the base and also into HEAD are included, because that is what `git branch -d` accepts. It deletes only with `-d`, and if the list has changed since the preview, it refuses everything. Squash and rebase merges are not detected | Decided (2026-10-19) | roadmap |
| D57 | Resizable sidebar (Slice 1d): minimum 200 px, at most 40% of the window; double click returns it to 240 px. The left column gives way so the main area keeps 320 px. The width is not saved between runs (the left column's width is not either) | Decided (2026-10-19) | roadmap |
| D58 | Selectable commit panel text (Slice 1d): the `TextArea` gained a read-only mode (an option, not a fork). Subject, body, hash, author, date and parents are separate areas. Copying is a registry action (`commit.copy_hash`, `commit.copy_message`); file paths are not selectable, so they do not conflict with the click that opens the diff | Decided (2026-10-19) | roadmap |
| D59 | Worktrees reachable without choosing a branch first (Slice 1d): an action with no argument, "New Worktree…" (in the palette and on the "+" button in the section); a "×" that appears when hovering over a linked worktree row; and the palette selects the worktree (Tab) to open, reveal, copy, remove, lock and unlock | Decided (2026-10-19) | roadmap |

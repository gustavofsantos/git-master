# Slice 1: the shell

## Branch

The brief asks for a `ui/redesign` branch. AGENTS.md says all development happens on `develop`, no feature branches, and the task says AGENTS.md wins on conflict. All slices are committed to `develop` and pushed to `origin develop`.

## Scope

1. Sidebar, one component: `sidebar_nav.rs` (pure: pages, footer, which page a screen belongs to, badges) and `shell_sidebar.rs` (the GPUI renderer).
   - Pages: Inbox, Changes, Review, History, Code, Agents, with count badges (Changes: changed files; Review: open comments; Agents: threads).
   - Topic section: STACK. Before topics exist (slice 5) it shows the stacks of the repository with their marks (restack N, ↑N, amber dot, parent gone/merged/diverged tags) and keeps drag to set parent. Only while the Stacks preview is on.
   - BRANCHES · <repo>: today's By name tree, filter, N of M, HEAD badge. Folded by default.
   - WORKTREES: unchanged. Folded by default.
   - Footer: Stashes, Reflog, Workflows, Settings (Settings is planned: shown disabled, layout only).
   - The By stack / By name switch goes. The Workspaces section leaves the sidebar; every workspace action stays in ⌘K (palette section Workspaces) until slice 5.
   - Screens map to pages: Changes = Working Copy (and the conflict resolver), Review = Local review, Code = Code and Search, Agents = Threads and Labels, Inbox = a new screen (empty placeholder until slice 6).
2. Top bar, three zones.
   - Left: repository name (project menu, as today) over "on <branch> ▾" (branch picker). The path leaves.
   - Middle: "Selected" + the selection and up to three ranked actions; "⋯ ⌘K" opens the palette scoped to the selection's actions. Model in `context_actions.rs`: the selection is a `context_menu::Target` (the context menus are the registry's per-target views already), ranking keeps menu order, drops disabled, destructive and confirm-first actions, takes three. Selections: Changes → file; History → commit; Agents → thread or label; Review → review comment; Stashes → stash; Reflog → entry; Code → viewer.
   - Right: the sync button (`sync_state.rs`, pure): label is the state (Push ↑n, Pull ↓n, Pull ↓n ↑m when diverged, Publish when there is no upstream, Up to date, or the busy label); a click runs the primary action; ▾ opens a menu with Fetch, Pull, Push with lease (registry actions). "Restack N behind" shows only when the stack of HEAD has stale branches. Background fetch: every 10 minutes while a repository with a remote is open and nothing remote runs; its failure is not shown as a notice (it is in the Git log).
3. Status bar: hook chips and transient notices as today, **Undo ⌘Z** (new registry action `undo`, enabled when the operation log has an undoable last operation: today only a restack has an undo, so the button shows while a restack record exists), Git log button (red as today), watching-limited notice. Path and counts leave. `⌘Z` is bound only in the window's list context, so text fields keep their own undo.
4. Palette ranking: enabled rows before disabled ones in each section; disabled rows collapse under a "N unavailable" line that expands with Enter or a click.

## Decisions made here (least change)

- No general undo engine: Undo covers what the operation log can undo today (restack). Commit undo would change Git behavior; it is left for Gustavo.
- The sidebar stays methods on `StatusView` (one renderer file plus one pure model) rather than a separate GPUI entity: moving the state out would touch every page.
- Inbox is a placeholder screen until slice 6 so the page list is final now.

## Tasks

| # | Who | Task |
|---|---|---|
| 1 | Haiku | Palette: enabled before disabled, disabled collapsed under an expandable line. `palette.rs`, `palette_view.rs`, tests. |
| 2 | Opus | `sidebar_nav.rs`, `context_actions.rs`, `sync_state.rs` pure models with tests. |
| 3 | Opus | Sidebar renderer, top bar zones, status bar, Undo action, background fetch, ⌘Z binding. |
| 4 | Haiku | Docs: first-steps (window tour), command-palette, keyboard-shortcuts (⌘Z), branches, stacks, workspaces pages. |
| 5 | Opus | Review, `make check`, commit, push. |

## Acceptance

- Every page reachable before is reachable: Working Copy, History, Code, Search (Code page / ⇧⌘F / palette), Threads, Labels (Agents / palette), Local review, Workflows, Stashes, Reflog, Conflicts, Memories, Workspaces (palette).
- Every documented shortcut still bound (keyboard-shortcuts.md rows marked Implemented).
- `make check` green.

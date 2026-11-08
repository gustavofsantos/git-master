# UI redesign: summary

Slices 1 to 4 of the brief are done, checked with `make check` and pushed to `origin/develop`. Slices 5 (topics and single instance) and 6 (inbox) are not started. Nothing was seen in a window: no display was used, and `scripts/ui-smoke.sh` was not run.

## Branch

The brief asked for a `ui/redesign` branch. AGENTS.md forbids feature branches and says it wins on conflict, so every commit is on `develop`. Another session pushed to `develop` at the same time (memory work, D202 to D205); my commits were rebased onto it, and my decisions are D206 to D209.

## Per slice

### Slice 1: shell (D206, `slice-1-plan.md`)

- Sidebar: pages Inbox, Changes, Review, History, Code, Agents with badges; STACK (stacks of the repository, loops at the end, drag to set parent); BRANCHES · repo (By name only, the By stack / By name switch is gone); WORKTREES; WORKSPACES (kept as a fold until slice 5); footer Stashes, Reflog, Workflows, Settings (planned, disabled).
- Top bar: repository over "on <branch> ▾"; the selection with up to three ranked actions (the selection's context menu without disabled, destructive or ask-first items) and "⋯ ⌘K", which opens the palette scoped to the selection; one sync button whose label is the state (Up to date, Push ↑n, Pull ↓n, Pull ↓n ↑m, Publish) with a menu (Fetch, Pull, Push with lease); "Restack N behind" only while the stack of HEAD is stale; a background fetch every 10 minutes that shows nothing.
- Status bar: path and counts removed; **Undo restack ⌘Z** (new registry action Undo Last Operation, bound in the list context only).
- Palette: enabled rows first, disabled rows collapsed under "N unavailable — show".
- Inbox is a placeholder page until slice 6.
- Mine: the pure models (`sidebar_nav`, `context_actions`, `sync_state`), the sidebar, top bar, status bar, undo and background fetch. Delegated (Haiku): palette ranking and scope, the docs. Reviewed and corrected: removed `allow(dead_code)`s, fixed the STACK docs, kept loops in the STACK section.

### Slice 2: Changes (D207)

- Composer above the file list; resize strip on its bottom edge.
- Files grouped under their parent folder; a box per file (checked, mixed, unchecked) and a tri-state box per folder (new Stage Folder / Unstage Folder).
- One-row diff header; the half-staged pair banner has **Select the pair** (also ends visual mode, which would split the pair again).
- Mine: `change_tree`, folder actions, list rendering, composer move. Delegated: diff header and pair banner, docs.
- `git add -p` + commit stays replaceable; staging invariant tests pass.

### Slice 3: Code and Agents (D208)

- Code: Files | Search tabs. Agents: Threads | Memories | Labels tabs.
- Thread list grouped by UTC day; a row's agent chip filters to that agent (Agent: x chip clears).
- A test keeps every thread action reachable from a thread menu or the palette.
- Mine: day grouping, agent filter, Labels tab, the reachability test. Delegated: Code tabs, docs.

### Slice 4: Review (D209)

- Left: Changes, Gates and Reviewers (planned) cards over the conversations. Middle: continuous diff of every changed file with **Viewed** (clears when the file's change changes; folds the file). Right: the selected conversation with replies and Before/After.
- Replies, handled, resolve and reopen already existed (D198, D200); the MCP loop is unchanged and its tests pass.
- Mine, all of it (the session was running short).

## Not done, and why

- Slice 5 (topics, storage, single-instance IPC, ⌘1–9, Add to topic, workspace action) and slice 6 (Inbox items): not started, the session ran out of time. Slice 5 is the largest: it needs a decision on how a second `gitmaster PATH` reaches the running instance (the brief says "the existing socket"; the app's socket today serves MCP) and on one engine per checkout inside one process.
- Amend in the composer, and agent-edit marks on changed files (slice 2): new Git behavior or a new engine query.
- Turn outline and collapsed turn ranges in the thread reader; model name on a thread row (slice 3).
- Goal text (needs topics), gate bypass (planned reviewer gates), inline conversations in the continuous diff, persisted Viewed marks (slice 4).
- General undo: only a restack can be undone; undoing a commit would add Git behavior.
- A pre-existing random failure of `gm-core` proptest `grep::tests::a_shaped_line_is_valid_text_with_marks_inside_it` (Unicode case folding in the search highlight, e.g. query "up"). The saved regression file was removed so the suite runs as it did at HEAD; the bug itself is not fixed.

## Open questions for Gustavo

1. Confirm D206 to D209 (all Proposed).
2. Undo: should ⌘Z also undo a commit (`reset --soft`), or stay on restack only?
3. Amend: add it to the composer (a new Git operation for the app)?
4. Labels: keep them as a tab of Agents (current), or drop the page and manage labels only from chips?
5. Viewed marks: keep them per window, or store them (in `.git/gitmaster/` or the app data directory)?
6. Slice 5: which socket and protocol should `gitmaster PATH` use to hand a path to a running instance, and should one process host one engine per checkout?
7. The brief's three open questions (name "topic"; composer on top, done; Branches fold follows the selected member) still stand.

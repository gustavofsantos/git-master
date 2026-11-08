# UI redesign: summary

Slices 1, 2, 3, 4 and 6 of the brief are done, checked with `make check` and pushed to `origin/develop`. Slice 5 (topics and single instance) was dropped by Gustavo on 2026-11-08 (D210); slice 6 was built for the current repository. Gustavo confirmed D206 to D209. Nothing was seen in a window: no display was used, and `scripts/ui-smoke.sh` was not run.

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

### Slice 6: Inbox (D210)

- Needs you (conflicts, workflow approvals, agent questions), Returned by agents (handled comments, proposed memories), Waiting on agents (your open comments), Fell behind (branch behind upstream, stale or orphaned stack branches), for the current repository.
- Derived on every frame from existing state, never stored, so each item clears by itself; tested.
- Selecting an item fills the top bar's actions; Enter or a double-click runs the primary one; the sidebar badge counts Needs you and Returned by agents.
- Mine, all of it.

## Fixed along the way

- The text search highlight (a random failure of the `gm-core` property test `a_shaped_line_is_valid_text_with_marks_inside_it`): a long line's window could cut a match in half, and Unicode case folding marked characters `git grep -i` does not match (the Kelvin sign for `k`). Both fixed; marks are whole matches with ASCII case folding, as in Git.

## Not done, and why

- Slice 5 (topics, storage, single-instance IPC, ⌘1–9, Add to topic, workspace action): dropped by Gustavo. With it go the topic Goal on Review, grouping Changes by member, and the Inbox's All topics / this topic switch.
- Blocked gates in the Inbox: a block leaves no state after its notice.
- Amend in the composer, and agent-edit marks on changed files (slice 2): new Git behavior or a new engine query.
- Turn outline and collapsed turn ranges in the thread reader; model name on a thread row (slice 3).
- Goal text (needs topics), gate bypass (planned reviewer gates), inline conversations in the continuous diff, persisted Viewed marks (slice 4).
- General undo: only a restack can be undone; undoing a commit would add Git behavior.

## Settled with Gustavo

All decisions of the redesign are Decided: D206 to D210, and D211 for the questions that were open here. Undo stays on restack, the composer has no Amend, Labels stay a tab of Agents, and Viewed marks last with the window. Slice 5 is dropped, so the brief's questions on the name "topic" and on the Branches fold no longer apply; the composer is on top.

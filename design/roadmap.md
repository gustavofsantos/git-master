# Roadmap

The order of the work. Status marks: **(done)** in the code and verified; **(to do)** not started; no mark = future slice. The order prioritizes reaching something the author uses daily and that already shows a differentiator. The documentation in docs/ is the desired state; this file orders the work toward it, and `make docs-status` lists what the docs still mark as planned.

Last updated: 2026-10-29 (slice 6t, threads, planned and refined; before that, slice 3c, N4 syntax highlighting built; before that, slice 1f, move a file: M5 to M7 built, M4 not done). Initial proposal, to be iterated on.

## Order criteria

1. Technical risk first (spikes), before building on top of it.
2. Each slice is vertical and usable on its own.
3. The first differentiator (hooks) appears early. Stacks and MCP come after the base of Working Copy and history.
4. If the most urgent differentiator is stacking, slice 5 can move forward. The order is the author's decision.
5. Workflows come early, because they make stack policies and LLM-written commit messages configuration, not code. Confirmed by the author on 2026-10-19 (Q20).

## Slice 0: spikes and skeleton (done)

Goal: validate what could invalidate the design.

**Scope**

- Spike A (dropped by the author on 2026-10-16): virtualized list in GPUI with 100,000 rows.
- Spike B (done): editable text in GPUI, for the resolver's result panel. Libraries and licenses evaluated. Verdict in Q7.
- Spike C (done): in-memory rebase with libgit2 (coverage) and `git merge-tree --write-tree`, compared with gitoxide. Verdict in Q6.
- Spike D (done, no fsmonitor verdict): status in a massive repository through a subprocess with fsmonitor. Findings in `FINDINGS.md` of the d-status spike (git history, 7932c61).
- Skeleton (done): Cargo workspace with `gm-schema`, `gm-core`, `gm-ui` and `gm-mcp`; GPUI window; open a repository; minimal operation log; base property tests.

**Done when:** the app opens a large repository and lists its changes without freezing. The spike verdicts are recorded in design/decisions.md (Q6, Q7).

**Known gaps:**

- fsmonitor could not be measured on this Git build, so Spike D has no verdict.

## Slice 1: Working Copy and commit (done)

**Scope**

- File list with tri-state checkbox, filter and grouping by directory (done).
- Unified diff (histogram), stage per file, hunk and line, with a warning for a −/+ pair (done).
- Commit composer and commit (done).
- Keyboard shortcuts: navigation, stage, filter, refresh and focus (done).

**Done when:** it replaces `git add -p` plus `git commit` in daily use. The code is complete; the author's daily use is the test.

**Known gaps:** none recorded.

## Slice 1b: the basics still missing, before the differentiators (done)

Review of 2026-10-16. With the Working Copy ready, three things are still missing before the app can replace a Git client day to day. They come before slice 2, in this order: command log, branch switch, project switch. The log comes first so the other two operations show up in it.

**Scope**

1. **Git command log** (done).
   - Logs only side-effecting operations (D28): stage, unstage, commit and checkout. Reads are not logged.
   - Each `git` run is a structured entry: program, directory, arguments, exit code, duration, stdout and stderr. The last 1,000 are kept in memory, never in the repository.
   - A status bar button opens a read-only panel: newest at the bottom, failures highlighted, output expandable.
   - Stage, unstage and commit now run the `git` executable (D26 revised), so commit runs Git's native hooks (D18). Per-line staging (D24) also uses Git commands.
   - Restack changes refs; logging it belongs to slice 5. This is not the engine's operation log (D14, design/architecture.md), which records what was requested and by whom.
2. **Switch branch** (done). `ListBranches` and `Checkout { branch }` commands. A selector in the status bar or header lists local and remote branches with a filter. A remote branch with no local copy creates a tracking branch. If Git refuses a checkout on a dirty working copy, its error shows in the panel and a notification appears. There is no hidden autostash (Q2, Q11).
3. **Switch project** (done). Native folder picker and a recent list, stored in the app's data directory, never in the repository (docs/concepts/principles.md). Switching explicitly discards the current state (selection, diff, commit draft). One repository per window in v1.
4. **Welcome screen** (done). Shown when no project is open: first run, no command-line argument, current directory not a repository, and no recent that still exists. It has the logo, one sentence about the app, an "Open a repository" action, the recent list (missing entries drop out) and the shortcut hint. Opening a repository switches the screen with no restart. A folder that is not a repository shows the error on the screen. Repository-only actions are hidden, not disabled. Strings are in English; the icon comes from `assets/icons/`, in the theme colors (design/visual-identity.md). There is no keyboard shortcut to open.
5. **Design sidebar** (done, with placeholders). Left column, as in the canvas: "Workspace" (Working Copy with a file count; History, Stashes, Local review and Reflog are placeholders), a "By stack" / "By name" switch, and the branch tree.
   - "By name" is real: branches grouped by prefix. Clicking one checks it out. The current branch gets the HEAD badge.
   - "By stack" shows "No stacks yet" and lists all local branches under "No stack" until slice 4.
   - Top bar: the repository name, with `branch · path` below. Fetch, Pull, Push with lease and Restack sit on the right, disabled with "Not available yet" until the engine has them. Fetch, Pull and Push with lease work now (slice 1d). Restack has no counter until the stack model exists. No search in the top bar, by decision of this slice.

**Done when:** switching project and branch works without leaving the app. Every Git command run can be inspected in the panel. Opening the app with no project shows a screen that explains what to do, not an empty window.

**Known gaps:**

- The native folder picker was tested only up to opening it, because the dialog needs a display.
- Missing from the design: stack indicators (`restack N`, `↑N`, amber dot, PR), the scoped status bar, fsmonitor, the operation log and MCP.

## Slice 1c: simple Working Copy, context menus and command palette (done, except the commit and stash menus)

Requested by the author on 2026-10-16, from two new canvas boards (branch context menu; command palette). It comes before slice 2 gets an interface, because slice 2's composer and chips live on this simplified screen.

**Scope**

1. **Simplified Working Copy** (done). `DiscardHunk` was added. Discard on the Staged side is refused.
   - Header "Working Copy" with the count ("6 changed"). A flat list, one row per file: state letter (M, A, D, ?) and full path. No filter, count or per-row checkbox.
   - Composer at the bottom: message box and a "Commit on <branch>" button.
   - Diff header with the path and a `+5 −1` summary. No extra button bar.
   - Stage and unstage work as in the reference clients (decided by the author on 2026-10-16):
     - A "Staged | Unstaged" selector at the top of the diff picks the side to view.
     - A summary under the header ("2 chunks, 3 insertions, 4 deletions").
     - Each hunk header has "Discard Chunk" (asks for confirmation) and "Stage Chunk" ("Unstage Chunk" on the staged side). Per-line staging (D24) stays, through the gutter.
     - Space and the file's menu stage or unstage the whole file. A file with anything staged gets a discreet mark. "Read" stays a separate axis (D23).
2. **Action registry** (done). Prerequisite for items 3 and 4. Per-line staging (D24) stays outside it. Each action has a stable id, a title, a shortcut, typed arguments (branch, file) and an "enabled now?" check that gives the reason when it is not. Keys, menus and the palette read the same registry. Actions the engine does not support are disabled with "not available yet". Running an action dispatches the same `CommandKind` as before, with `Actor::Human`.
3. **Context menus** (done for branch and file; commit and stash still to do). Right-click on a sidebar branch or a file in the list. `shift-F10` opens the selected file's menu; `Esc` closes it. Items come from the registry, with separators per group. A disabled item shows its reason in place of the shortcut. Destructive items are red and ask for confirmation.
   - Branch menu: Checkout, New child branch, Create worktree, Restack this stack, Push with lease, Copy branch name, Rename, Delete. Only Checkout, Copy branch name and Create worktree run.
   - File menu: Stage file, Unstage file, Discard changes, Copy file path, Reveal in file manager. Discard changes is disabled, because the engine only discards hunks (`DiscardHunk`).
   - Design shortcuts: Checkout Enter, New child branch ⌘B, Create worktree ⌥⌘W, Restack ⇧⌘R, Push with lease ⇧⌘P, Copy branch name ⌥⌘C, Rename F2, Delete ⌫. Only ⌥⌘C runs, on the current branch.
4. **Command palette** (done; `⌘K` on macOS not yet tested there). `Ctrl+K` opens it, with fuzzy search over the sections "Actions", "Worktrees" and "Branches". A section appears only when it has results; branches appear only after typing. Matches are highlighted, with a result count. Arrows move, Enter runs, Tab picks the action's argument (branch or file), and Esc backs out or closes. The list is virtualized, and disabled actions show their reason. It runs through the same path as keys and menus (`run_action`, `Actor::Human`). Sorting runs off the UI thread, and only the newest query's results show.
5. **Worktrees** (decided 2026-10-17, D31; in sub-slices). Patterns and out-of-scope items are in D31 (design/decisions.md). Each sub-slice enables only its own actions; the rest show "Not available yet".
   - **W1, listing** (done). Lists each worktree's path, branch or detached HEAD, lock state, prunable and bare status. The main checkout is first. The list comes with every status refresh and with `ListWorktrees`.
   - **W2, sidebar section** (done). A "Worktrees" section under the branch tree, with "here" and "missing" badges and a color from a hash of the name. No agent badge (Q12). The worktree registry is watched, so a worktree added or removed from the CLI updates the section.
   - **W3, create** (done). A dialog with three modes: existing branch, new branch from a commit, detached HEAD. The folder defaults to `../<repo>-<branch>`. It must not exist; that refusal is ours, with Git's wording. Git's error shows in the dialog. The command runs in the engine, off the UI thread.
   - **W4, open** (done). Double-click opens the worktree in its own window, which is another process of this program started in that folder. A missing folder warns in the status bar. When a branch is used by another worktree, the dialog offers "Open that worktree".
   - **W5, remove with checks** (done). The row menu has Open, Reveal, Copy path and Remove worktree (red). The dialog runs `InspectWorktree`, a read-only check of what removal would lose or refuse.
     - Blocks that force can override: dirty (staged, unstaged or untracked); locked (with Git's reason); commits that no other local or remote branch has, or that are ahead of upstream (the larger count).
     - Blocks that force cannot override: main checkout, current window, missing folder.
     - With force, the user ticks a confirmation box instead of typing the name (changed 2026-10-17, see D31). The engine re-checks and runs `git worktree remove` (`--force`, or twice for a locked worktree). Never `rm -rf`. Only `Actor::Human`.
   - **W6, lock and clean** (done). The row menu has Lock or Unlock, and Prune Worktrees on a missing-folder row. Cleaning runs Git's dry run and shows each record with Git's reason. The prune runs only on that same list and refuses if it changed. Folders are not touched, and locked worktrees are skipped, as Git does. The sidebar shows a "locked" badge.
   - **W7, agent path** (`Actor::Agent`): not in this slice. It goes to slice 7 (MCP), per Q12.
   - **W8, branch menu variant** (done). A branch checked out in a linked worktree gets: Open Worktree, Reveal Worktree in File Manager, Copy Worktree Path, New Child Branch, Restack This Stack and Remove Worktree (red), with the rules of W4 to W6. The main checkout keeps the normal menu.
6. **Automatic refresh, no refresh button** (done; design confirmed by the author on 2026-10-16). The button left the top bar. "Refresh" stays in the registry and on `r`.
   - Premise: the screen never goes stale, and the user never has to ask.
   - Triggers: the window regains focus; changes in `.git` (HEAD, index, refs, `packed-refs`, `MERGE_HEAD` and similar), watched non-recursively; changes in the working tree, watched recursively, skipping what `.gitignore` ignores and `.git`.
   - A debounce of about 150 ms. Never two status calls in flight. The app's own writes do not trigger a second refresh.
   - If the system refuses the watch (inotify limit) or the tree is too large, the app refreshes on focus only, and the status bar says so once. Do not enable `core.fsmonitor` or `untrackedCache` without consent.
   - It never writes to the repository. Status runs off the UI thread, and the indicator shows only past about 300 ms.

**Done when:** the Working Copy is in the simplified design. Every action on screen exists in the registry and is reachable by key, context menu and palette. A disabled action says why.

**Known gaps:**

- Items 1 and 2: none recorded.
- Item 3:
  - Commit and stash have no menus yet.
  - The branch menu does not open from the keyboard and does not close with `Esc`. Enter on checkout has no keyboard target. The menu key uses the pointer position, not the row's.
  - The Discard confirmation is not exercised while the item is disabled. Reveal is not in the smoke test, and it offers the path even when the file is gone.
- Item 4:
  - Chunk actions show as disabled with "Pick a chunk in the diff first.", because they run from the diff panel.
  - The file list updates only on the next status event.
  - The smoke test covers only the keyboard. Highlighting is greedy, so with repeated letters it can mark a less obvious one.
- Item 5:
  - W1: the newline-separated fallback is covered only by parser tests. A worktree removed by hand shows as `prunable` only on the next refresh.
  - W2: the section does not scroll, so six rows show. Lock and unlock from the CLI appear on the next refresh, and so does a folder removed by hand. Bare worktrees do not appear. The color comes from the hash; the lane rule of D25 does not exist yet. "here" may miss a symlinked path.
  - W3: `gitmaster.worktree.root` is not read yet, so the suggested folder is always a sibling of the repository. The new-branch and detached modes have no smoke coverage. Sparse checkout, stack parent and the `post-worktree-add` hook are out of scope (not in D31).
  - W4: no smoke coverage; checked by hand. The new window is a separate process, because each window has its own engine. No palette action opens a worktree.
  - W5: "also delete the branch" and "undo" are not built. Open review comments and ignored-file sizes are not computed, so ignored files are deleted with no size warning. The current-window check resolves symlinks, but the registry compares text. The dialog has one field and no branch list.
  - W6: Lock sends an empty reason. The clean dialog does not close with `Escape`. "Prune Worktrees" in the palette opens the dialog with no target. The list shows the record name (`repo-gone`), not the path. Cleaning does not count the locked records Git skipped.
  - W8: there is no "Open in terminal" (no terminal integration). No registry shortcuts exist, so ⇧⌘C and ⌘⌫ do not show. The top bar's branch menu lacks the variant, and the palette does not list it.
- Item 6:
  - External edits within 150 ms after an app write may be missed. One extra refresh happens on open.
  - macOS (FSEvents) and trees near the 5,000-directory cap are untested.

## Slice 1d: the basics still missing from daily use (done, with gaps)

Requested by the author on 2026-10-19. Decisions D54 to D59 in design/decisions.md. Nothing here was seen in a window, because there was no display. The UI is covered by unit tests of the pure models, and the engine by tests with real Git. `scripts/ui-smoke.sh` did not run (`xdotool` and `scrot` are missing), and no smoke scenario was written.

**Scope**

1. **Copy from the commit panel** (done, D58).
2. **Create and delete branch** (done, D55). Local branches only.
3. **Worktrees** (done, D59). The branch field suggests local branches (up to 8). Branches already used in another worktree are not marked. The palette lists worktrees to remove, lock and unlock. The sidebar section has "+" and "×".
4. **Resizable sidebar** (done, D57).
5. **Delete merged branches** (done, D56), from the palette ("Delete Merged Branches…").
6. **Fetch, Pull and Push with lease** (done, D54).

**Done when:** the author's five complaints work in the real window: copy; create and delete branch and worktree; resize the sidebar; delete merged branches; Fetch, Pull and Push. Still to check by hand.

**Known gaps:**

- Item 1: file paths are not selectable or copyable. Selection is per area, so selecting in one field does not clear another's highlight. Esc in a focused area sends `Cancel` to the `TextArea` (not checked in a window). Drag, double- and triple-click selection and Ctrl/Cmd+C are untested, because the UI crate has no GPUI test harness.
- Item 2: the ⌫ registry action is not bound to a key. "is not fully merged" is recognized by text, with `LC_ALL=C`. The delete dialog is the only confirmation.
- Item 3: "New Worktree…" has no shortcut. Only the branch field suggests, not the commit field. Lock still sends an empty reason. The section still cuts off at six rows.
- Item 4: windows narrower than about 500 px still zero the main area, because there is no minimum window size.
- Item 5: squash and rebase merges are not detected. The preview shows at most 15 lines and says how many are left, since the dialog does not scroll. If re-reading status fails after a delete, the report is lost.
- Item 6: no timeout or cancel, so a stuck fetch holds the remote slot until Git finishes. `GIT_TERMINAL_PROMPT=0` does not stop the `ssh` password prompt when the app is started from a terminal. A status read that fails after the operation is silently ignored. No smoke scenario covers the buttons.

## Slice 1e: the basics still missing (done, with gaps)

The author's request of 2026-10-22, before H5: close what a Git client needs day to day and is still a placeholder. Decisions D93 to D97 in `design/decisions.md`. Order: B1, B2, B3, B4; each sub-slice is usable alone and enables only its own actions in the registry. No smoke scenario in this slice (D88).

- **B1, discard a file's changes** (done 2026-10-22, D93; only the Discard Changes path of a modified file was seen in a window). Enables `file.discard` ("Discard Changes"), disabled until now.
  - A tracked file goes back to the index content (`git restore --worktree`); what is staged stays. An untracked file is deleted. A second action, "Discard All Changes", also discards what is staged (`git restore --source=HEAD --staged --worktree`); for a new staged file it removes it from the index and the disk.
  - Command `DiscardFile { path, include_staged }` in `gm-schema`; `GitPort::discard_file`; human only; recorded in the command log (D28). No safety net: the loss is final, so the confirmation lists what goes away (D87).
  - Done when: real-Git tests (modified, deleted, new untracked, new staged, staged plus unstaged, binary, path with spaces and Unicode, a path outside the working copy refused, agent refused); rule and plan in the registry; confirmation dialog with the summary; file menu and palette.
  - Known gaps: not seen in a window: Cancel, Escape, the Discard All Changes mode, Git's error in the dialog and the palette (the rule and the summary have unit tests); no smoke scenario (D88); no safety net (D93); a `git add -N` (intent-to-add) file is removed from the index and the disk, not restored; the dialog shows an untracked folder by its status path (`dir/`); "Discard Changes" on a file that is only staged is disabled (use Discard All Changes).
- **B2, Stashes** (done 2026-10-22, D94; seen in a window in the light theme, not the dark). The sidebar's "Stashes" page.
  - List (`stash@{n}`, message, source branch, time), detail with files and a read-only diff (reuses the history panel, H3 and H4), create (with a message; option to include untracked files), apply, pop and drop, with context menu and palette. Applying with a conflict shows Git's error and does not try to resolve it (Q11).
  - Done when: real-Git tests for each action, including `-u`, the index, a conflict on apply and a numbered stash after a drop; the list refreshes by itself through the watcher (`refs/stash`).
  - Known gaps: the untracked files of a stash (third parent) are not in the file list or the diffs; the check against an old list and the execution are not atomic (a tiny window); the sidebar count does not update while the page is closed; Cancel, Escape and Enter in the dialogs, apply and pop from the context menu, the pop conflict that keeps the stash and the dark theme were not seen in a window; no smoke scenario (D88); apply runs without `--index` (the stash's staged state comes back unstaged); the palette lists stashes only after the page was opened once; the grep test `git_is_killed_when_the_result_is_full_not_waited_for` is flaky inside the full suite (passes alone).
- **B3, Reflog** (done 2026-10-22, D95; seen in a window in both themes). The sidebar's "Reflog" page.
  - Paged list of HEAD (`git reflog`) with index, action (checkout, commit, reset, rebase...), short hash and time; detail and diff through the history panel. Actions: create a branch from the entry and check it out (detached). No `reset --hard` on this screen.
  - Done when: real-Git tests (commit, checkout, reset, rebase, amend); cursor paging like H2.
  - Known gaps: a refresh on a status event re-reads only the first page (200), so later pages come back when you scroll; the `time` of a commit entry is the commit date, not the write date (that is how Git records it); the palette lists "Check Out Commit (Detached)" and "New Branch From Commit…" disabled, because there is no commit picker; the success notice of the detached checkout, the hint of the disabled buttons, Escape and Enter in the error dialog and the palette were not seen in a window; only HEAD's reflog (not other branches'); no smoke scenario (D88); a commit subject with U+001F is cut on read.
- **B4, local review without MCP** (done 2026-10-22, D96 and D97; Q3 resolved: `.git/gitmaster/review.db`; only the Working Copy diff in the first version). Steps: B4a, anchors and storage; B4b, snapshots and engine commands; B4c, comments in the Working Copy diff; B4d, the Local review page and Before/After.
  - Inline comments in the diff, states `open` and `resolved` (the agent's `handled` comes with slice 7), anchors that never guess (exact text once; otherwise `outdated`), a working tree snapshot at the moment of the comment (D97), a Before/After selector. The MCP tools stay for slice 7.
  - Done when: anchor and states in a pure module with property tests; real-Git tests of the snapshot (touching neither public refs nor the working tree).
  - Known gaps: the Before/After selector marks no line: with the anchor found exactly once both sides have the same text, and an outdated comment has no "after" (D97 forbids re-anchoring by approximation); what really changed comes with the agent's "after" snapshot in slice 7; only the Working Copy diff (the commit diff comes later), commenting only on the unstaged side, and on the staged side the cards anchor by the index's line numbers; the margin marker is a dot (there is no balloon icon in `assets/icons`); the palette lists the comment actions disabled (there is no comment picker); a snapshot that fails is silent (the comment is left without a snapshot); resolving as a human takes no "after" snapshot; the cost of the snapshot's `git add -A` was not measured in release and a split index (`core.splitIndex`) was not checked; not seen in a window: the "Comments outside the changed lines" strip, Escape in the composer, the engine's refusal in the composer, saving an edit from the page, the split layout on the page and the keyboard-only paths. The composer returns focus to the diff when it closes; the other dialogs (discard file, stash, branch, worktree, merge, workflow, message) still return focus to the file list, which only matters when they are opened from the palette with the focus in the diff.

## Slice 1f: move a file to a branch or a worktree (M1 to M3 and M5 to M7 built; M4 not done; nothing seen in a window)

Requested by the author on 2026-10-23. Decision D109 in `design/decisions.md`; the undo that this slice prepares is Q24. The behavior is in `docs/guide/working-copy.md` ("Move a file to a branch or a worktree"), marked `Planned`.

**Scope**

1. **M1, engine core** (done 2026-10-23, with gaps). `GitPort` methods: snapshot of given paths (generalises `snapshot_worktree`), tree merge (`merge-tree --write-tree`), `commit-tree`, `update-ref` with the old value, a per-file three-way merge (`merge-file -p`) and an atomic write into another worktree, discard of paths with the check against the snapshot (D93), the safety ref `refs/gitmaster/move/<id>`, and the Git version read (floor 2.38, only for this feature). Command `PreviewMoveChanges` (read) with its blockers. Pure module `move_changes.rs` for the rules (blockers, names, step order). Done when: real-Git tests (new, modified, deleted, renamed, binary, untracked; clean merge and conflict; branch checked out elsewhere; a race on `update-ref`; a target file that changed after the read; a failure injected after the write proves nothing is lost and the ref stays).
   Known gaps of M1: `PreviewMoveChanges` is wired in the engine, but `MoveChanges` and the safety ref `refs/gitmaster/move/<id>` come with M2 and M3; the per-file write into another worktree (atomic rename) is M2; an unborn HEAD (no commit yet), a directory at the target path or a file where a parent folder is needed make the preview fail as an operation error, not as a blocker; each preview writes an unreferenced snapshot commit (cheap, collected by `git gc`; its cost in a large repository was not measured in release); `safe_relative` refuses any path with a backslash; the move snapshot is not in the command log, the review snapshot still is; `merge_trees` takes any revision and resolves it to a commit first. Nothing was seen in a window and no smoke scenario ran.
2. **M2, move to an existing worktree** (done 2026-10-23; `MoveChanges`, logged, human only), including a dirty target. Known gaps: the equal-oid path of the safety ref (two snapshots of identical content in the same second) is not forced by a test; a failure in the cleanup of the source after the target holds the change is reported as an error that says so; nothing was seen in a window.
3. **M3, move to an existing or new branch** (done 2026-10-23), with the commit message. Known gaps: a new untracked folder is one entry in the status, so it is refused as a folder (`IsFolder`) and its files move one at a time; the commit runs no Git hooks (D109).
4. **M4, spike of drag in GPUI, measured in a release build.** It can run in parallel with M1 to M3. It checks auto-scroll of the sidebar during a drag, dropping on a virtualized row, and the cost of the drag ghost. If it fails the bar of "smooth and fast", the gesture degrades to the menu and palette, it does not ship slow. **Not done:** no spike was measured. M5 and M6 were built on the GPUI API without it, so the release-build check is still owed before the gesture ships.
5. **M5, drag and drop UI** (built 2026-10-23, with gaps). Pure model in `crates/gm-ui/src/move_file.rs` (answers, hover check, popover, notice, drag poll), a drag source on each Working Copy file row, drop zones on branch rows, worktree rows and the Local header, the 50 ms drag poll that asks Git after the 150 ms rest and ends a drag gpui no longer carries, the message popover, and the status bar notice with **Open Worktree**. Known gaps: nothing seen in a window and the drag was not tried by hand, so the threshold, the hover timing and the styles are untested; the hint is in the status bar and the ghost shows only the file name; the popover opens beside the sidebar, not on its row; Esc ends the app's drag state but gpui keeps its own drag until the mouse is released (the drop then does nothing); the preview has no cancel in the engine, so a superseded answer runs to its end and is dropped; a drop from the palette has no preview yet, so it goes ahead while checking; the Git 2.38 floor is known only after a preview reports it; the **Open Worktree** button is refused with "This worktree is not listed." until the worktree list holds the new worktree.
6. **M6, creation targets** (built 2026-10-23, with gaps): drop on the Worktrees header (its `+` is inside it) opens New Worktree in the new-branch mode and moves the file into the worktree it creates; drop on the Local header or a prefix group opens the branch popover with the name. Known gaps: the chained move uses the folder path as typed, so a relative path fails with `WorktreeNotFound`; the Branches header is a target only in the **By name** view, since **By stack** has no Local header; a worktree created from the dialog is moved to at once, with no confirmation of the dialog.
7. **M7, palette, file menu and keyboard** (built 2026-10-23, with gaps): `Move File to Branch…` and `Move File to Worktree…` in the file menu after View File and in the palette (the file is the selected row; Tab picks the target), with no default shortcut. The menu item opens the palette's target step. Known gaps: the actions are not refused for an old Git until a preview says so; the markers of `docs/` were updated for what the code paths cover, and the parts not done keep a `Planned` marker (the hint beside the row, the popover on its row, the By stack header); `make check` was run for the code and docs, not for a window.

**Out of this slice:** Undo (Q24), multiple selection, moving by hunk or the staged side only, carrying the staged state, agents and workflows (Q12).

## Slice 2: `prepare-commit` hook (done)

Backend and UI are done. Reviewed on 2026-10-16 (D29, D30): only what Git lacks becomes a GitMaster hook. `pre-commit` and the other native hooks stay with Git (D18, D29). It moves to a workflow in WF3 (D75).

- Versioned JSON contract in `gm-schema`: input is the current message, staged diff and repo; output is the message (D30). Hooks run with the login shell's PATH.
- Configured as `[gitmaster "prepare-commit.<name>"]`. A repo-level hook runs only after an approval kept outside the repo, tied to the command's hash (D17).
- Async run: the message header shows the run state and has "Cancel". "Suggest message" starts it (`commit.suggest_message`).
- The answer fills the message only if HEAD, the staged diff and the text are all unchanged. Otherwise it is offered with "Replace" or "Discard".
- A repo-level hook needs "Approve" or "Deny" first. Deny hides the panel for this session only.
- Each finished step is logged in "Git commands" as `hook <name>: <command>`, with exit code, duration and a one-line result. Input, output and diff are never logged (D28).

Done when: a script the author writes can generate the commit message without freezing the UI, and native `pre-commit` still runs as before.

Known gaps:

- The log does not store the command hash, so a cancelled or timed-out step shows as a failure.
- A failed step shows only its exit code. The hook's first error line is on the chip, not in the log.

## Slice 3: history and branches (by-name view) (in progress: H1 to H5 done, with H4b-A, H4b-B and H4c)

**Next step (author's decision, 2026-10-21; updated at the end of the 2026-10-22 session):** Slice 3c (code navigation, see the Slice 3c section) comes now. Spikes S-A, S-B and S-C and sub-slices N1 to N4 are done. **N5 (symbols and the single box)** is next. H6, H7 and H8 are postponed. The replacement of workflows with scripts is done (S-D, WS0 to WS2); WF3 has not started. H5 is done. See "How to continue" below.

### How to continue (end of the 2026-10-25 session)

**Where the code is.** Everything is on the working branch of the session. Before this session: H5 (lane graph), spikes S-A to S-C, N1 to N3. This session added N4 (syntax highlighting, D111). The agent did not see any of it in a window, because the container has no display. `make lint`, `make docs` and `cargo test --workspace` are the checks; there are no new smoke scenarios (D88).

**Next work: N5, symbols and the single box.** It reuses the tree-sitter of N4 and the proposals of D92: definitions by tree walk where the language allows it (Rust, Python, Go) and by query for TypeScript and JavaScript, an index in `.git/gitmaster/symbols.db` keyed by blob, language and query version, and the single search box of D50. D45 stays Proposed for the symbol index: confirm it with the author, and agree the "Done when" of N5 in `design/code-navigation.md`, before writing code.

**Owed after N4:**

- Look at the colors in a window, in both themes, on real files of each language, and tune the queries in `crates/gm-core/queries/` where a role looks wrong. A change there is a change of product: keep the tests of `syntax.rs` green and add a sample for a fix.
- Measure the binary growth and the build on macOS (`measure.sh` of the nav-b-treesitter spike, git history, 7932c61).

**Minor pending items, by value** (all details are in the known gaps of each sub-slice):

- Quick Open: the list of new files (`--others`) takes about 0.75 s on 100,000 files. A two-phase list (tracked files first, new ones after) fixes it. It is reread only when Quick Open opens.
- Search: no streaming and no progress. Clearing the box does not cancel the running Git. Measure `--threads` for `git grep` on macOS (D91).
- Try on macOS: Quick Open, Code and Search (none has been seen in a window). Run `measure.sh` and `run.sh` of S-B.
- Code: the viewer has no text selection and no line wrap. The tree has no keyboard support. "View file" from the commit waits for N6.

**Preparing a new session** (the cloud container starts clean): `rustup update stable` (`rust-toolchain.toml` asks for the newest stable; the container came with a version that is too old), `~/.cargo/bin` first on `PATH`, `apt-get update && apt-get install libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev pkg-config` (to link `gm-ui`), then `make lint` and `cargo test --workspace`. The spike corpora (`~/.cache/gm-smoke`) do not survive. `fetch-corpora.sh` of the nav-b-treesitter spike (git history, 7932c61) downloads the repositories, and `FINDINGS.md` says how to redo the rest. Files the editor reformats (`cargo fmt`) change lines: if a script's text replacement does not match, reread the file.

Order set by the author on 2026-10-16: this slice, together with the diff view (below), is the next work after finishing 1c (including automatic refresh) and the slice 2 UI. The author's decisions of 2026-10-17 are in D32 to D37 (design/decisions.md). The slice is split into sub-slices H1 to H8, each with its own done-when. Reading history uses `git log` as a subprocess, without writing a commit-graph (D32).

### H1: real commit list (done)

- HEAD's first 200 commits, flat, in 48 px rows: ref chips (HEAD, branches, tags), subject, short hash and relative time.
- HEAD with no commits shows a message instead of an empty list.
- A "Show History" action in the action registry opens the page from the sidebar. Read command, not logged (D28, D33).

Done when: the page matches real Git for linear repos, merges, root commits, HEAD with no commits, detached HEAD and Unicode subjects, and the UI smoke passes in both themes.

Known gaps:

- Ref chips cannot tell local refs from remote ones (`%D` does not say).
- The list refreshes only when the page opens (H2). No paging beyond 200, no selection, no keyboard shortcut for "Show History".
- The list uses Git's default date order, not `--topo-order` (H5). Relative time uses the author date.

### H2: paging and refresh (done)

- Pages of 200 commits, requested near the end of the list from the last listed commit (cursor by oid). Output is streamed and resumes where it stopped. If the cursor does not match, the read restarts.
- Each watcher status update rereads the top, so commits made outside the app appear.
- A click selects a row by oid. The selection survives updates.
- Reads run on their own thread. A superseded answer is dropped by `OperationId`, and its git process is killed. Nothing is written to the commit-graph or the command log.

Done when: a 5,000-commit merged repo (made with `git fast-import`) pages exactly like `git rev-list --topo-order HEAD`, with no repeats or gaps; outside commits appear with the selection kept; the UI smoke passes in both themes.

Known gaps:

- Refresh rereads only the first page.
- Every status update rereads the top, even when only the working tree changed.
- Paging has no UI test and no smoke run with 5,000 commits (the performance gate is H8).

### H3: commit detail (done)

- A 360 px panel: subject, full body, full hash, author name and email, author date, short parents, and the changed files. Each file has a status letter (A, M, D, R; a rename shows `old → new`) and + and − counts. Binary files show `bin`. Over 200 files show "and N more". No selection shows "Select a commit to see its details."; an error shows Git's own message.
- Merges compare with the first parent (`--diff-merges=first-parent`). A root commit lists every file as A.
- Reads on their own thread with the H2 model. A new selection kills the ongoing read.

Done when: real-Git tests cover add, modify, delete, rename, binary, root commit, merge, multi-paragraph body, Unicode and the file limit. Superseded reads are dropped and stopped. The UI smoke passes in both themes.

Known gaps:

- `git show` output is read fully into memory before the 200-file limit applies.
- The hash and parents wrap to two lines. The date has no relative time.
- File rows are not clickable yet (H4). The panel has no UI test; the smoke checks pixels, not text.
- Killing a running `git show` has no real-Git test. Non-UTF-8 names show replacement characters.
- The design's "Range" is outside H3's scope.

### H4: read-only commit diff (done)

- Clicking a file opens its diff in place of the list. Up and Down go to the previous and next file. Esc or "Back to commit" returns to the commit. The header shows the path, `+ −` and the chunk summary. Hunks look as in the Working Copy, without stage or chunk controls.
- Merges compare with the first parent. Root commits use `--root`. Renames show both paths. Binary shows "Binary file".
- A read-only mode of the diff pane, not a new component.
- Reads on their own thread with the H2 and H3 model. Emits no stage.

Done when: real-Git tests cover add, modify, delete, rename with edits, binary, root commit, merge, mode change, large file and non-UTF-8 content. Superseded answers are dropped and reads are stopped. The UI smoke passes in both themes, with the related scenarios green.

Known gaps:

- "Back to commit" does not stop the ongoing read.
- A non-UTF-8 file name shows replacement characters, and its diff comes out empty.
- A mode change with no content shows "No text changes in this file.", the same as an empty file.
- Up and Down skip the "and N more" files. The smoke covers add and delete only.

### H4b-A: split commit diff (done)

- A "Unified | Split" selector in the commit diff header, saved per user in `settings.json` (D39, D40). Default: Unified.
- Split shows old and new lines side by side, paired as in the D24 warning. A missing side is blank. Horizontal scroll is shared by both sides.
- Cursor and selection follow the unified lines, mapped to their split rows. Switching layout keeps the cursor line.
- No automatic switch to Unified in narrow windows (D41).

Done when: pairing tests (2 vs 3 lines, pure addition, pure removal, interleaved lines, context) pass, and a property test shows each changed line appears once per side. Real-Git tests for add, remove and rename with edits pass. The UI smoke passes in both themes, and the choice survives a restart.

Known gaps:

- No keyboard shortcut for the layout.
- Horizontal scroll is wheel-only, in fixed steps. Its limit is the widest line, not the visible width.
- The choice is saved only from the header. Its own screen is slice 9 (D38).
- Split and unified rows have separate draw functions, so a visual change to one must be checked in the other.
- No UI test for the split panel.

### H4b-B: split Working Copy diff (done, with gaps)

- The same selector and row model as the commit diff. The choice is shared by both diffs and saved (D39, D40). Stage, unstage and discard work as before. The D24 half-pair warning applies to both.
- Each split cell has its own line box. Only the cursor's cell is highlighted. Keys are the unified ones (D43): Up and Down walk the changed lines, so a −/+ pair visits each of its cells in turn.
- It also closes two H4b-A gaps. A second Down pressed while a file loads is no longer lost. The "\ No newline at end of file" marker is drawn after the line text, in both layouts.

Done when: cell and unified-line selection give the same result, and the written index matches real Git; the split-stage smoke passes in both themes (single-line stage, a second `s` on a half pair, Stage Chunk); the other staging and history scenarios stay green in both layouts.

Known gaps:

- No keyboard shortcut for the layout, and no keys to jump between the cells of a pair.
- No UI test for the split panel. The smoke checks git and pixels; the half-pair warning is checked by pixels, not text.
- The end-of-file marker is tested only as a text function. No smoke scenario draws it.
- The horizontal offset resets when the file or side changes. There is no unit test for it.
- Intra-line highlighting waits until after slice 3 (D42).

### H4c: stacked commit diff (done, with gaps)

- Clicking a file shows one continuous diff of all the commit's files, in order, like `git show`, scrolled to the clicked file. Each file has a header with its status letter, path (old → new for renames) and `+ −`. Binary, mode-change and empty files show a notice. A file over 5,000 lines shows "and N more lines, not shown.". Past 200 files, the last line reads "and N more files, not shown.". The header shows "N files" and the Unified | Split selector, which applies to all files.
- Keys (D43 to D46) are Ctrl+Shift plus a letter (⇧⌘ on macOS, D44); neither diff binds bare letters. Ctrl+Shift+N and Ctrl+Shift+B mean next and previous hunk in the Working Copy, and next and previous file in the commit view. Tests check for bare letters, repeated keys and macOS clashes with registry shortcuts (exception: Ctrl+Shift+P, Q13).

| Screen | Key (Linux and Windows; macOS) | Action |
| --- | --- | --- |
| Working Copy | Up, Down | Cursor to the previous and next changed line |
| Working Copy | Ctrl+Shift+N, Ctrl+Shift+B (⇧⌘N, ⇧⌘B) | Next and previous hunk |
| Working Copy | Space | Marks or unmarks the cursor line |
| Working Copy | Ctrl+Shift+V (⇧⌘V) | Visual selection from the cursor |
| Working Copy | Ctrl+Shift+A (⇧⌘A) | Stage or unstage the cursor's chunk (the registry label is ⇧⌘A) |
| Working Copy | Ctrl+Shift+S (⇧⌘S) | Stage the selected lines |
| Working Copy | Ctrl+Shift+I (⇧⌘I) | Unstage the selected lines |
| Working Copy | Esc | Clears the selection |
| Commit, stacked | Up, Down | Scrolls one line |
| Commit, stacked | PageUp, PageDown | Scrolls one page |
| Commit, stacked | Ctrl+Shift+N, Ctrl+Shift+P (⇧⌘N, ⇧⌘P) | Next and previous file; they wrap around at the ends (D45) |
| Commit, stacked | Esc | Back to the commit |

- The stacked view has one page of space after the last file, so any file can scroll to the top (D46).
- One read of the commit patch (`git show --format= --patch --histogram --diff-merges=first-parent -M --root`), split per file and paired with the file list by position. It runs on its own thread with the H3 and H4 model. Read command, not logged.

Done when: pure tests cover the layout, notices, line limit and steps; real-Git tests cover two files, rename, binary, mode, root commit and merge; superseded reads are cancelled and nothing is written; the UI smoke passes in both themes, with the related scenarios (split variants included) green.

Known gaps:

- The whole commit patch is in memory before the 200-file limit applies, as in H3 and H4.
- The file header scrolls away; it does not stick.
- The "current" file for next and previous is the one holding the top line.
- The 5,000-line limit hides the rest of a file, with no way to view it on this screen.
- Up and Down move one fixed line, with no acceleration.
- Non-UTF-8 names show replacement characters, as in H4.
- Scroll-to-top works only after the window has been drawn.
- Not tested on macOS, including the ⇧⌘ shortcuts (D44). Ctrl+Shift+P is still the Push With Lease label in the registry (Q13). The Working Copy file list still uses j, k and r (Q14).

### H5: lane graph (done, with gaps)

- Reads now request parents (`%P`): full oids, first parent first. The order is still `--topo-order`, page by page.
- A pure, deterministic layout built in one top-to-bottom pass that does not look ahead. Each new page extends the previous layout, and lanes still open at a page's end carry over. Each commit is one row: its node column, plus the lanes that pass, arrive, leave or shift. Drawing only paints.
- The graph column sits left of the subject, up to 8 columns. Beyond that, the row shows `+N`. Rows join seamlessly. Row height stays 48 px. HEAD's node has a ring.
- Colors: five lane accents (not state colors) and a faint trunk color. The theme contrast test covers all six in both themes, normal and selected.

Done when: `%P` matches `git rev-list --parents --topo-order`; layout property tests pass (one row per commit, every parent edge drawn, deterministic, pages extend the layout); case tests pass (linear, fork, merge, octopus, two roots, parent outside the page, repeated parent, over 8 columns); a real repo with two merges draws every edge; `make lint` and `cargo test --workspace` pass. The author used the graph on macOS on 2026-10-21 and confirmed it works.

Deviations from what was agreed: the layout runs on the UI thread, not off it, because 20,000 commits take about 45 ms in a debug build, so a 200-row page takes well under 1 ms. The timing test is 20,000 commits under 2 s in debug, not 200 rows under 5 ms in release. A lane born from a merge takes its color from the hash of the oid it expects, not the branch name, because looking ahead would recolor rows already drawn.

Known gaps:

- No smoke scenario and no UI test of the drawing (D88).
- A shifting lane changes tint where it joins. The `+N` label counts columns, not hidden lanes. No hover highlight for lanes.
- The graph column takes 14 px even in linear history.
- Local and remote names (`feature` and `origin/feature`) get different colors, because `%D` does not say which is which.
- No dashed edge and no commit owner (D25 stays Proposed). The graph shows only HEAD; choosing other scopes is H6.

### H6, scope (done in slice 4c, see there; D139)

Pill with the current branch, all refs, or one ref. Left-clicking a branch in the sidebar, on the history page, sets the scope. Checkout stays on double-click or the context menu. "Current stack" shows disabled with "Not available yet" (D34).

### H7, content search

`git log -S` for fixed text. The field sits in the history header, not in the top bar. It is debounced and cancellable (D35).

### H8, performance gate

Benchmarks, ignored by default, on repos of 50,000 and 200,000 commits, generated with `git fast-import` (D37).

Out of slice 3 (author's decision on 2026-10-17): stack scope, commit owner, stack parent, dashed edge, Stacks tab, Restack counter, Fetch and Pull, reflog, stashes and local review. The stack items stay "Not available yet".

Done when: browses the history of a large repository with smooth scrolling.

## Slice 3d: branch filter in the sidebar (done, with gaps)

Decision D53 (design/decisions.md, proposed on 2026-10-19). The sidebar shows "Mine" (the current branch, and branches whose tip commit is the user's) or "All", with a search by name and a "N of M" count. If no branch matches, "No branches match" shows, with a link to show all. The Mine/All scope, its two palette actions and the `branch_scope` setting were removed again (D134): the sidebar lists every branch and the search narrows it.

- Done when: the scope, search and default rules are tested in a pure module (`branch_filter.rs`). Tree rows come from the filtered set, with no empty group header (`sidebar.rs`). The actions have tested rules and plans. The scope survives a restart (round-trip test of the file).
- Known gaps: nothing was seen in a window. The smoke scenario was neither written nor run. On this machine GPUI does not start (no software Vulkan driver), so the UI has not been checked on screen. Remote branches with no readable author are hidden under "Mine". The filter is recomputed every frame, with no cache. Keyboard navigation inside the filtered tree did not change. The search text is not saved. "Mine" looks only at each branch's tip commit. The "Local" section disappears when there are no local branches, also without a filter.

## Slice 3w: workflows (in progress: S-D and WS0 to WS2 done, with known gaps; WF3 to WF5 to do; WF0 to WF2 were built in TOML and are superseded by the Lua scripts, D98 to D102)

Proposal of 2026-10-19, detailed in the TOML workflow specification (available in git history at commit 5f88889, `docs/10-workflows.md`; replaced by `design/workflow-scripts.md`), with decisions D60 to D80 in `design/decisions.md` (D73 to D75, D78 and D79 decided in principle; the rest proposed). The first design made a workflow a sequence of typed engine actions, tied to a trigger and declared in TOML, one file per workflow. Each step is a `Command`, so this slice adds no new logic path. The stack policies and the LLM commit message become workflows, with no specific code. The author confirmed the position (Q20): right after slice 3 and before 3c. WF4 and WF5 come with slices 4, 5 and 7.

**Change of direction (2026-10-22, D98 to D102, design in `design/workflow-scripts.md`).** The author decided to replace the TOML state machine with Lua scripts running in an embedded sandbox, and WS2 (D108) removed the TOML engine. Execution stays ephemeral for now. Durable execution in the style of Temporal is postponed (Q23). WF0 to WF2 below stay as a record of what was built; the scripts supersede them. In WS2, `exec`, `loader`, the session, `catalog`, the run view and the palette were kept and adapted, and `model`, `parse`, `expr`, `validate`, `machine`, `edit`, `dry_run` and the Flow tab were removed. WF3 has not started, so it will be built once, on scripts. There is no support for both formats.

- **Spike S-D, script engine (done on 2026-10-23, Linux only; D104: Luau via `mlua`).** It compares Luau (`mlua`), Lua 5.4 and Rhai on:
  - sandbox escape attempts (`os`, `io`, `load`, `require`, `string.dump`, a memory bomb, an infinite loop, deep recursion, changing globals between runs);
  - instruction, time and memory budgets;
  - cancellation latency, including a blocked call;
  - the cost of a new VM per run;
  - binary size;
  - building on the three platforms and in CI.

  Result in `FINDINGS.md` of the script-engine spike (git history, 7932c61) and in `design/decisions.md` (Q22). Done when: an engine is chosen, with numbers and a list of escape attempts that fail. Done: Luau passed every must-have (`FINDINGS.md`). Known gaps: macOS and Windows were not measured (a CI matrix job proves the build before a release depends on it), the `.d.luau` editor type stub was not verified, and the budget values are starting points to tune in WS0.
- **WS0, script core (done on 2026-10-23, with known gaps).** Adds the `ScriptEngine` port in `gm-core`, with a VM adapter and a fake `ctx` for tests. The `ctx` is generated from the catalog (level, signature, outcomes). It also covers budgets and cancellation, the call journal, and ephemeral execution of a run on its own thread, with blocking `approve` and `wait` (D100). An interrupted run is marked `interrupted` on the next open. Done when: a sample script runs against real Git in a temporary repository (`crates/gm-core/tests/workflows_real_git.rs`, adapted); an infinite loop, a memory bomb and each escape attempt end in `failed` with the reason; cancelling wakes a blocked `approve`; killing the engine in the middle of an `apply` leaves the run `interrupted` on the next open.
  - Built in `gm-core/src/script/` (D104 to D106): the `ScriptEngine` port, the Luau adapter, `ctx` generated from the catalog, budgets and cancellation, the SQLite call journal, the approval policy of D64 and D101, and one thread per run with blocking `approve`, `wait` and `exec`; `start_script` and `script_effect` on the engine carry a run's effects out through the same `Actions` as before (D81). Checked: 62 sandbox tests (escape attempts, tampering between runs, the six budgets, cancellation, cyclic and deeply nested data, hostile metamethods, memory bombs), 23 runtime tests with a closure in place of the VM, 9 real-Git tests through the real VM, 3 determinism properties, and a crash test that kills a real process in the middle of an `apply`.
  - Known gaps: nothing reaches a user yet. There is no command or event for a script run in `gm-schema`, the runner does not yet carry a run's effects and events to the engine thread (the engine methods exist and the tests play that thread), and the UI still runs the TOML engine; WS1 and WS2 close this. A script is not reviewed by hash yet (WS1): `RunSpec.reviewed` and `auto_approve` come from the caller. `concurrency` (`queue`, `replace`, `skip`), triggers and phases are WF3. `fetch`, `push.lease` and the stack, restack and reparent calls are declared (D106) but a call ends `failed` with "not available yet" until WF4. `ctx.wait` is tested with the closure engine and the runtime, not yet from a Lua script. The crash test kills a runtime driven by a closure engine, not the whole engine with Luau. The budget values are the starting ones of D104 and no release-build timing was taken. The Linux build links the C++ runtime statically (`.cargo/config.toml`); the macOS build is not tried, and the CI and release workflows are not changed. The journal tables share `state.db` with the TOML engine's until WS2 removes them. The pages in `docs/` keep their `planned` status: no user can run a script yet.
- **WS1, loader, `M.meta`, models, approval by hash and CLI (done on 2026-10-23, with known gaps).** Covers a loader for `.lua` files (user directory and `gitmaster.workflowPath`), `M.meta` read without `ctx`, built-in models rewritten in Lua, and trust by hash (D101, "Run this version?" dialog). It also sets the rule that `apply` runs after `exec`, generated documentation in `.api/` (D102), and the subcommands `gitmaster workflow check`, `dry-run` and `test`. Done when: `suggest-commit-message` and `branch-from-ticket` in Lua run from the palette; an edited script goes back to "not reviewed"; `check` points to the file and line of an error; `test` checks the call sequence of a script against a fixture; `dry-run` lists the calls without running `exec`; an agent can write a new script using only `.api/` and the subcommands.
  - Built (D107): the `.lua` loader (`script/loader.rs`) and `inspect`, `M.meta` validation (`meta.rs`), the review store by hash (`trust.rs`, `gitmaster.db`), the four templates in Lua, the generated `.api/` reference (`api_docs.rs`), the three subcommands (`cli.rs`, `input.rs`, dispatched from the binary), and the "Run this version?" dialog (`gm-ui/src/workflow_review.rs`). The palette's list, run, approve, cancel and create-from-template commands are script-backed, and the run events are the ones the window already knew.
  - Checked: 14 command-level tests through the engine with real Git, 24 CLI tests, and the loader, trust, templates and API-reference unit tests. Seen in a window (release build, virtual display): the palette lists a Lua workflow, its inputs dialog, the review dialog with the script text, a confirmed run that created and checked out a branch and showed the script's notice on the composer, a second run that did not ask again, an edit that asked again, and Escape cancelling. On the real binary: `check` pointing at a line, `dry-run` that neither ran a program nor changed the repository, `test` passing and failing. An agent given only the generated `.api/` folder wrote a working script and its two fixtures, and a mutated script failed that fixture.
  - Known gaps: the approval dialog for `ctx.approve` and an after-`exec` apply, and cancelling a run, were not seen in a window (they reuse the existing events, tested through the engine only). The Workflows page still edits TOML: its Save, Delete, Dry Run and run-history commands keep the TOML behavior until WS2, so a Lua workflow opened there reads wrongly. Keyboard shortcuts of workflows (`trigger.key`) are validated but not bound. `workflow` as the first argument of `gitmaster` is now a subcommand. The TOML engine tests that went through the changed commands were replaced; the page's save and delete tests return with WS2. No UI smoke scenario yet (WS2). The `.d.luau` type file is still unchecked in an editor. macOS is not built or tried, and CI and release workflows are unchanged (the Linux runners have `g++` and the static `libstdc++`).
- **WS2, interface and removal of the TOML engine (done on 2026-10-23, with known gaps).** A Workflows page with Script and Runs tabs (one entry per `ctx` call), dry run on the open text, atomic save, and a conflict when the file changed on disk. It also removes `model`, `parse`, `expr`, `validate`, `machine`, `edit`, `dry_run`, `flow_graph*` and `flow_inspector`, the `.toml` fixtures and the `w-flow-editor` scenario. A `.toml` in the directory is ignored with a warning. Done when: a script is created from a template, edited, saved and run, with the Runs tab showing the calls; no code of the old format remains; a UI smoke scenario exists.
  - Built (D108): the Workflows page with Script and Runs tabs (live diagnostics, dry run on the open text, atomic save with conflict, delete to the trash, run, runs with one entry per `ctx` call, `Interrupted` and `Cancelled` states), `trigger.key` shortcuts, `CheckWorkflowScript`, and the removal of `model`, `parse`, `expr`, `validate`, `machine`, `edit`, `dry_run`, `driver`, `store`, `loader`, `flow_graph*`, `flow_inspector`, the `.toml` fixtures and the `w-flow-editor` scenario. A `.toml` file is ignored with a warning.
  - Checked: unit tests of the page model, key plans and diagnostics tracking, and engine tests with real Git for list and load runs, dry run, check, save, delete and the `.toml` warning; clippy, fmt and `make docs` clean.
  - Known gaps: nothing was seen in a window and the smoke scenario `w-script-editor` was written but not run; its coordinates are marked unverified. The Script tab layout, click-to-line on a diagnostic, the shortcut firing, the review dialog by key and the Runs tab display need a first look. Cancel and Approve have no buttons on the page (palette only). `make check` (audit) was not run. The unused `workflow_*` tables stay in existing `state.db` files. `current_state` of a run summary is always empty for scripts. The slow test `script::templates::tests::create_gives_up_after_the_copy_limit` takes about 100 s.
- **Script editor (D149, D151), in progress.** Built: the Lua grammar is registered for highlighting, with a query checked against the component's names; the completion rules for `ctx` are a pure module, with unit tests, that reads the sandbox's surface and the reference text. Not built: the code editor in the Script tab (line numbers, completion, highlighting, problems underlined in the text), the undo and dirty tracking through its events, and the diagnostics. It is blocked on the window root (Q35): the component needs `Root`, which draws a second window frame. Done when: the Script tab is the code editor with highlighting and completion, undo and Save and Dry Run behave as before, and `scenario_w_script_editor` passes under Xvfb.
- **WF3, event triggers, waits, phases and migration of hooks (D75, D98 to D101), now on scripts and after WS0 to WS2.** Covers:
  - engine events as triggers, `match`, debounce, and concurrency (in memory, D100);
  - automatic approval of event `apply` steps, tied to the reviewed script hash (D101), and `Stale` reconciliation;
  - `ctx.wait` for events such as `restack.resumed`, while the app is open;
  - `before.<operation>` phases (a gate, with `ctx.block`), `transform.<field>` (the returned value) and `after.<operation>` (observation);
  - parity of `transform.commit_message` with `prepare-commit`;
  - the action "Convert hooks to workflows", which writes the equivalent Lua scripts into the user's workflow directory;
  - GitMaster stops reading the `[gitmaster "<hook>"]` sections of `.gitconfig`.

  Done when: the behavior of `prepare-commit` is reproduced by a workflow and the hook code path is removed; a workflow on `commit.created` runs; the `apply` step waits for approval; editing the file sets the hash back to "not reviewed" and revokes automatic approval; closing the app in the middle of a run leaves it `interrupted` on the next open (D100); a run that calls `ctx.wait("restack.resumed")` stops and resumes when the event arrives, while the app is open.
- **WF4, stack actions and templates (with slices 4 and 5).** `ctx.stack.*`, `ctx.restack.*`, `ctx.reparent.*`, `ctx.push.lease`, the events `stack.parent_moved` and `stack.parent_merged`, and three built-in templates, in Lua. Done when: turning on "Restack children when parent moves" restacks the subtree after a commit on the parent, with a plan, undo and a pause on conflict, and with no code specific to this policy.
- **WF5, MCP (with slice 7).** `workflow_list` and `workflow_run`. There is no tool to create or edit scripts (D102). Done when: an agent lists the workflows and fires one; the `apply` steps of the run wait for the human's approval in the UI (D64(c)).

## Slice 3c: code navigation (in progress: S-A, S-B, S-C, N1, N2, N3 and N4 done; N5 next)

Details, engines and risks are in `design/code-navigation.md`. Decisions D43 to D51 are in `design/decisions.md`: D43 is decided; D44 to D51 are proposed, with D44 and D46 supported by the spikes (D89). The slice adds a Code page with Quick Open for files, grep and symbols, on top of a read-only viewer. Each sub-slice has its "Done when" in `design/code-navigation.md`.

- **Spikes S-A, S-B and S-C** (lists and grep at scale, tree-sitter, path matcher), before building.
  - **S-A and S-C done on 2026-10-21** (`FINDINGS.md` of the nav-a-lists spike (git history, 7932c61), `FINDINGS.md` of the nav-c-match spike (git history, 7932c61), D89). They confirm D44 and D46 at 100 thousand files, and the matcher up to 1 million. Still to measure on a Mac: grep's `--threads`, and grep on the 1-million-file worktree.
  - **S-B done on 2026-10-22** (`FINDINGS.md` of the nav-b-treesitter spike (git history, 7932c61), D92). Tree-sitter is viable (speed, size, accuracy), but only with its own queries. macOS was not measured; `measure.sh` and `run.sh` run there unchanged.
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
  - Measured on 100 thousand files: for tracked files only, a common pattern reaches the limit in 0.065 s and a full scan takes 0.9 s. With untracked files, the times are 1.1 s and 2.0 s. The extra 1 s is Git's worktree scan for untracked files, the same cost as `status`, and it is paid even when there are none. See the correction in `FINDINGS.md` of the nav-a-lists spike (git history, 7932c61) and D91.
  - Known gaps:
    - the result arrives whole, with no streaming or progress, so a full scan on a huge repository (about 9 s over blobs in 1 million files) stays at "Searching…" until it ends; only a new search cancels it;
    - the search covers the worktree, not a revision;
    - the files box splits on spaces, so a path with a space works only through a glob;
    - the regex is Git's extended syntax (`-E`), with no `\d`, and highlighting uses Rust's `regex` and may differ from Git in rare cases;
    - a file name with a line break splits the record, and that line disappears;
    - the results list has no keyboard support and no text selection;
    - `--threads` must be measured on the Mac;
    - the Code page's search field and Quick Open are still separate boxes (the single box is N5, D50).
- **N4, syntax highlighting** (done on 2026-10-25, D111; no smoke scenario, per D88; nothing seen in a window).
  - The viewer colors Rust, Python, Go, TypeScript with TSX, and JavaScript (widened on the same day by D112 to Java, C, C++, C#, Ruby, Bash, JSON, TOML, YAML, HTML and CSS, and by D113 to Clojure), in twelve roles with palette tokens in both themes (design/visual-identity.md). The text shows first and the colors follow in a `FileHighlighted` event; a file in another language, a binary and a missing file stay plain.
  - D112 samples: one test per added language checks its roles, a test checks every compiled-in query compiles and names only known captures, and one checks the path-to-language table. Not done: the timing and binary size of the added grammars, Markdown, SCSS and PHP.
  - Built: pure module `gm-core/src/syntax.rs` (language by extension, capture name to role, `highlight(language, text, stop)` with a 2 s budget checked by the parser and the query cursor, long lines left plain), the product's queries in `gm-core/queries/`, one Cargo feature per grammar, `HighlightJob` on its own thread stopped by a newer read, `SyntaxRole` and `HighlightSpan` in `gm-schema`, `Shown.spans` and `styled()` in `gm-ui/src/code_page.rs`, and the painter in `shell_code.rs`.
  - Checked: 20 unit and property tests of the module (samples per language, span order and character boundaries, determinism, stop, budget, long lines, multibyte, syntax errors, every capture of every query has a role), 4 engine tests (text before colors, no job for plain files, a newer read stops and drops the colors), 8 model tests for the pieces of a line (scroll, tabs, spans from earlier lines, the colors of a file no longer on screen are dropped), and 3 theme tests (4.5:1 on four backgrounds in both themes, distinct colors, one per role). Release build on Linux: 2 MiB of Rust (58,000 lines) in 0.9 s, first-use query compile 37 ms (`cargo test --release -p gm-core --lib syntax::tests::timing -- --ignored --nocapture`).
  - Known gaps: nothing was seen in a window, so the actual look of the colors and the legibility on a real screen are unchecked; the binary growth (about 5 MB) and the macOS and Windows builds were not measured; the whole file is parsed on every read (no incremental parsing, the viewer is read-only); there is no text selection, so the `selection` background is checked in theme tests only; the queries are the crates' seeds with a few corrections, not yet tuned on real code (for example Rust `self` and attribute items, Python decorators, TypeScript generics), and an unlisted capture name is silent (a test lists them); `.d.ts` files go through the TypeScript grammar, and no extensionless file (a shebang script) gets a language; highlighting in the diff is separate (D42); injections (code in markdown) are out; Markdown, SCSS and PHP have no grammar yet.
- **N5, symbols and single box**: index by blob, single search box (D50), outline, go to definition.
- **N6, revisions, MCP Read tools and performance gate.**

Done when: opening a file by name, finding text and jumping to a definition takes seconds in a 100 thousand file repository, without freezing the UI.

## Slice 4: stacks (model and visualization) (in progress: 4a, 4b and 4c done, with gaps; 4c2 and 4d to do)

- Metadata `stackParent` and `stackParentBase`; create and reparent a branch (drag).
- Sidebar per stack; lag indicators; dashed edge in the graph.
- Derived events `stack.parent_moved` and `stack.parent_merged` (D68), which feed workflow triggers.

Done when: a real everyday stack is represented and the lag is visible.

Sub-slices (agreed with the author on 2026-10-31: engine and sidebar first; D13 and D68 decided, rules in D138):

- **4a, model and By stack sidebar (done on 2026-10-31, with known gaps).**
  - Engine: `LoadStacks` (`StacksLoaded`, a read, not logged) and the human-only writes `CreateChildBranch`, `SetStackParent`, `ClearStackParent` and `ReparentToGrandparent`, which change metadata only and answer with a fresh snapshot. Pure rules in `gm-core/src/stack.rs` (forest, loops, states, `restack N`, refusals, grandparent), port reads with libgit2 (merge base, ahead and behind, ancestry, patch ids) and writes with `git config --local`, logged. Derived states: current, stale, diverged base, parent gone, parent merged (fast-forward, merge, rebase and squash merges).
  - UI: the By stack view draws each tree with its depth, the amber dot, `↑N`, `restack N` and the state tags (each with a tooltip), **Calculating…** before the first snapshot, and loops under No stack with a `parent loop` tag. The stacks are read again after every status refresh, with one read in flight at most. New Child Branch opens the New Branch dialog in a child mode; Set Parent… picks the branch, then its parent, in the palette; Remove From Stack; Reparent to Grandparent asks first in the menu. They are in the branch menu, the worktree-branch menu and the palette.
  - Checked: 10 unit tests of the rules, 17 real-Git tests (`crates/gm-core/tests/stacks_real_git.rs`: create, stale, diverged, amended parent, squash, rebase and merge detection, gone parent, set and clear, refusals, agent refused, odd names, loops, a linked worktree), and UI model tests (sidebar rows, the stacks model, the registry rules and effects, the child form). Seen in a window (release build, virtual display, light theme): the ledger stack of the docs with stale branches and counts, a squash-merged parent tagged, the branch menu, Reparent to Grandparent with its confirmation, Set Parent… through both palette steps, and New Child Branch through the dialog, each checked against `git config`.
  - Known gaps: the snapshot is read on the engine thread, one libgit2 pass per link, so a large stack delays other commands while it reads and the read cannot be cancelled; it is not measured on a large repository. Patch ids look at most 2,000 commits of the root, and a squash merge whose context lines changed on the root is missed. `↑N` after Reparent to Grandparent counts the old parent's commits too until a restack. The palette runs Reparent to Grandparent without the menu's confirmation. The two config writes are not atomic: a failure between them leaves a parent with no base, shown as diverged. The registry's `Ctrl+B` for New Child Branch is not bound. The sidebar keeps the depth of a stack branch when the search hides its parent. No UI smoke scenario was added to `scripts/ui-smoke.sh`.
- **4b, drag a branch onto another to set its parent (done on 2026-10-31, with known gaps).**
  - Built in `crates/gm-ui/src/branch_drag.rs`: in the By stack view each local branch row is a drag source and a target for a branch, with its own payload type (`DraggedBranch`), so it never meets the file drag's handlers. A pure `answer` accepts or refuses each row: the branch itself, its current parent, a branch under it, and a busy repository refuse, with the sentence the status bar shows; the engine still checks shared history. A drop sends `SetStackParent`, the command of Set Parent…. A 50 ms poll ends the window's drag when gpui no longer carries one, as the file drag does.
  - gpui keeps one `can_drop` predicate per element and draws `drag_over` styles only when it passes. The file drag's predicate on a branch row now lets every other payload through, so the branch drag reaches its own handler there, and a refusing row is dimmed; the branch drop checks again and says why it refuses.
  - Checked: unit tests of the answer and of the drag's state; seen in a window (release build, virtual display, light theme, driven with xdotool): a branch dragged over its parent is dimmed with "… is already the parent of …", over another row is highlighted with "Make … the parent of …", and the drop changes `branch.<name>.stackParent`; a file dragged onto a By stack branch row still opens the commit popover, and a refusing row still refuses it.
  - Known gaps: the release-build drag measurement owed by M4 of slice 1f was not made (the virtual display renders on the CPU, so its frame times say nothing about a real machine). The sidebar does not scroll during a drag, so a target out of view needs Set Parent…. There is no drop on the No stack header to remove a parent. Esc ends the gesture only when the mouse is released (as for files). The dark theme was not looked at during a drag. The file drag's own `can_drop` still refuses before gpui draws its dimmed style, so a refusing row for a file is not dimmed (a gap of M5, not changed here).
- **4c, history scope and stacks in the graph (done on 2026-10-31, with known gaps; D25 decided, D139).** It also builds H6 of slice 3 (the scope pill), as the author chose.
  - Engine: `LoadHistory { scope }` with Current branch, Current stack (the branches of HEAD's stack, the default branch and HEAD), All refs (`--branches --remotes --tags` and HEAD) and One ref (a local branch by its full ref name, after `--end-of-options`); `GitPort::history_stream` takes the revisions. Each `StackBranch` lists its own commits (`stackParentBase..tip`, newest first, up to 1,000).
  - UI: `history_scope.rs` (the choice, the default that follows HEAD, the menu's lines and reasons); the scope pill and its menu in the history header (deferred, so it paints over the rows); a left click on a sidebar branch on the History page shows One ref and never checks it out. The layout takes `StackMarks` from the snapshot: a node owned by a stack branch takes its colour, and the first-parent edge of the oldest own commit of a branch behind its parent is dashed and amber until it reaches its base or a solid line joins its lane. Each branch chip behind its parent is followed by **N commits behind parent**. A new snapshot lays the graph out again; the first one reads the list again when it changes the default scope.
  - Checked: 7 real-Git tests (`crates/gm-core/tests/history_scope_real_git.rs`: each scope, a tag named like a branch, a name read as an option refused, own commits), layout tests (marks from the snapshot, the dashed run and where it ends, a cut list, a new snapshot redoing the rows), and the scope model's tests. Seen in a window (release build, virtual display, light and dark): the fixture's ledger stack under Current stack, Current branch and All refs with the expected dashed edges and badges, the menu over the rows, and One ref from a sidebar click with HEAD unchanged. The window check found the menu painted under the rows and a badge read as the wrong branch's; both were fixed and seen again.
  - Known gaps: Commits and Stacks modes are 4c2. The pill is not reachable by keyboard and has no palette action. One ref of a name that resolves to nothing shows an empty list with no message (Git's error is not read while the walk streams). Current stack reads the snapshot again in the engine on each load. A commit owned by no stack branch keeps its lane colour even inside a stack. The behind badge sits on the row of the branch's tip only, so a tip outside the scope shows none. The ownership list stops at 1,000 commits per branch, and then no edge is dashed. No smoke scenario was added.
- **4c2, Commits | Stacks switch** in the history graph: one node per branch, with its commits folded or unfolded.
- **4d, the derived events** `stack.parent_moved` and `stack.parent_merged` for workflow triggers (with WF3), and `stack_tree` over MCP (slice 7).

## Slice 5: restack (in progress: 5a to 5e done, with gaps)

- Exact plan per commit, in-memory execution, snapshot and atomic undo, persisted state.
- Fallback without a resolver: on conflict, pause and deliver the conflict in the working tree to resolve outside.
- Workflows `before.restack` and `after.restack` (the old `pre-restack` and `post-restack` hooks, D75).
- Q2 decided: block with a banner (2026-10-31). Execution in D142.
- Primitives exposed as Commands: `restack.plan`, `restack.apply`, `restack.undo`, `reparent.*` and `push.lease` (D68).
- The automatic update policies are workflow templates (WF4), with no specific code (D68).

Done when: a complete restack runs, can be undone, and survives closing the app.

Sub-slices (agreed with the author on 2026-10-31; D14 decided, execution in D142; each engine first, with `crates/gm-core/tests/restack_real_git.rs`):

- **5a, plan (done on 2026-10-31, with known gaps).**
  - Engine: `PlanRestack { branch }` answers `RestackPlanned` (a read, not logged). Pure rules in `gm-core/src/restack.rs` walk the subtree parents first and decide each branch; the port's `restack_replay` runs `merge-tree --write-tree` per commit and `commit-tree` with the original message, author and author date, writing unreferenced objects only. A child is replayed onto its parent's planned tip. Result types in `gm-schema/src/restack.rs`, with a `plan_id` over tips, bases and new tips.
  - Refusals: diverged base, parent gone or merged, a merge commit among the own commits, 1,000 or more own commits, a branch checked out in a linked worktree; a dirty working copy with a plan branch checked out stops the whole plan (Q2). A conflict blocks the branch and its descendants only; a commit already in the base is dropped.
  - Checked: 9 unit tests of the rules and 9 real-Git tests (`crates/gm-core/tests/restack_real_git.rs`). No window: the UI ignores the event until 5e.
  - Known gaps: the plan runs on the engine thread and cannot be cancelled, as the stack snapshot in 4a; commits are not signed even with `commit.gpgSign`; `merge-tree` does not use rerere (slice 6); the committer is the current user, as for a rebase, so the committer date differs from a `git rebase` result only by time. Not measured on a large stack.
- **5b, apply and undo (done on 2026-10-31, with known gaps).**
  - Engine: `ApplyRestack { branch, plan_id }` plans again, refuses a changed plan id or a stopped plan, saves the record in `.git/gitmaster/restack.json`, moves every unblocked branch in one `git update-ref --stdin` transaction (each ref checked against its old value), brings the checked-out branch's index and files along with `read-tree -m -u` (the refs are put back if that fails), then writes the bases. `UndoRestack` refuses when a branch moved since, and reverses refs, files and bases. Both are human only and answer with `RestackFinished`, the branches, the status and the stacks.
  - The replay keeps the original committer date, so planning twice gives the same commits and the applied tips are the planned ones.
  - Checked: 7 more real-Git tests in `restack_real_git.rs` (refs, bases and stack state after apply, a stale plan id, the checked-out branch clean, undo once, undo after a later commit, a blocked branch beside a moving sibling, an agent refused). No window.
  - Known gaps: a crash after the ref transaction and before the bases leaves the bases old (5c repairs it from the record); the record keeps only the last restack.
- **5c, persisted state and resume (done on 2026-10-31, with known gaps).**
  - The record has a phase: `applying` is saved before the refs move and `applied` after the bases. `ResumeRestack` (human, sent when a repository opens) drops a record whose refs never moved, finishes one whose refs all moved (files of the checked-out branch when the working copy shows changes, then the bases), and reports a mix of old and new tips without touching it. Undo resumes first.
  - Checked: 5 real-Git tests (kill before the bases, before the files, before any ref, a mix, a new engine undoing an applied restack). The kills are simulated by putting the state back by hand, not by killing a process.
  - Known gaps: the UI does not send `ResumeRestack` yet (5e); a kill during the ref transaction itself relies on Git's own atomicity; one record only, so a second restack replaces the undo of the first.
- **5d, conflict handling (skip done on 2026-10-31; delivery of a conflict to the working tree not built).**
  - A conflict already blocks only its branch and the ones under it, and apply moves the rest. `PlanRestack` and `ApplyRestack` take `skip`, the branches to leave out (Skip branch and children): they and their descendants are not replayed, and the plan id changes with the list. Abort all is not sending `ApplyRestack`, since a plan moves nothing. Checked: one real-Git test (skip keeps a branch and its child, moves a sibling, and the skipped ones are offered again) and a unit test.
  - Not built: delivering a blocked branch's conflict in the working tree to resolve outside, and continuing after. It needs the author's choice between a temporary worktree and the open checkout, and overlaps slice 6.
- **5e, UI flow (done on 2026-10-31, with known gaps).**
  - `crates/gm-ui/src/restack_dialog.rs`: Restack This Stack (branch menu, palette) and the top-bar Restack open a dialog for the stack of the branch, planned from its root. It lists each branch with what its replay gives (commits replayed, dropped, the conflicting commit and files, why a branch waits or is refused), a banner for a stopped plan, **Skip branch and children** / **Include** per branch (the plan is made again), **Restack N branches**, then **Undo restack**. `ResumeRestack` is sent once when the stacks first load, so the Undo of a restack from an earlier session is offered.
  - Checked: 9 unit tests of the dialog's state and wording and a registry test; `make check` for the code and docs; seen in a window (release build, virtual display, light and dark): the plan, Skip and Include, apply (the sidebar's `restack 2` and amber dot went away, the branches sat on `main`, the bases were written), and Undo (the original commits and bases came back, the record was removed).
  - Known gaps: the shortcut ⇧⌘R is not bound; there is no progress view while it runs (the apply is one transaction); the done view still words the rows as "Replays"; a failed plan or apply shows only its reason; the dialog is not reachable by keyboard apart from Enter and Esc; no smoke scenario in `scripts/ui-smoke.sh`; Push with lease and delivering a conflict are not built.
- Out of this slice for now: `before.restack` and `after.restack`, push with lease, MCP.

## Slice 6: conflict resolution

- Three panes, with sides labeled by role, base on demand, rerere.
- The hook `resolve-conflict` becomes the workflow `transform.conflict_hunk` (D75).

Done when: the conflicts of a restack are resolved without leaving the app.

**Status:** C1 to C4 built except script suggestions (parser, engine commands, the resolver page from the Working Copy's file menu, recorded resolutions with the Review and Forget banner, and the origin of each hunk: human or recorded resolution). Nothing seen in a window; no smoke scenario, at the author's request.

Known gaps:

- The Result pane re-shapes its whole text on each change, and the conflict list is not virtualized, so large files with many conflicts are slow.
- No count of the non-conflicting changes Git applied on its own ("N changes applied").
- No keyboard shortcuts in the resolver; Escape and focus behaviour not seen in a window; the dark theme not seen.
- The restack's **Resolve conflict** entry is not built, and the restack itself is not available.
- The unified or stacked layout (Q9) and script suggestions are not built.
- The rebase's left-side label names `onto`, while stage 2 is the partially rebased HEAD.
- Recorded resolutions (D117): a hand resolution identical to a postimage reads as recorded; forget restores markers in the user's conflict style; resolving a recorded file to different text is not recorded; `ReadConflict` fails when the rerere pass fails.
- Review replaces the Result pane text, so edits made there before Review are lost.
- A recorded text that still has markers leaves its hunks unresolved, so no card shows the recorded origin; the banner carries it. A free edit that changes the text makes every origin human (D118).

## Slice 6t: threads, agent session capture (built 2026-10-29 except T0, T8, T9, with gaps; nothing seen in a window and never run against a real agent)

Requested by the author on 2026-10-29. Decisions D119 to D129 and open questions Q25 to Q31 in `design/decisions.md`; the engineering design, schema, redaction rules and sub-slice plan are in `design/threads.md`; the behavior is in `docs/guide/threads.md` and `docs/agents/agent-integration.md`, marked `Planned`. It sits before slice 7 because the MCP search tools (T9) need the store, and it needs no MCP to be useful: the window searches and reads threads on its own. Pillar 3.

**Goal.** Capture agent sessions per repository into SQLite (the conversation, every tool call and every file edit), redact secrets before anything is stored, and let the user browse, search and curate them in the window. Search is an engine command, so the MCP server exposes it later without new logic.

**Scope**

1. **T0, contract and measurements (spike).** Real hook payloads, transcripts and plugin layout of the harness as fixtures; cold start of the hook command; FTS5 and tokenizer; redaction throughput. Resolves Q25 and Q26. Needs a machine with the harness.
2. **T1, model and redaction (pure).** Types and commands in `gm-schema`; redaction engine with built-in patterns, user patterns, path rules, fail-closed validation and scrub; query builder; `settings.json` key `threads`.
3. **T2, store and search.** `threads.db` (WAL, FTS5, `secure_delete`, caps, migrations), `ListThreads`, `SearchThreads`, `ReadThread`, `ReadThreadEdit`, the watcher and `ThreadsChanged`.
4. **T3a, harness-neutral capture.** The `HarnessAdapter` trait, canonical events, registry and `--harness` argument (D129); `gitmaster thread hook`; the pipeline (redact, cap, idempotent upsert, offset cursor, spool, edit pre and post images); capture gaps; the conformance kit, proven with a fake harness.
   **T3b, Claude Code adapter.** Hook and transcript parsing, tool mapping, sub-agents, resumed sessions, import of existing sessions.
5. **T4, integration installer.** Each adapter's generated plugin, `gitmaster harness list | install | status | uninstall <harness>`, palette actions, stale-path detection. (Superseded by D150: the plugin is source in the repository, installed with `claude plugin`; `harness list` remains.)
6. **T5, Threads page.** List, filters, search, detail by turn, edit cards with **Open Diff** and **View File**, empty and paused states.
7. **T6, curate and link.** Rename, note, star, tags, delete, export, manual and observed links, derived edit state, **Show Threads** on a file and a commit.
8. **T7, redaction settings.** Patterns, path rules, test field, per-repository pause, **Scrub Threads**.
9. **T8, observed turn changes (optional).** Per-turn working-tree snapshot for edits no tool event covers; ships only if a release-build measurement fits the bar.
10. **T9, MCP read tools (with slice 7).** `threads_search`, `thread_get`, `thread_read`, the skill in the plugin and the policy switch.

**Done when:** a Claude Code session in a repository appears as a thread within a second of each event, with its prompts, replies, tool calls and edits; no string matching a built-in or user pattern exists anywhere in `threads.db`, the spool or the log; the hook never fails or noticeably slows the agent; the user finds a past decision by searching, opens the agent's edit as a diff and links the thread to a commit; deleting a thread removes its text from the file; the working tree is never written. T0 to T7 (T3 as T3a and T3b) each have their own done-when in `design/threads.md`.

**Status (2026-10-29).** Built by the agent in one session, in this order, each checked with clippy `-D warnings` and tests: T1 (canonical types in `gm-schema`, redaction engine, `threads` settings, FTS query builder), T2 (SQLite store with FTS5, `secure_delete`, `0600`; engine commands, events and a watcher for `ThreadsChanged`), T3a (harness trait, registry, canonical events, conformance kit proven with a fake harness; the pipeline: redaction before storage, caps, idempotent upsert, offset cursor, spool, edit pre and post images with libgit2, observed links), T3b (Claude Code adapter), T4 (`gitmaster harness list | install | status | uninstall`, generated plugin), T5 (Threads page), T6 (star, rename, note, tags, link, delete, export, scrub, Show Threads for a file), T7 (Threads Settings dialog, Install Agent Integration dialog). D130 records the choices the design did not cover.

**Known gaps:**

- **T0 was not done.** The Claude Code hook payloads, transcript format, plugin layout and install commands in `harness/claude_code.rs` come from documentation, not from a real session (Q26); the fixtures are hand-written. Until T0 runs on a machine with the harness, assume the adapter may not match. The hook's cold start was measured on Linux only: p95 27 ms in a release build (Q25); macOS is unmeasured.
- Nothing in the UI was seen in a window (no display): layout, colors, scrolling and keyboard paths of the Threads page and its dialogs are unchecked. Patches in a card show 400 lines at most.
- Not built: a **Scrub Threads** button and a paused-repositories list in the settings dialog (scrub is a palette action), per-repository pause of capture (needs `gitmaster.db`), filters for agent, branch and date in the UI, T8 (turn-level snapshot), T9 (MCP read tools, with slice 7) and the plugin's skill and MCP settings.
- A match that touches a `[REDACTED:…]` marker is ignored, so a secret written directly against a marker with no separator can survive; a token glued to a letter (`x` + token) misses the `\b` of some patterns; a secret no pattern describes is never caught (stated in the docs).
- Edit patch truncation is not flagged, an over-8 MiB payload is dropped rather than stored as a placeholder, batches drained from the spool get no commit links.
- `ListThreads.total` is the unfiltered count; edit state is derived against the open repository's worktree, not the thread's own.
- Follow-up (2026-10-30): thread search has a trigram fallback (D131): exact, then substring, then typo-tolerant matches, marked `approx.`. Follow-up (2026-10-30, D132): the search indexes only the conversation (`user` and `assistant` entries) and edit paths, schema 3; tool calls and results are no longer indexed, which cut the trigram database to 1.33 times the size without it on a corpus shaped like real threads (84% of the bytes in tool results; D132, down from 2.8 times on the D131 corpus, which is not comparable); the hook does not migrate a database of 50,000 entries or more and spools instead.
- Follow-up (2026-10-30, D133): Claude Code's markers (slash commands, command and bash output, reminders, interruptions) are cleaned by the adapter; the core never sees them. Stored entries are repaired once by schema 4. Gaps: a marker nested inside another marker's body stays as text; every stored prompt loses its surrounding whitespace in the repair; the Threads page was not checked in a window.
- Follow-up (2026-10-29): the thread detail is a virtualized variable-height list (`gpui::list`) that pages in as you scroll (200 entries, earlier and later), a search hit loads around its entry and scrolls to it, tool calls are grouped and collapsible (closed by default, Expand all / Collapse all), the commit panel lists linked threads (`ThreadFilter.commit`), **Copy as Markdown** (`ReadThreadMarkdown`) and **Copy Commands** exist. Gaps: the commit panel has no "created during this thread" mark and no exact "and N more" (the summary carries no link origin and `total` is the whole store); Expand all opens only the groups loaded at the click; none of it was seen in a window.
- The prompts of a session appear when the next event of the agent arrives, because the adapter reads prompts only from the session file (D130).

## Slice 6g: thread graph (planned 2026-10-31)

Decided by the author on 2026-10-31 as proposed decisions D143 to D146, with Q34 open. The design, schema, command table and sub-slices are in `design/threads.md`, section "Graph, provenance and labels"; the behavior is in `docs/guide/threads.md` and `docs/reference/mcp-tools.md`, marked `Planned`. Pillars 2 and 3: a range of code explained by the sessions that wrote it, and labels and links an agent can record without writing thread text.

**Goal.** Ask why a line range is the way it is and get the commits, threads, edits and labels behind it, each with the evidence that ties it to the code. Let agents record labels and links, under the human's control.

**Scope**

0. **S0, agent thread search (D147), before P1.** Widen `ThreadFilter` (`role`, `thread`, turn range, `model`, `edited_only`, path glob, relative times), cursor paging and `context` in `SearchThreads`, thread metadata in each hit, `facets`, the helpful errors, then the MCP schema and description of `threads_search`. The `related_to` filter comes with P3. Real-git tests with a scripted `threads.db`; the MCP stdio test covers each filter. Done when an agent can answer "threads that edited this path in the last week" with no query, and a broad query is narrowed with facets in one more call. **E1 done (2026-10-31):** the engine filter takes `role`, `thread`, `turn_from`/`turn_to`, `model`, a `path` glob and `parse_when`; known gaps: no `edited_only` (`has_edits` is the one implementation), `role` excludes edit-path hits (their entry is a tool call), the glob's `*` crosses `/`, and listing ignores the entry filters. **E2 done (2026-10-31):** cursor paging, `group` (thread or entry), `order` (relevance or recent), `context` (0 to 3), `facets` (branch, agent, path), the zero-hit `diagnosis`, the hit metadata (agent, branch, `started_at`, `edit_count`, labels), an optional query for listing, every filter of the complete form except `mode` and `related_to`, and the MCP schema and description of `threads_search`; known gaps: no `mode` (exact or approximate), no `related_to` (with P3), the UI does not use the cursor, `context` or facets yet (E3), and the E1 gaps still open: `role` excludes edit-path hits (their entry is a tool call), the glob's `*` crosses `/`, and listing ignores the entry filters.
1. **P1, provenance engine.** Blame, blob and line matching, the ambiguous and unverifiable states, the bounds. Real-git tests. Measures N and closes Q34. Done when each real-git fixture (blob match, line match, ambiguous, formatter after the edit, rebase, squash, reorder, redacted and truncated edit) returns the evidence the docs promise, and a range is answered within the budget in the release build.
2. **P2, MCP provenance tool.** `provenance` over the engine, untrusted-marked and bounded. Needs slice 7 for the server. Done when an agent asks about a range and gets the thread, its entries and the evidence, and a cut result says so. **P2 done (2026-10-31):** `provenance` takes `path` (repository-relative; an absolute or `..` path is refused with the reason), `from_line` and `to_line` (default `from_line`). It returns the commits with author, date and subject, each with its threads, evidence strongest first, `ambiguous`, the matching edits with `unverifiable` and the entries around each; all text is untrusted. `complete` and `cuts` say when a bound cut the answer. Tests in `crates/gm-mcp/tests/stdio_real_git.rs`: a blob match, a line match, an ambiguous pair, unverifiable edits, the 5-thread cut, bad arguments and the schema. Known gaps: a matching edit has no `change` (the engine's `ProvenanceEdit` does not carry the edit's change kind; adding it is a `gm-schema` change, not made here); labels and linked threads are not in the result (P3); `Session::run` drains the provenance job inline, so a provenance read is not cancellable through MCP; Q34 is open, so `LINE_MATCH_MIN` is the placeholder; nothing was seen in a window.
3. **P3, labels and links.** Schema v5 with its migration from `thread_tag`, the commands, commit links that survive a restack and resolve after an outside rewrite on an exact patch match only, MCP `label_create`, `label_attach` and `thread_link`, and the allow rules the docs recommend (the plugin cannot ship permission rules). Done when the migration keeps every tag as a label, the rewrite cases behave as the docs say, and a row an agent wrote shows the agent and can be removed.
4. **P4, UI.** The Labels page, chips, label dialogs, the provenance view for a range on the Code page, link threads from the header and the actor on each agent row. Done when a range can be asked about and its evidence read in a window, and the screens are recorded.

**Done when:** the whole of P1 to P4 meets the docs, `make docs-status` shows no `Planned` marker left on the pages of this slice, and D143 to D146 are accepted or superseded by the author.

**Status (2026-10-31).** S0, P1, P2, P3 and P4 built. P4 gaps are listed in `design/threads.md` (P4: UI) and `design/screens.md`: no attach of a file, commit or label from the app, rewrite states missing in the thread's Commits panel, the narrowest width unchecked, and the viewer menu's reason text overflows. Q34 is open. The Labels page, the New Label dialog and the provenance panel were seen in a window on 2026-11-01.

## Slice 7: MCP

Redirected by the author on 2026-10-31 (D135): `gitmaster mcp` is a second process over stdio, with no socket, token or app-side server. It runs its own engine on the repository and shares only the `.git/gitmaster/` files with the app. Its first tools are the ones an agent does not already have: review comments and threads. No grep, file read or symbol tool (D48 is superseded).

- **7a, stdio server with review and thread tools** (done 2026-10-31, D135). `gitmaster mcp [--repo PATH]`; tools `review_comments_list`, `review_comment_resolve` (sets the new `handled` status with a summary, review schema 2), `threads_search`, `thread_get`, `thread_read`; the actor is `agent:<client>` from `initialize`; results mark person- and session-written text `untrusted`. Tests: JSON-RPC over a real repository and real stores (`crates/gm-mcp/tests/stdio_real_git.rs`).
  - Known gaps: the app does not watch `review.db`, so a comment an agent marks handled shows when the Local review page is next listed, not live; the `handled` summary is not shown in the app yet; the plugin's skill is short and was not tried in a session (Q26); `since` takes milliseconds; nothing was seen in a window.
  - Claude Code does not document the directory a plugin's stdio server starts in, so `gitmaster mcp` falls back to `CLAUDE_PROJECT_DIR` when the current directory is not in a repository; neither was seen in a session.
  - Also in 7a: the Claude Code plugin carries `.mcp.json` (`gitmaster mcp`, one process per session) and a `gitmaster` skill; parallel servers on one repository, in several worktrees, are tested, including a race on the first creation of the stores, and `set_handled` is one statement so a resolved comment is never overwritten.
- **7a2, the "after" snapshot of a handled comment** (done 2026-10-31, D140). `HandleReviewComment` takes a snapshot of the working tree (`refs/gitmaster/review/<id>/after`, review schema 3); the read `LoadReviewChange` maps the anchored lines from the "before" snapshot through the file's `-U0` hunks to the "after" one; `review_comment_resolve` returns it as `change`, and the Local review page's After side shows it for a handled comment, numbered where the snapshot puts it. Built with an orchestrator and three workers (engine, MCP, UI). Tests: pure and property tests of the mapping, real Git in `crates/gm-core/tests/review_real_git.rs`, JSON-RPC in `crates/gm-mcp/tests/stdio_real_git.rs`.
  - Known gaps: "other changes" (hunks that overlap no comment) are not shown; the agent's summary is not shown in the diff; the app does not watch `review.db`; the snapshot's cost in release was not measured; the After captions ("As it was when the agent marked it handled", "The commented text was not in the file when the agent handled it") were worded by the agent; nothing was seen in a window.
- **7b, propose and apply tools** (not started). Approval is the harness's (D137); open: Q33 (clients that cannot enforce it). `stack_tree`, `restack_plan`, `restack_apply`, `conflict_*`, `workflow_list`, `workflow_run`, `operation_log`, the agent policy, and the settings section.

Done when: an agent can read the stack, propose a restack and apply it with approval (7b).

- **7b2, comment on any code** (built 2026-10-31, D141). A line pick separate from the staging selection: drag across line numbers or Shift-click, in the Working Copy diff (any line, both sides), the Code viewer and the commit, stash and reflog diffs; Ctrl+Shift+M or the header button opens the composer. Where the surface is the working tree (unstaged side, Code viewer) the comment is by line numbers; elsewhere the engine command `AddReviewCommentByText` finds the picked text exactly once in the file or refuses. Tests: pure (`review::picked`, the registry rule, the composer target), real Git (`review_real_git.rs`: once, twice, absent, agent refused).
  - Known gaps: comments already written are not drawn inline in the Code viewer or in commit and stash diffs (`Planned` in `docs/agents/local-review.md`); no autoscroll while dragging past the edge of the list; the commit-diff flow and Ctrl+Shift+M were not tried in a window (no key reaches the app under Xvfb without a window manager), the drag and the header button were, in the Working Copy diff and the Code viewer.

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

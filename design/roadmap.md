# Roadmap

The order of the work. Status marks: **(done)** in the code and verified; **(to do)** not started; no mark = future slice. The order prioritizes reaching something the author uses daily and that already shows a differentiator. The documentation in docs/ is the desired state; this file orders the work toward it, and `make docs-status` lists what the docs still mark as planned.

Last updated: 2026-11-11 (slice 6h, code health, planned, D225; before that, 2026-11-10: slice S5 of 8s done, D221; before that, slice S4 of 8s done, D220; before that, slice 8s S1 to S3 done; slice 8r is replaced by slice 8s, D219; before that, 2026-11-07: M1 to M3 of slice 6m done, path unification done (D197), review reply threads (D198) and sub-agent threads (D199) recorded; before that, slices 6s, 6m and 8r planned, D187 to D190; before that, N5 removed, D163; before that, next slices ordered: 5f restack conflicts, WF3, N5; before that, slice 6t, threads, planned and refined; before that, slice 3c, N4 syntax highlighting built; before that, slice 1f, move a file: M5 to M7 built, M4 not done). Initial proposal, to be iterated on.

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
   - **W9, shell commands** (done, D172; checked with real Git in `crates/gm-core/tests/worktree_cli_real_git.rs` and through the binary in `crates/gitmaster/tests/worktree_cli.rs`). `gitmaster worktree list|branches|path|add|remove|include`, with `.worktreeinclude` shared by symbolic link or reflink. The author's tmux scripts call it; nothing in the app changes. Known gaps: no `prune` or `lock` from the shell; the worktree dialog does not read `gitmaster.worktree.root` or `.worktreeinclude`; a Windows build has no links.
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

**Next step (author's decision, 2026-11-01):** the order is (1) **resolving a restack conflict** (slice 5, sub-slice 5f; done), (2) **WF3** (slice 3w, sub-slices in D155; done), (3) ~~N5~~, removed by the author on 2026-11-02 (D163); then (4) **WF4** (author, 2026-11-02; sub-slices in D164; done), then (5) **`transform.conflict_hunk`** (author, 2026-11-02; D168). H6 is done in slice 4c; H7 and H8 stay postponed. The "How to continue" note below is from 2026-10-25 and describes N5 as it was then.

### How to continue (end of the 2026-10-25 session)

**Where the code is.** Everything is on the working branch of the session. Before this session: H5 (lane graph), spikes S-A to S-C, N1 to N3. This session added N4 (syntax highlighting, D111). The agent did not see any of it in a window, because the container has no display. `make lint`, `make docs` and `cargo test --workspace` are the checks; there are no new smoke scenarios (D88).

**Next work (superseded: N5 was removed by D163 on 2026-11-02): N5, symbols and the single box.** It reuses the tree-sitter of N4 and the proposals of D92: definitions by tree walk where the language allows it (Rust, Python, Go) and by query for TypeScript and JavaScript, an index in `.git/gitmaster/symbols.db` keyed by blob, language and query version, and the single search box of D50. D45 stays Proposed for the symbol index: confirm it with the author, and agree the "Done when" of N5 in `design/code-navigation.md`, before writing code.

**Owed after N4:**

- Look at the colors in a window, in both themes, on real files of each language, and tune the queries in `crates/gm-core/queries/` where a role looks wrong. A change there is a change of product: keep the tests of `syntax.rs` green and add a sample for a fix.
- Measure the binary growth and the build on macOS (`measure.sh` of the nav-b-treesitter spike, git history, 7932c61).

**Minor pending items, by value** (all details are in the known gaps of each sub-slice):

- Quick Open: the list of new files (`--others`) takes about 0.75 s on 100,000 files. A two-phase list (tracked files first, new ones after) fixes it. It is reread only when Quick Open opens.
- Search: a case-insensitive match on a character whose lowercase has another byte length (U+212A KELVIN SIGN folds to ASCII `k`) puts the mark at the wrong byte offsets; the property test `grep::tests::a_shaped_line_is_valid_text_with_marks_inside_it` finds it now and then (seen 2026-11-01). Not fixed.
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
- **Script editor (D149, D151, D152), built with known gaps.** Built: the Script tab is the component's code editor, with Lua highlighting, line numbers, no soft wrap and two-space tabs; the window root is `Root` (D152), with the frame moved back over its padding; a new editor per open file, so undo starts with the file; dirty tracking and Save from its change events; Dry Run on its text; go to a problem's line; and completion of `ctx` after `ctx.`, `{` and `,`, with the typed word highlighted. Known gaps: the problems are not underlined in the text (the list under the editor stays; the check answer has no hook into the editor); the frame cancel was seen only under Server decorations (Xvfb has no window manager), so the Linux Client path (rounded frame, resize band, Root shadow) is not seen; Tab focus cycling and Esc were not checked by eye; completion of argument-table keys and nested groups is covered by unit tests, not seen in a window; `scenario_w_script_editor` was not run; the release-build timing and size with the grammar and the component are not measured. Done when: the Script tab is the code editor with highlighting and completion, undo and Save and Dry Run behave as before, and `scenario_w_script_editor` passes under Xvfb.
- **WF3, event triggers, waits, phases and migration of hooks (D75, D98 to D101), now on scripts and after WS0 to WS2.** Covers:
  - engine events as triggers, `match`, debounce, and concurrency (in memory, D100);
  - automatic approval of event `apply` steps, tied to the reviewed script hash (D101), and `Stale` reconciliation;
  - `ctx.wait` for events such as `restack.resumed`, while the app is open;
  - `before.<operation>` phases (a gate, with `ctx.block`), `transform.<field>` (the returned value) and `after.<operation>` (observation);
  - parity of `transform.commit_message` with `prepare-commit`;
  - the action "Convert hooks to workflows", which writes the equivalent Lua scripts into the user's workflow directory;
  - GitMaster stops reading the `[gitmaster "<hook>"]` sections of `.gitconfig`.

  Done when: the behavior of `prepare-commit` is reproduced by a workflow and the hook code path is removed; a workflow on `commit.created` runs; the `apply` step waits for approval; editing the file sets the hash back to "not reviewed" and revokes automatic approval; closing the app in the middle of a run leaves it `interrupted` on the next open (D100); a run that calls `ctx.wait("restack.resumed")` stops and resumes when the event arrives, while the app is open.
  Sub-slices (D155, author, 2026-11-01), in this order, each engine first with real-Git tests, then the UI, then `make check`:
  - **WF3a, event triggers (done on 2026-11-01, with known gaps; D156).** The engine publishes `repo.opened`, `commit.created`, `checkout.done`, `fetch.done`, `restack.done`, `refs.changed` (watcher, debounced) to a dispatcher that matches `trigger.on` and `match`, applies `concurrency` in memory and starts `event:<name>` runs with `input.trigger` payloads; an operation a run performs carries its run id and triggers nothing. Done when a `commit.created` workflow runs after a commit from the window, a `refs.changed` workflow with `match` runs once per burst and only for matching branches, a commit made by a workflow triggers nothing, and each concurrency mode behaves as the docs say.
    - Checked: 19 unit tests of the matcher, the payloads and the concurrency machine; 12 real-Git tests (`script_triggers_real_git.rs`: commit, checkout and repo-opened runs with their payloads, `match`, unreviewed notice, switch off, one `refs.changed` per branch with old and new, no cascade from a run's own commit and ref change, queue, skip, replace before and after an applying call); a mutation check of the no-cascade snapshot.
    - Known gaps: `restack.done` and `fetch.done` are wired but have no end-to-end test; the watcher's debounced path is covered by the path filter's unit test, not by a test that writes a ref under a running watcher; the unreviewed notice stands in for the waiting run of WF3b; the D156 limits (an outside change absorbed after a run's write, shared operation ids of queued runs); nothing seen in a window.

**A2** (done 2026-11-07, with gaps): MCP `review_comment_create` and the `author` argument and `author`, `kind`, `severity`, `needs_answer` fields on `review_comments_list`, with six JSON-RPC tests on real Git. Known gaps: the create result is the agent's newest thread on the path by highest id, which two agent clients creating on one path at once could mix up; the 20-per-repository limit is tested in the engine, not over MCP; the plugin skill does not name the tool yet (A4); no UI (A3).

**A3** (done 2026-11-07, with gaps): pure chips model (`review::chips`, kind and severity chips, **Agent-written**, **Needs answer**, Dismiss for a note), the **Needs answer** and **Agent** filters with counts, and the chips on the Local review page and in the Working Copy diff cards. **A4** (done, plugin 0.2.0): the `inbox` skill and the `gitmaster` skill update. Known gaps: nothing was seen in a window and no smoke scenario covers Local review; agent threads are not drawn in the Code viewer or commit and stash diffs (the existing planned item); the skills were not run in a live session (Q42).
  - **WF3c, gates and observers (done on 2026-11-01, with known gaps; D157, D158).** `before.*` and `after.*` on `commit`, `restack`, `branch_create`, `conflict_resolve`; the operation waits without freezing the UI; a block, an error or a failure stops it; `after` failures warn. Done when `before.restack` with `ctx.block` stops a restack with its reason in the dialog, a failing gate stops a commit, and `after.commit` sees the new commit.
    - Checked: 17 unit tests of the operation mapping, payloads, order and veto texts; 15 real-Git tests (`script_phases_real_git.rs`: block, error, two gates in order, pass, unreviewed, `before.restack` with refs unchanged, `before.branch_create`, `after.commit` and `after.restack` payloads, a failing observer only warns, cancel while a gate waits, no gate for a run's own commit, switch off), stable over repeated runs; UI model tests for the waiting text, Cancel gates and a cancel that is not an error. Seen in a window (release build, Xvfb, light): a blocking `before.commit` gate shows its reason in the status bar with no commit made; a waiting gate shows "Waiting for workflow gates…" with **Cancel gates**, and cancelling leaves no commit and no error.
    - Known gaps: a phase workflow cannot be reviewed from the window yet (the review flow serves manual workflows; WF3b), so the window check marked it reviewed in the store; the conflict-resolve gate and the gate on resolving a restack conflict have no real-Git test; the composer's run line reads "cancelled: cancelled" after a cancel; the restack, branch and resolver waiting texts were not seen in a window; the dark theme was not seen.
  - **WF3d, transforms (done on 2026-11-01, with known gaps; D159).** The `composer.opened` trigger (moved here from WF3a: it fires at the same moment as the transform and needs the composer UI); `transform.commit_message` (Suggest Message, and an empty message box getting focus with something staged) and `transform.branch_name`, in sequence by file name, with the Replace/Discard reconciliation. Done when it does everything `prepare-commit` does.
    - Checked: unit tests of the fold and of the window's routing, fill rule, once-per-staged-state rule and cancel target; 17 real-Git tests (`script_transforms_real_git.rs`: order, nil, failure skipped, unreviewed, `changed` after staging, `branch_name`, `composer.opened`, cancel, supersede, switch off, agent refused) stable over repeated runs, including two parity tests that give the same message as `prepare-commit` for one program and for a chain. Seen in a window (release build, Xvfb, light): clicking into the empty message box with a file staged fills the message; with typed text, Suggest Message shows the suggestion with Replace and Discard, and Replace takes it; Suggest Name in New Branch fills and suggests the same way; a waiting transform shows **Cancel suggestion**, and cancelling leaves the box empty with no error.
    - Known gaps, against `prepare-commit`: an unreviewed transform shows only a notice, with no review control in the composer (WF3b); nothing tells the person when no transform is loaded; the per-step hook log (command hash, exit code) has no equivalent beyond the run journal. Also: the once-per-staged-state rule and the New Branch cancel were not seen in a window; the run line reads "cancelled: cancelled" after a cancel; the window check needed `xdotool windowfocus` because bare Xvfb has no window manager.
  - **WF3e, hook migration (done on 2026-11-01, with known gaps; D160).** Convert Hooks to Workflows; stop reading `[gitmaster "<hook>"]`; remove the hook code, `RunPrepareCommit`, the hook approvals and their settings rows; D04, D16, D30 superseded. Done when no hook code path is left and a converted `prepare-commit` gives the same message.
    - Checked: 9 real-Git tests (`convert_hooks_real_git.rs`: the converted script gives the hook's message, config bytes unchanged, a second convert writes `-2`, invalid and repository sections skipped with their reason, a chain of two gives one script, the `match` guard, a failing command gives no value, agent refused, both scopes listed); the WF3d parity tests now compare against literals; the smoke scenario `x-convert-hooks` passes in both themes (the light screenshot was read). Removed: the hook module, its schema types, `RunPrepareCommit`, `CancelHook`, `ListHooks`, `ApproveHook`, the hook events, `hook-approvals.json`, the hook UI and the smoke scenarios n, o, p and q (about 3,200 lines). Suggest Message runs only transforms and is disabled with a reason when none is loaded.
    - Known gaps: the D160(4) limits; the window checks the removed smoke scenarios gave the suggestion flow are not rewritten for transforms (WF3d was checked by hand); the dark screenshot of the convert dialog was not read.
  - **WF3b, review and automatic approval of event runs (done on 2026-11-01, with known gaps; D161).** The grant in "Run this version?", the non-modal notice for an unreviewed event run, Approve and Cancel on the Workflows page. Done when an edit revokes the grant, a granted event run applies and an ungranted one pauses, seen in a window.
    - Checked: 11 real-Git tests (`script_review_real_git.rs`: the request instead of a run, review starts the run for its commit, latest commit wins, dismiss, the grant applies without asking, no grant pauses until approved, an edit asks again and drops the grant and the old request, revoke, an unreviewed observer waits, an unreviewed gate vetoes and the same commit goes through after review); unit tests of the dialog, the pending model and the page's buttons. Seen in a window (release build, Xvfb, light and dark, reviewed through the UI, no hand-written rows): commit → "wants to run" with Review and Dismiss → **Mark Reviewed** without the grant → the run pauses at its apply call → Approve on the Workflows page → done, starter `event:commit.created`; a gate's veto with **Review** → reviewed → the same commit goes through; Revoke brings the request back; Dismiss runs nothing.
    - Known gaps: the grant applying without asking and an unreviewed transform's notice were not seen in a window (tests cover them); after a run creates a branch, the sidebar does not refresh until something else reloads it (older, seen here).
  - **WF3f, waits and stack events (done on 2026-11-01, with known gaps; D162).** `ctx.wait` resumed by engine events while the app is open; `stack.parent_moved` and `stack.parent_merged` (4d). Done when a run waiting on `restack.resumed` resumes on the event and moving a parent fires `stack.parent_moved` with its payload.
    - Checked: 9 unit tests of `stack_transitions`; 8 real-Git tests (`script_waits_real_git.rs`: a finished resolution resumes a waiting run with its branch and commit, a cancelled one aborts it, starting and resolving does not wake it, a timeout still times out, opening a repository with a stale branch fires nothing, a commit on the parent fires `stack.parent_moved` once, a deleted parent fires `stack.parent_merged`, a run's commit on the parent fires nothing) stable over three runs; a mutation check of the no-cascade baseline. Engine only; nothing seen in a window.
    - Known gaps: only `restack.resumed` can be waited on (the catalog's one wait event), so waking by other names is covered by the runtime unit test; WF4's restack calls are still needed for the stack templates to act.
  - **WF3 status (2026-11-01):** WF3a to WF3f done. The "Done when" of WF3 is met: `prepare-commit` is reproduced by a transform and the hook path is removed; a `commit.created` workflow runs; apply steps of event runs wait for approval; an edit revokes review and automatic approval; a run interrupted by closing the app is `interrupted` (D100, from WS0); a run waiting on `restack.resumed` resumes on the event. Open: `transform.conflict_hunk`, the WF4 calls, agent runs (WF5).
  - After WF3, on its own: **`transform.conflict_hunk` (done on 2026-11-02, with known gaps; D168).** Script suggestions on the resolver's hunk cards.
    - Checked: 3 + 12 unit tests (the trigger built from a parsed hunk; the resolver model: offered on its hunk only, choosing resolves with origin script, Clear keeps the option, another read or file ignored, free edits keep suggestions); 11 real-Git tests (`script_hunks_real_git.rs`: one suggestion per hunk, the trigger's fields, fold in file order, nil for one hunk, unreviewed asks once, switched off, a new read cancels a waiting script with no late answer, resolving cancels, triggers off, the restack site, the script origin in the operation log) stable over three runs; a mutation check of the cancel. Seen in a window (release build, Xvfb, light and dark): the review request in the resolver, review through the UI, reopened file with a suggestion on each card, one chosen and one Left with their origins, Apply, and `git diff --cached` with the suggested text.
    - Known gaps: a long suggestion's Show/Hide, a suggestion kept across a free edit, the restack-site card and two folding workflows were not seen in a window; cancelling by `CancelOperation` of the read has no test and no window command sends it; switching screens away from the resolver does not cancel runs.
- **WF4, stack actions and templates (with slices 4 and 5).** `ctx.stack.*`, `ctx.restack.*`, `ctx.reparent.*`, `ctx.push.lease`, the events `stack.parent_moved` and `stack.parent_merged`, and three built-in templates, in Lua. Done when: turning on "Restack children when parent moves" restacks the subtree after a commit on the parent, with a plan, undo and a pause on conflict, and with no code specific to this policy.
  Sub-slices (D164, author, 2026-11-02):
  - **WF4a, engine calls (done on 2026-11-02, with known gaps; D165).** `ctx.stack.tree`, `ctx.stack.stale`, `ctx.fetch`, `ctx.restack.plan`/`apply`/`undo`, `ctx.reparent.plan`/`apply`, `ctx.push.lease` carried out by the engine with the catalog's outcomes (`stale`, `conflict`), over the same commands as the window. Done when each call has a real-Git test, including `apply` ending `stale` after a branch moved and `conflict` with the unblocked branches moved.
    - Checked: 7 planner unit tests of the parent override, 7 unit tests of the plan book and values; 13 real-Git tests (`script_stack_calls_real_git.rs`: tree and stale, plan and apply logged under the run, `stale` after a branch moved and for an unknown id, `conflict` with the clean sibling moved, undo by id, reparent with undo restoring the old parent, fetch, push asks every time and fails when the lease is broken, an event run waits before it restacks, a read answered at once during a slow fetch, cancel during a fetch stops git) stable over three runs; mutation checks of the id compare, the old-parent restore and the kill.
    - Known gaps: a stopped git's process record is not written to the command log; killing `git fetch` leaves its helper (`upload-pack`, or `ssh` for a network remote) to exit on its own; the push cancel path has no test of its own; after a workflow restack the window's restack dialog does not offer Undo until it is reopened (WF4c); nothing seen in a window.
  - **WF4b, per-repository switches and built-in stack workflows (done on 2026-11-02, with known gaps; D166).** Done when a switched-off workflow does not fire in that repository, a built-in one is off until switched on, and "Restack children when parent moves" restacks the subtree after a commit on the parent with no code specific to this policy.
    - Checked: unit tests of the switch store, the filter, the built-in listing and the copy naming; 9 real-Git tests (`script_builtins_real_git.rs`: a switched-off workflow does not run here and runs elsewhere, a built-in is off until enabled, "Restack children when parent moves" with the grant restacks `feat` after a commit on `main` with no policy code, without the grant it pauses at `restack.apply`, on a conflict it waits and re-plans after the resolution, "Restack stale stacks after fetch", "Reparent to grandparent", Copy to Edit).
    - Seen in a window (release build, Xvfb, light; 2026-11-02): the Built-in group, the Switch On dialog with the grant, and, with `feat` already stale, a commit on `main` from the window restacking `feat` by itself (badge gone, run done, started by `event:stack.parent_moved`; `git log` and the base confirm it). The first window check found that `stack.parent_moved` never fired for a branch already stale; the rule was revised (D162) and two real-Git tests reproduce that case.
    - Known gaps: the built-in's notice reads "Restacked 1 branches" (no singular); the Workflows settings fields wait for the settings dialog (slice 9); the dark theme was not seen.
  - **WF4c, workflow restack in the window (done on 2026-11-02, with known gaps; D167).** The approval of `restack.apply` shows the plan; a conflict in a run notifies and opens Resolve conflict; the run resumes on `restack.resumed`; Undo is offered at once.
    - Checked: 12 UI unit tests; 3 new real-Git tests (the approval carries the plan, a conflict in a run is resolved from its notice and the run re-plans and applies, a workflow restack reports its record and Undo restores it), the file stable over three runs. Seen in a window (release build, Xvfb, light): the approval band listing the held-back branch and its conflict, the notice with Resolve conflict, the dialog and the resolver with the restack labels, Continue restack, the run's apply, the dialog re-planning to "up to date" with Undo restack, Undo.
    - Known gaps: after **Undo restack**, `stack.parent_moved` fired at once and an automatic workflow restacked straight back: fixed (an undo is a quiet baseline, D162; a real-Git test fails without the fix), not re-seen in a window; reparent approvals use the same path but have no test and were not seen; the stale retry of the built-ins has no direct test; the dark theme was not seen.
- **WF5, MCP (with slice 7).** `workflow_list` and `workflow_run`. There is no tool to create or edit scripts (D102). Done when: an agent lists the workflows and fires one; the `apply` steps of the run wait for the human's approval in the UI (D64(c)).

## Slice 3c: code navigation (in progress: S-A, S-B, S-C, N1, N2, N3 and N4 done; N5 removed by D163; N6 to do)

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
    - the Code page's search field and Quick Open are separate boxes (by design since D163).
- **N4, syntax highlighting** (done on 2026-10-25, D111; no smoke scenario, per D88; nothing seen in a window).
  - The viewer colors Rust, Python, Go, TypeScript with TSX, and JavaScript (widened on the same day by D112 to Java, C, C++, C#, Ruby, Bash, JSON, TOML, YAML, HTML and CSS, and by D113 to Clojure), in twelve roles with palette tokens in both themes (design/visual-identity.md). The text shows first and the colors follow in a `FileHighlighted` event; a file in another language, a binary and a missing file stay plain.
  - D112 samples: one test per added language checks its roles, a test checks every compiled-in query compiles and names only known captures, and one checks the path-to-language table. Not done: the timing and binary size of the added grammars, Markdown, SCSS and PHP.
  - Built: pure module `gm-core/src/syntax.rs` (language by extension, capture name to role, `highlight(language, text, stop)` with a 2 s budget checked by the parser and the query cursor, long lines left plain), the product's queries in `gm-core/queries/`, one Cargo feature per grammar, `HighlightJob` on its own thread stopped by a newer read, `SyntaxRole` and `HighlightSpan` in `gm-schema`, `Shown.spans` and `styled()` in `gm-ui/src/code_page.rs`, and the painter in `shell_code.rs`.
  - Checked: 20 unit and property tests of the module (samples per language, span order and character boundaries, determinism, stop, budget, long lines, multibyte, syntax errors, every capture of every query has a role), 4 engine tests (text before colors, no job for plain files, a newer read stops and drops the colors), 8 model tests for the pieces of a line (scroll, tabs, spans from earlier lines, the colors of a file no longer on screen are dropped), and 3 theme tests (4.5:1 on four backgrounds in both themes, distinct colors, one per role). Release build on Linux: 2 MiB of Rust (58,000 lines) in 0.9 s, first-use query compile 37 ms (`cargo test --release -p gm-core --lib syntax::tests::timing -- --ignored --nocapture`).
  - Known gaps: nothing was seen in a window, so the actual look of the colors and the legibility on a real screen are unchecked; the binary growth (about 5 MB) and the macOS and Windows builds were not measured; the whole file is parsed on every read (no incremental parsing, the viewer is read-only); there is no text selection, so the `selection` background is checked in theme tests only; the queries are the crates' seeds with a few corrections, not yet tuned on real code (for example Rust `self` and attribute items, Python decorators, TypeScript generics), and an unlisted capture name is silent (a test lists them); `.d.ts` files go through the TypeScript grammar, and no extensionless file (a shebang script) gets a language; highlighting in the diff is separate (D42); injections (code in markdown) are out; Markdown, SCSS and PHP have no grammar yet.
- **N5, symbols and single box: removed by the author on 2026-11-02 (D163).** Nothing was built.
- **N6, revisions and performance gate** (the MCP Read tools were dropped by D135).

Done when: opening a file by name and finding text take seconds in a 100 thousand file repository, without freezing the UI.

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

## Slice 5: restack (in progress: 5a to 5f done, with gaps; push with lease and the restack phases to do)

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
- **5f, resolve a restack conflict (done on 2026-11-01, with known gaps; D154, decided by the author on 2026-11-01 as "temporary worktree", details proposed).** Closes the "Done when" of this slice and of slice 6. Sub-slices, each with `make check`:
  - **5f-1, engine: resolution worktree and store.** `GitPort` opens the resolution worktree at `onto` with the conflicting commit cherry-picked (rerere on), builds the resolved commit with `commit-tree` (metadata as D142), and removes the worktree; the store `.git/gitmaster/restack-resolutions.json` with its `refs/gitmaster/restack/resolved/<oid>` refs; worktrees under `<git common dir>/gitmaster/` hidden from `worktrees()`. Done when real-Git tests show the stages match the plan's conflict paths, the main checkout (even dirty) is untouched, the resolved commit keeps the metadata, and a removed worktree leaves nothing in `git worktree list`.
  - **5f-2, engine: commands and plan integration.** `StartRestackResolution`, `FinishRestackResolution`, `CancelRestackResolution`; `site` on the three conflict commands; `RestackBlock::Conflict` carries `onto`; `ReplayStep::Resolved`; the replay consults the store; apply clears the entries it used; a leftover worktree is removed on `ResumeRestack`. Done when a real-Git test resolves a conflict end to end through the engine (start, resolve with `site: restack`, finish, plan shows `resolved` and the children plan, apply moves every ref), a moved parent ignores a stale resolution, a second conflict in the same branch is resolved the same way, cancel leaves no worktree, and an agent is refused.
  - **5f-3, UI.** **Resolve conflict** on a conflict row of the restack dialog; the resolver in restack mode (header with branch and commit, the list of conflicted files, **Continue restack**, **Cancel**); a **Resolved** step in the plan; the `Planned` markers in `docs/guide/stacks.md` and `docs/guide/conflicts.md` removed. Done when the pure state tests pass and the flow is seen in a window under Xvfb, in both themes.
  - Checked: 5f-1 six real-Git tests (`restack_resolution_real_git.rs`) and four store unit tests; 5f-2 eight real-Git tests in `restack_real_git.rs` (end to end through apply with a dirty checkout untouched, a stale resolution after the parent moves, two conflicts in one branch, cancel, agent refused, stale plan id, no conflict, another repository dropping the session) and three plan unit tests; 5f-3 thirteen unit tests of the dialog and resolver state. Seen in a window (release build, Xvfb, light and dark): Resolve conflict, the restack header and role labels, Apply, Next file across two files, Continue refused while a file remains, Cancel back to the same plan, Continue to a plan showing Resolved, Restack, and Git confirming the branch moved, the base was written, the checkout was unchanged and the resolution worktree was gone.
  - Known gaps: a resolved commit is still replayed once to read its subject (wasted work, harmless); the rerere-resolved-everything path, a refused Start and a refused Finish are covered by unit tests only; the palette guard (a working-copy Resolve Conflict refused while a restack resolution is open) was not tried in a window; the side labels are built in the adapter, since the port has no commit-subject read; the dialog shows Resolved per branch, not per commit; the plan's `merge-tree` still does not use rerere, so a recorded resolution shows only once the resolver opens; the hunk card's **Clear** is clipped at the pane edge (older); no smoke scenario in `scripts/ui-smoke.sh`; macOS not seen.
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
- The restack's **Resolve conflict** entry is built in 5f (D154); the plan's replay does not use rerere.
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

## Slice 6s: sharing threads (deferred 2026-11-07; D187, Q37, Q38, Q43; **not in the build order**)

The author decided the principles (D187: opt-in, additive, explicit, redaction check, untrusted imports) and then reopened the transport: a session of its own will explore SQLite-based alternatives to Git (Q43) before anything here is built. Until then there is no user documentation for sharing, and the sub-slices below are the Git-ref candidate, kept as a reference. Engineering design in `design/threads.md` ("Sharing").

**Goal.** Publish chosen threads as one Git object each to a remote the user picks, and read the threads teammates published, with a redaction check before anything leaves the machine and ingest redaction on everything that comes in.

**Sub-slices (candidate, on hold).** SH1 format and checks (pure), SH2 publish and unpublish, SH3 fetch and import, SH4 UI, SH5 memories on the same channel. SH1 (the format, the hash and the redaction check) is transport-neutral and is the likeliest part to survive Q43.

**Acceptance.** A test against a real bare remote proves: publish, republish (still one object), fetch into a second repository, a tampered blob shown as **Modified**, a refused extra tree entry, unpublish, and that a run with sharing off creates no ref and makes no network call. Measure the size and time of Q38 on the real `threads.db` before SH2 is called done.

**Known gaps to expect.** No automatic publish (Q37); no sync of notes, stars or labels; nothing seen in a window until SH4.

## Slice 6m: memories (M1 to M4 done 2026-11-07; D190, D191, Q42, Q44)

Requested by the author on 2026-11-07, first priority. Engineering design in `design/threads.md` ("Memories") and `design/agent-skills.md` ("Skills and sub-agents"); behavior in `docs/guide/memories.md` (`Planned`). Depends on slice 6t and 6g (threads, labels, provenance).

**Goal.** Decisions, rules and lessons that agents propose and the human accepts, searchable by later sessions over MCP, with sources and a derived staleness signal.

**Sub-slices.** M1 model and store (pure, no UI), M2 MCP tools (`memories_search`, `memory_get`, `memory_propose`), M3 Memories tab, M4 skills (`propose-memories`, `prepare`; first verify the plugin format, Q42). M5, sharing of memories, is deferred with slice 6s. **Contexts (D191, Q44)** answer work across many repositories: C1 builds `contexts.db`, `repo-id`, membership, nesting and the merged search, after M1 and before M2 (the MCP search arguments carry the scope); C2 is the Manage Contexts… dialog with the scope chip and filter, built with M3 (as M3c). Order: M1, C1, M2, M3 with C2, M4; M1 to M4 are done.

**M1, model and store** (done 2026-11-07, with gaps; D192). Schema v7 of `threads.db` (`memory`, `memory_source` with a nullable `repo_id`, `memory_rel`, `memory_fts`); the pure rules in `threads/memories.rs` (text, duplicate key, sources, actor, state machine, **Files changed since**); the engine commands `ListMemories`, `ReadMemory`, `ProposeMemory`, `AcceptMemory`, `RejectMemory` and `DeleteMemory`, with accept, reject and delete human only and the reads kept out of the command log. Tests: pure unit tests in the module and `crates/gm-core/tests/memories_real_git.rs` (changed, renamed, deleted and added files, redaction, commit resolution, actor rules, the log). Known gaps: no MCP tool and no UI (M2, M3), so the docs pages stay `Planned`; the `model` column stays empty until M2 passes it; `ProposeMemory` has no `context` yet (C1); the duplicate check scans the memories of one kind on each write, which suits hundreds of memories, not tens of thousands; **Files changed since** runs one Git process per file source per result; a workspace member store still at v6 is skipped by the workspace thread listing until a writer opens it, as for every migration under D180. Nothing was seen in a window.

**C1, contexts store and visibility** (done 2026-11-07, with gaps; D191, D193). `contexts.db` in the app data directory with its own schema; `.git/gitmaster/repo-id`; pure `visible_contexts`, context paths, cycle refusal and scope names; the commands `ListContexts`, `CreateContext`, `UpdateContext`, `DeleteContext`, `AddRepoToContext`, `RemoveRepoFromContext`, `RenewRepoId`, `MoveMemoryToContext`, `AttachMemory` and `DetachMemory`; `ProposeMemory` with a context; `ListMemories` with a scope over the merged stores. Memories are addressed by uid. Tests: `crates/gm-core/tests/contexts_real_git.rs` on the author's shape (Platform of 7 repositories with Billing under it, Partners of 5, 13 real repositories): visibility, labels, the scope filter, merged order and paging, agent proposals in a context, move (and a store test of a move interrupted between its writes), attach and detach, a file of another repository read at its own `HEAD` and then unavailable, an unreadable `contexts.db`, a moved clone and a copied clone. Known gaps: no MCP tool and no UI (M2, C2), so `docs/guide/contexts.md` stays `Planned`; two writers are tested as two engines in one process, not two processes; a merged search reads every candidate's id and score before it pages, which suits thousands of memories; `ThreadStore::list_memories` and `ContextStore::list_memories_in` remain for their store tests only. Nothing was seen in a window.

**M2, MCP tools** (done 2026-11-07, with gaps). `memories_search`, `memory_get`, `contexts_list` (read) and `memory_propose` (record) in `crates/gm-mcp/src/memory_tools.rs`; they translate JSON to the engine commands, which enforce the rules. Memory objects are marked `untrusted`; `docs/reference/mcp-tools.md` lists them as implemented. Tests: `crates/gm-mcp/tests/memories_mcp_real_git.rs` over JSON-RPC on real repositories, with a temporary `contexts.db` (proposal and duplicate, a refusal that leaves the server answering, accepted-only by default, a read with a file changed since acceptance, two repositories joined by a context, paging). Known gaps: the `model` column stays empty, because the MCP handshake names the client and its version but no model; context names in `contexts_list` are not marked untrusted, since only the person writes them; the tools were driven by the tests, not by a live agent session. Nothing was seen in a window.

**M3, Memories tab** (done 2026-11-07, with gaps; split as the commits show). **M3a, view model** (432e20a): the pure `MemoriesPage` in `crates/gm-ui/src/memories.rs`, with filters and scope, proposed-first ordering, paging with late answers dropped by `OperationId`, the human action planners, and the Files changed since display; no GPUI. **M3b, the tab** (f2c9efb, with 87c3413 and 5ffb47c): the tab strip with the proposed badge, a virtualized list with filters and search, a detail pane with sources and Files changed since, accept, edit, reject, delete and supersede, bulk accept, New Memory, four palette actions, the scope chip and filter, and memory text shown as plain text. The proposed count on the badge is unfiltered (5ffb47c). **M3c, C2** (f84dd16): the Manage Contexts… dialog with its tree, a repository pane, adding and removing a repository from the recent ones or the open one, rename, move and delete from the ⋯ menu, and a clash prompt with a new id; the read command `PreviewDeleteContext` (D196), used by the delete confirmation; Move, Attach and Detach for memories in the detail pane and the palette. Known gaps: nothing seen in a window and no smoke scenario ran. `Memory.contexts` (the contexts the repository sees, by path) lets Detach list every attached context; it is also in the MCP `memories_search` and `memory_get` results. An agent proposing into a context has no UI beyond the generic accept. `docs/guide/memories.md` and `docs/guide/contexts.md` are `partial`, with the gaps marked there.

**M4, skills** (done 2026-11-07, with gaps). `ai/` commit 7fba9c6: skills `propose-memories` and `prepare`, and the `memory-proposer` sub-agent (`haiku`, five GitMaster tools, no shell or write). Known gaps (Q42): the skill and agent frontmatter and the namespaced sub-agent type `gitmaster:memory-proposer` come from the documented format, not from a live session; only the `mcp__plugin_gitmaster_gitmaster__*` tool names were seen in a session. Not tried against a real run. The other skills wait for `thread_summary_set` (summarize-thread) and slice 8r (review, tune-reviewers, dispatch).

**Acceptance.** An agent actor can only propose, with a source; only a human accepts; an accepted memory's file source shows **Files changed since** after the file changes, in a test with real Git. M4 is checked against a real Claude Code session, and what was and was not seen is stated.

## Slice 6q: memory quality (decided 2026-11-07; D201, D202, Q46)

Behavior to be written in `docs/guide/memories.md` and `docs/reference/mcp-tools.md` as `Planned` (D201 is confirmed). **Sub-slices.** Q1 `basis` on a memory (schema, `memory_propose`, search and detail, Memories tab). Q2 retrieval count and last-retrieved time, and the size signal in the tab. Q3 plugin: the `memory-consolidator` agent and `/gitmaster:consolidate`, the rewritten `memory-proposer` and `prepare` text with a brief cap. **Q1** (done 2026-11-08, with gaps; D201 (1)): `MemoryBasis` in `gm-schema`; `threads.db` v10 and `contexts.db` v3 with a checked `basis` column (old rows read `observed`); `memory_propose` takes `basis`, search and get return it and add a memory-level `note` when it is `unconfirmed`; the New Memory form has a Basis choice and the tab shows a chip for Tested and Unconfirmed (Observed shows none). Accept and edit keep the basis; the duplicate check ignores it; a move into a context keeps it. Known gaps: no basis filter; a person cannot change the basis of an existing memory; the old-row migration is tested on real SQLite in the stores, not in the real-Git files; nothing was seen in a window. **Q2** (done 2026-11-08, with gaps; D201 (4), (5)): `threads.db` v11 and `contexts.db` v4 add `retrieval_count` and `last_retrieved_at` (old rows read 0 and none); `memories_search` and `memory_get` send `RecordMemoryRetrieval` after they answer, for the memories they return, so the increment is a write outside the command log (D28) and a failed one is written to stderr while the read still answers; a moved memory keeps its count; `MemoriesListed` carries `accepted_total` and `accepted_characters` for the scope, which the tab shows as a line above the list ("12 accepted, 3.1k characters"); the memory detail says "Retrieved N times, last …" or "Never retrieved". No MCP result carries a count. Known gaps: the app does not refresh counts an agent changed until the list is loaded again; the size line ignores the filters and the query by design; no ordering by retrieval (not asked for); a memory held by both stores in an interrupted move is counted once; nothing was seen in a window. **Q3** (done 2026-11-08, with gaps; D201 (2), (3), (5)). Plugin in `ai/` at 0.3.0: the `memory-consolidator` sub-agent (`haiku`; the GitMaster read tools it uses and `memory_propose`; no shell, no write), the `consolidate` skill (`/gitmaster:consolidate`), `memory-proposer` and `propose-memories` rewritten to D201 (3) and to set `basis`, and `prepare` capped at 10 memories and about 2,000 characters. The consolidator's priority is `files_changed_since` first, then `unconfirmed`, then the oldest accepted memories, from the existing listing and filters; no engine change. The docs: `memories.md` and `agent-integration.md` with `/gitmaster:consolidate` no longer Planned. Known gaps: `memory_propose` has no `supersedes` argument (D201 (2) amended: the text names the memory); the consolidator cannot read current file text, since no MCP tool reads files, so it judges by `files_changed_since` and `provenance`; the listing is newest first, so reaching the oldest memories takes paging to the end; the plugin text, the skills and the sub-agents were not run in a live Claude Code session, and nothing was seen in a window; `plugins.json` keeps `min_gitmaster` at `0.0.0` (the app has no release version yet).

**U1, the engine and MCP of the usefulness mark** (done 2026-11-08, with gaps; D202 (1) to (5)). `threads.db` schema 12 and `contexts.db` schema 5 add `memory_mark`, an append-only history per memory and actor; `MarkMemory` is allowed to both actors on an accepted memory only, is logged with its actor and writes no row for a repeat of the actor's latest mark; `ReadMemory` answers the history beside the memory, and every `Memory` carries its latest mark per actor as `marks`; a move into a context carries the marks and a delete removes them. MCP: `memory_mark`; `marks` on `memories_search` and `memory_get` (the reason only in `memory_get`, untrusted); the `wrong` note, combined with the unconfirmed note when both apply. Tests: `crates/gm-core/tests/memory_marks_real_git.rs` (10, real Git), four MCP tests in `crates/gm-mcp/tests/memories_mcp_real_git.rs`, the migration and delete tests of both stores, and the pure rules in `threads::memories`. Known gaps: the app cannot mark or show marks yet; the plugin does not read or write marks, so the consolidator does not yet put a `wrong` memory first (D202 (3)); no filter by mark; the Memories tab does not show marks at all yet; nothing was run in a live Claude Code session and nothing was seen in a window. **U2** (the app: person marks, marks in the tab and the detail, and a disagreement shown beside the memory) is done, see below; **U3** (the plugin: the consolidator puts `wrong` memories first, `prepare` warns about them, the skills read the marks) is next.

**U2, the app's marks** (done 2026-11-08, with gaps; D202 (1), (2), (3) and D205). The detail of an accepted memory has Useful and Wrong buttons, the person's current mark highlighted, an optional one-line reason (at most 200 characters, checked before sending), the latest mark of each actor, and the full history read with the detail. The Memories tab shows a Marked wrong chip on a row when any actor's latest mark is wrong, and the detail also shows Disagreement when the person and an agent differ. Palette: Mark Memory Useful and Mark Memory Wrong, for the selected accepted memory. A saved mark reads the memory again, and a mark the person already has sends nothing. Tests: the mark planner, reason check and labels in `memories.rs` and `memory_rows.rs`, and the palette rules in `action_registry.rs` (unit tests; 1660 gm-ui tests pass). Known gaps: nothing was seen in a window, so the layout of the new detail section and the reason box is not checked on screen; no filter by mark; the plugin does not read marks yet (U3); the palette mark has no reason.

**Acceptance.** Real-Git tests: `basis` is stored and returned, an `unconfirmed` memory is marked in results, counts rise on retrieval and are never exported, (Q1 to Q3 done). Q3 is not yet confirmed in a live Claude Code session: that step is open.

## Path unification (D197): one config and data path (done 2026-11-07, commit fec4253)

Settings, workflows, the recent list and the trust store use one resolver, `gm_core::paths`, with the XDG layout on Linux and macOS: `$XDG_CONFIG_HOME/gitmaster` and `$XDG_DATA_HOME/gitmaster`, with the home-directory defaults. The earlier macOS locations are not migrated, since the app has no users before go-live (D197 (3)).

## Sub-agent threads (D199; S1 done, S2 blocked on Q45, S3 and S4 not started)

- **S1, storage and model** (done, f84dd16): schema v9 of `threads.db` with `parent_thread` and `parent_call`; the thread list hides sub-agent threads; `thread_get` and `thread_read` report the parent; deleting a parent deletes its sub-agent threads.
- **S2, capture** (blocked): waits for the real `SubagentStop` and `PostToolUse` payloads and the sub-agent transcript files, which must be captured with the T0 method first (Q45).
- **S3, Threads page** (not started): the nested row, its expansion, **Open Thread** and the depth cap.
- **S4, search and MCP** (not started): `parent` on search hits and on the thread reads, and the `Planned` markers in `docs/guide/threads.md` and `docs/reference/mcp-tools.md` removed as each part ships.

## Slice 8a: agent annotations (decided 2026-11-07; D200)

Requested by the author on 2026-11-07. Behavior in `docs/agents/local-review.md` ("Notes, questions and flags from the agent", `Planned`).

**Goal.** An agent leaves notes, questions and flags on code and you answer in the app; the agent reads the answer by pull.

**Sub-slices.** A1 store and command: review schema v5 with `kind`, `severity`, an author on roots, the limits, `CreateAgentComment` and the default list filter. A2 MCP: `review_comment_create`, the `author` filter of `review_comments_list`, fields in the results. A3 UI: kind and severity chips, **Agent-written**, **Needs answer** badge and count, Local review filter. A4 plugin: the `inbox` skill and the `gitmaster` skill update. Order A1, A2, A4, A3.

**A1** (done 2026-11-07, with gaps): review schema v5 (`kind`, `severity`), `CreateAgentReviewComment`, the limits, the human-only close, `needs_answer` and the `author` list filter (`ReviewAuthorFilter`), with real-Git and store tests. Known gaps: no MCP tool, UI or skill yet (A2 to A4), so the docs stay `Planned`; the MCP `review_comments_list` already uses the human default; nothing seen in a window.

**Acceptance.** Real-Git tests: an agent creates a thread on a unique anchor; a missing or ambiguous anchor and the limits are refused; the agent cannot resolve or reopen it; a person's reply makes it appear in the default list; an agent thread is not handed back as work before a reply. A4 is tried in a real session and what was seen is stated.

## Slice 8s: review workflows and scorecards (decided 2026-11-10, D219)

Requested by the author on 2026-11-10. Engineering design in `design/reviewers.md`; behavior in `docs/agents/reviewers.md`, `docs/agents/local-review.md` and `docs/reference/mcp-tools.md` (`Planned` where not built). Depends on slice 8 (local review), slice 7 (MCP) and slice 3w (workflows). It replaces slice 8r: its gates, approvals, proposals, reviewer files and bypass are removed (S2), and its verdict table is replaced by scorecards (S3).

**Goal.** A reviewer is a manual workflow that starts the person's own agent. Its result is a scorecard. A push is blocked only by a `before.push` workflow that reads the scorecards, and nothing in the app is native or bypassed.

**Sub-slices.**

- **S1, docs and D219 (done when this lands).** The docs, `design/decisions.md`, this roadmap, `design/reviewers.md` and `design/agent-skills.md`.
- **S2, removal.** Remove gates, `gates.json`, proposals, approvals, reviewers, Bypass and the native check from the engine, MCP and tests. Done when no code path or test names them and `make check` passes. **Done 2026-11-10.** Known gaps: the verdict, reviewer and workspace reviewer-layer code is kept for S3, so `reviewers/` files are still read for `reviewers_list` and `verdict_record`; the tables `approvals`, `bypasses` and `proposals` stay in the schema until migration v11 (S3) and their queries are gone; `ReadBranchDescription` is kept as a read, `SetBranchDescription` is removed; a `gates.json` left in a repository is no longer read; a push or restack is blocked only by its `before.<op>` workflows; nothing was seen in a window, and `ui-smoke` was not run.
- **S3, scorecards.** The `scorecards` table, labels and decorations on comments, migration v11, the MCP tools `scorecard_publish` and `scorecards_list`, `label` and `decorations` on `review_comment_create`, and `ctx.scorecards`. **Done 2026-11-10.** Known gaps: the comments of a scorecard that retention deletes keep no `scorecard_id` (the foreign key is `ON DELETE SET NULL`, as the design says), so an open one counts against the agent-thread limits; the verdict, finding and reviewer columns `verdict_id`, `finding_branch` and `finding_head` stay, unused; a comment and a suggested fix are capped at 1,000 characters (the design does not give a figure); a `blocking` decoration on a label that is not a flag counts in `blocking_open` but gives no severity; a publish or listing in a workspace member (`member`) is not covered by a test; the Review page still shows the reviewers placeholder (S5); no `meta.review` or `ctx.change` (S4); nothing was seen in a window, and `ui-smoke` was not run.
- **S4, review workflows.** `meta.review`, `ctx.change()`, the `.git/gitmaster/workflows/` loader path, `GITMASTER_WORKFLOW_RUN` in the child environment, and the two templates `review-invariants.lua` and `review-for-task.lua`. **Done 2026-11-10** (choices in D220). Known gaps: `ctx.change` gives empty `files` and `truncated` above 10,000 paths, not a cut list (D220 (4)); the picker is the pure filter `review_workflows`, with no command yet, and the UI does not show the location or the shadowing yet (S5); `role` replaces the `review` flag, a clippy limit (D220 (1)); a dry run answers the review reads with an empty result (D220 (6)); `GITMASTER_WORKFLOW_RUN` is read when `gitmaster mcp` starts, and that the variable reaches `claude -p` is not checked (S6); the `workflow` CLI has no listing, so the location and review flags are checked through the engine's `ListWorkflows` only; nothing was seen in a window, and `ui-smoke` was not run.
- **S5, Review page.** Pure view models first (chips, trend, blocking count, filters), then the GPUI page: the Scorecards table, **Run review…**, Conversations by file with the filters Blocking, Scorecard and Label. **Done 2026-11-10** (choices in D221). Known gaps: the table reads `ListRecentScorecards` (newest five of each name, D221 (1)); a comment whose scorecard is older than its name's five newest shows no name and is not matched by the Scorecard filter (D221 (8)); several picked reviews that declare inputs each open the inputs dialog, so only the last stays open (the registry's Run Workflow, one dialog at a time); the picker lists the workflows read when it opens; the scorecard table is not virtualized (few names); the agent side is not checked against a live `claude -p`; `make check` was not run as a whole (audit step, see the report); **ui-smoke was not run, by decision of the author**, so nothing was seen in a window. S6 skipped by the author (budget); the live `claude -p` check stays open under Q42.
- **S6, live check.** `claude -p` with MCP: tool names, the flags that deny shell and file writes, environment passing and thread capture, against a real session, kept as fixtures. Cursor CLI afterwards, once its MCP configuration and permission flags are verified without writing into the working tree (Q42).
- **S7, plugin skill.** `/gitmaster:write-reviewer` in `ai/`, a separate repository: noted here only.

**Acceptance.** A `before.push` workflow that calls `ctx.block` vetoes a real push on real Git and fails closed on an error. A stale scorecard is marked **Stale** wherever it is read. An agent cannot resolve a comment, publish on a stale head or switch off a blocking workflow.

### History of slice 8r (R1, R2, R2h; removed by D219)

The work below was built in part and verified in the backend. D219 removes it: the gates, approvals, proposals, Bypass and reviewer files in S2, and the verdict table in S3. It stays here as the record of what was built.

**Sub-slices.** R1 verdict store and findings, R2 gates (pure, with the workflow phases and `ctx.verdicts()`), R3 UI, R4 the remaining skills and sub-agents, R5 `branch_agent_config`.

**Review of 2026-11-09 and slice R2h.** An independent review found that workflow `ctx.push.lease` and `ctx.restack.apply` were not gated, that the gate check did not see the presets that `verdict_record` accepted, that the bypass record was in memory only, that a restack was gated on its root only, that a commit could land between the check and the push, that findings were keyed by branch name, and that five test files were not hermetic. R2h fixes them and the docs were corrected; see the commits. The order now is R2h, R2b, R2c, slice 6w (workspace inherits the memories and contexts of its members, D216), slice 6u, R3.

**Plan of 2026-11-09 (D212, amended by D214: conditions, layers, proposals and workspaces; the order is R1, R2 with conditions, R2b proposals and `/gitmaster:plan-review`, R2c workspace layer, slice 6u, R3; D215 adds `human_approval` and the branch description to R2, and `suggested_fix` with the fix loop to R2b).** The author chose to build R1 to R3 together; R4 and R5 wait for Q42. Order: (1) docs and D212; (2) R1 store: review schema v6, verdict types, reviewer files, `verdict_record` and `verdicts_list`, `reviewers_list`; (3) R2 gates: pure `gates::evaluate`, `gates.json`, the native check on push and restack, `BypassGates`, `gates_status`, then the `push` phase and `ctx.verdicts()`; (4) R3 UI: Reviewers Settings, **Run Reviewer…**, the verdict panel, gate state on the branch, the blocked-operation message with **Bypass**. Each step ends with `make check`. **Status (2026-11-09): R1 and R2 are done, backend only; R2b, R2c, slice 6u and R3 are not started.** R1 (D189, D212): review schema v6 to v8 (verdicts, `verdict_id`, `finding_branch`, approvals in v7), reviewer files with `paths` and `branches`, `RecordVerdict` (agent only, redacted, fail closed, head must be the tip, branch subject only), `ListVerdicts`, `ListReviewers`, MCP `verdict_record`, `verdicts_list`, `reviewers_list`. R2 (D212, D214, D215): the pure evaluator and glob (`gates.rs`, `glob.rs`, property-tested), `gates.json` with user and repository layers, `ApproveBranch`, `RevokeApproval`, `SetBranchDescription`, the change of a branch (`change.rs`), `GatesStatus` and MCP `gates_status` (with `settings`, `intent`, `approved`), the native check before `PushWithLease` and `ApplyRestack` (veto, `GatesBlocked`, a broken gates file fails closed), `BypassGates` (human only, names the gates it bypasses), the `before.push` and `after.push` phases and `ctx.verdicts`. Known gaps: nothing was seen in a window, and there is no UI yet (R3); verdicts record only for the `branch` subject, so `staged` and `commit` reviewers are refused; `verdicts` has no `subject_kind`, `base`, `diff_hash` or `derived_from`; `model` and `run_thread` of a verdict are always empty (the MCP call carries no model and the run is not linked to its thread); the `handled` rule for findings and `suggested_fix` (D215 (3)) are R2b, so a handled finding is not exercised; `member` on `gates_status` and the workspace layer are R2c; the failure path of a gate check that cannot read the store is covered by review only; `ctx.verdicts` opens `review.db` on each call; a push refused for another reason (an agent, a running remote operation) is now refused before the workflow gates; the in-memory operation log has no `detail`, so the bypass's gates and reason are in the log through the command itself; `ApplyRestack` is gated on its root branch only, not on every branch it moves; no skill or sub-agent exists yet (R4, in the `ai` submodule).

## Slice 6k: contexts as areas of knowledge (done 2026-11-10, with gaps; D222, D223, D224)

Every memory belongs to one or more contexts; a context is an area of knowledge that may have no repository; a sub-context inherits its ancestors' memories; repositories are bound to contexts. Behavior: `docs/guide/contexts.md`, `docs/guide/memories.md`, `docs/guide/workspaces.md` and `docs/reference/mcp-tools.md`, with `Planned` markers. It comes before slice 6u and before any further agentic or knowledge feature, since those build on where a memory lives.

**Sub-slices.**

- **K1a** (done 2026-11-10; D223 (1) to (3)): any context is read by name, a named context covers its ancestors, a memory in `contexts.db` is read and acted on whatever the repository is bound to, and agents propose only into the bound reach. **K1b** (done 2026-11-10, with gaps; D223 (4) to (7)): a proposal without a context goes to the one bound context or to the context of the repository; a plain repository's `threads.db` memories migrate into the context of the repository. **K1c** (done 2026-11-10, with gaps; D223 (7), (8)): workspaces and their members migrate into their contexts and read and write through contexts only; every writing repository is recorded in `contexts.db` v7. Known gaps: no test of a member whose id clashes; marks carried by a workspace migration are tested in the store only; `ReadMemory` and `RecordMemoryRetrieval` drop a failed migration silently, since their events have no note; MCP and UI still offer the `repository` scope and Move to Context from a repository (K2, K3); nothing was seen in a window.
- **K1, one store and the migration (engine, no UI change).** `contexts.db` holds every memory; the move of each repository's and workspace's `threads.db` memories into the context of the repository (D222 (4), (6)), with sources, links, marks, counts and basis, idempotent by uid; `ProposeMemory` with the default context rules; `ListMemories` with `scope` `bound` or any context (inheritance through ancestors); `MoveMemoryToContext` between contexts; the `repository`, `workspace` and `member` scopes and the two-store merge in `memory_scope.rs` removed; the workspace reach of D222 (5). Tests: real Git in `crates/gm-core/tests/contexts_real_git.rs` and `memories_real_git.rs` reshaped to the example of `docs/guide/contexts.md` (Finance, Payments, Refunds, Accounting, Sales and a context with no repository), a migration of a populated `threads.db` and one interrupted between its writes.
**Done 2026-11-10.** K2 (08335fa), K3 (8df79f2, 91f9559, 794ce34, 8e89c56, b91a357), K4 (plugin 0.6.0 on the plugin's `develop`, pinned in b3a8fcb) and K5 (0a0a225, 0cde880, 498c7ba). Seen in a window under Xvfb, light theme: the Memories tab with chips per context and the scope filter on a named context; Manage Contexts with the whole tree, sub-contexts indented, the **bound** mark and the memory count per row. Known gaps: the dark theme, New Memory's context chooser, Move, Attach and Detach were not seen in a window; `scripts/ui-smoke.sh` fails on its first Working Copy scenario because the layout sits lower than its click constants (not caused by this slice), so no smoke scenario covers it; long context names clip without an ellipsis glyph; `ThreadStore::open_keeping_memories` is public (doc-hidden) for tests; the plugin skills were not run in a live agent session.

- **K2, MCP.** `memories_search` `scope` (`bound` or a context), `contexts_list { all }` with `bound`, `memory_propose` defaults and the refusal that lists the bound contexts, `member` removed; `crates/gm-mcp/tests/memories_mcp_real_git.rs`. Done 2026-11-10 (D223 (9)); the app's **This repository** chip now sends `bound`, covered by a unit test only, not seen in a window.
- **K3, the app.** Manage Contexts shows the whole tree, **Bind Repository** and **Unbind**, contexts with no repository; the scope filter (bound, or any context); New Memory picks a context; Move between contexts; the workspace window.
- **K4, plugin text.** `propose-memories`, `prepare` and `consolidate` speak of contexts, not repository memories; the plugin version is raised.
- **K5, cleanup.** `threads.db` drops its memory tables once empty; the `Planned` markers come off and the pages go back to their status.

## Slice 6h: code health (CH0 to CH5 done 2026-11-11, with gaps; CH6 and the backtest to do; D225 to D228)

Requested by the author on 2026-11-11 for daily use on very large repositories at work. Behavior: `docs/guide/code-health.md`, the Health tab and heat dots in `docs/guide/code-navigation.md`, the keys and `health.db` in `docs/reference/configuration.md`, the tools in `docs/reference/mcp-tools.md`, all `Planned`. No preview flag (D225 (8)). Sub-slices use the prefix CH, since H1 to H8 belong to History.

- **CH0, docs and decision** (done 2026-11-11): the pages above, D225, this entry.
- **CH1a, the index** (done 2026-11-11). `health.db` store, ingest of `git log --numstat -M -z` newest first in two phases, cancellable, resumable, shared with `gitmaster mcp`. Real-git tests.
- **CH1b, the metrics** (done 2026-11-11). Pure functions: clocks, heat, trend, dormancy, changes together, ownership, constraints, config resolution and `config_hash`. Unit and property tests.
- **CH1c, the engine** (done 2026-11-11). `LoadHealth` command and its events, mirroring `ReadProvenance`; per-process cache per `(rev, config_hash)`; progress events while indexing.
- **CH2, MCP** (done 2026-11-11). `health_context`, `hotspots_list`; `crates/gm-mcp/tests/health_mcp_real_git.rs`.
- **CH3a, heat dots** in the Files tree (done 2026-11-11). **CH3b, the Health tab** (done 2026-11-11): dormancy breaks, virtualized hotspot list with sparkline, file details, filter, **Show Code Health** in the palette.
- **CH4, churn** (done 2026-11-11; D227, D228). **CH4a, engine:** `health.db` v2 with the hunk headers of the first-parent commits, their ingest, the pure replay into a `LineChurn` per file, the job fills `FileHealth::churn`. **CH4b, front ends:** the Health tab's churn panel and the file's churn in the details, and `churn_summary` over MCP.
- **CH5, the prepare skill** reads `health_context` (plugin, `ai` submodule; done 2026-11-11, plugin 0.7.0).
- **CH6, the hotspot map** (treemap).
- **Owed:** the backtest of the research doc on a large real repository before tuning any threshold.

**Measured** (release build, git/git, 11,316 commits in the 3-year window, `crates/gm-core/tests/health_measure.rs`): cold index 9.7 s (about 1,170 commits a second), an incremental pass 0.22 s, loading the window 0.26 s, the indicators 8 ms. A window of 200,000 commits would take about three minutes to index the first time, the last six months first.

**Churn, measured** on git/git (release): the hunks of 2,751 first-parent commits (103,683 hunks) read in 7.7 s cold and 63 ms incrementally, the replay in 83 ms; 1.4% of 446,894 lines rewritten within 14 active days, 85% alive after 180. **Checked against `git blame`** at HEAD for five files: the lines born in the window and alive match exactly for `odb/source-files.c` (1,327), `builtin/clone.c` (573) and `odb.c` (841); `setup.c` (1,426 against 1,404) and `Makefile` (817 against 800) count more, as D228 (2) says: lines committed on a side branch before the window and merged in it are born on the merge day. Seen in a window: the churn panel for the whole repository and for `odb` (3% of 5,745 lines), and the file's churn in the details.

**Seen in a window** under Xvfb on git/git: dark theme, the tree's heat dots, the progress line while the history is read and the dots of the partial report, the Health tab with a dormancy break, the hotspot list and the file details; light theme, aligned hotspot columns, the selection, the folder filter and the agent's constraints.

**Known gaps:** no hotspot map (CH6); the survival chart's bars are 24 px tall, so 98% and 66% look alike; complexity does not weight the rank yet (heat alone); no `scripts/ui-smoke.sh` scenario; the partial answer of the MCP tools is not covered by a test; a refresh while a health read is in flight is skipped until the next refresh; a first MCP call in a repository with no index builds it inline (open the Health tab first in a very large repository); keyboard navigation of the hotspot list. `cargo audit` reported RUSTSEC-2026-0459 (`libsqlite3-sys` 0.38.2 through `rusqlite` 0.40) before this slice; `rusqlite` 0.41 fixes it with no code change.

## Slice 6u: memory revisions, applies-to paths and links (decided 2026-11-09; D213, Q47)

Requested by the author on 2026-11-09. Behavior in `docs/guide/memories.md` and `docs/reference/mcp-tools.md` (`Planned`). Order: as the author chose on 2026-11-09, after the review workflows of slice 8s (S4); D219 replaces the gates of slice 8r that it was planned after.

**Goal.** An agent can suggest an update to an accepted memory, covering its text and the files it relates to, and the person accepts or denies it; memories can be linked; and the engine can answer which accepted memories apply to a path, which a later recall hook (Q47) will use.

**Sub-slices.** U1 store: revisions with their history, `memory_applies` and `memory_link` in `threads.db` and `contexts.db`, the pure path match and the link and revision rules. U2 engine and MCP: `ReviseMemory`, the accept, edit and deny commands, `MemoriesForPath`, link commands, `memory_revise`, `memories_for_path`, `applies_to` on `memory_propose`, `links` and `applies_to` on `memory_get`. U3 UI: **Proposed update** in the memory detail with before and after, applies-to and links in the form and detail, **Conflicts with**. U4 skill: `/gitmaster:consolidate` proposes revisions instead of replacement memories where it can.

**Acceptance.** A revision of an accepted memory is proposed by an agent and applied in place by a person, keeping marks and attachments; an agent cannot accept it or change a memory any other way; a stale revision cannot be accepted unedited; the path match and the link rules are pure and property-tested; `MemoriesForPath` is tested on real Git and measured in a release build.

## Slice 6w: a workspace inherits the knowledge of its members (decided 2026-11-09; D216)

Requested by the author on 2026-11-09. Behavior in `docs/guide/workspaces.md`, `docs/guide/memories.md`, `docs/guide/contexts.md` and `docs/reference/mcp-tools.md` (`Planned`). After R2c, before slice 6u. D219 removes R2c with the rest of slice 8r; the order of slices 6w and 6u is unchanged.

**Goal.** Opening a workspace of two or more repositories that already hold memories, the workspace sees all their memories and the contexts they belong to, with its own threads kept, and nothing is copied.

**Sub-slices.** W1 engine: the members of the open workspace (D174), a merged scope over the workspace's own store, each member's `threads.db` and `contexts.db` through the members' `repo-id`, `scope` labels, unavailable members, `contexts_list` of members. W2 MCP: `memories_search`, `memory_get`, `contexts_list` from a workspace, `member` on `memory_propose`. W3 UI: member chips, the scope filter, Manage Contexts from a workspace window.

**Acceptance.** Real-Git test on a workspace of three clones with memories and two contexts: the workspace lists every memory with its scope, a later memory in a member appears, a removed member is listed as unavailable while the others answer, an action on an inherited memory changes the member's store, and a new memory without a target stays in the workspace.

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

## Slice 7s: workspaces (planned 2026-11-06; D174, D175, Q36)

Several repositories in one workspace: a superproject whose submodules are linked worktrees of the user's clones. Docs: `guide/workspaces.md`. The ids are `SW*` because `WS*` belongs to the workflow slice.

- **SW0, engine and CLI (D177) (done 2026-11-06, with gaps):** built in `gm-core::workspace_cli`, 22 real-Git tests in `workspace_cli_real_git.rs`, routing tests in `gitmaster/tests/workspace_cli.rs`, and checked by hand with the binary over three repositories. Gaps: Q36 (b) and (d) are still open (a moved workspace folder or a moved member repository is not repaired, only refused or reported); a locked member is not handled specially; removal is not transactional; no engine `Command` yet, so the UI cannot use it. Original scope: `gitmaster workspace create|add|list|status|remove` over the worktree commands of D172; real-Git tests in temporary repositories for the shape of D174 (worktree member added as a submodule in place, no `protocol.file.allow`, detached member detected, removal blocked as D31 says). Answer Q36 (b) and (d) here.
- **SW1, submodule reads in the UI (D175) (done 2026-11-06, with gaps; seen in a window under Xvfb, both themes: the row and its marks, the commit pane, the context menu, Quick Open, the Code page and text search across two members, and Open Submodule by double-click):** member rows in the Working Copy (branch, dirty, new commits, detached), submodule diff as a commit list, opening a member, `--recurse-submodules` in Quick Open and text search.
  - **SW1a, files and search (engine) (done; gaps: gitlinks without a `.gitmodules` entry still list as files; one failing submodule grep fails the whole search; one extra `ls-files --stage` per read when `.gitmodules` exists):** the file list and `git grep` run in the repository and in each checked-out submodule and merge by full path; the gitlink is no longer listed as a file; files-box words route to the submodule they name. Docs: `guide/code-navigation.md`, "Submodules".
  - **SW1b, submodule state (engine) (done 2026-11-06, with gaps):** built as specified; 8 real-Git tests in `submodule_real_git.rs`. The status flag is `commit_changed` (Git's `C`: the checked-out commit is not the recorded one, ahead or behind); the row's "N new commits" and "N behind" come from `LoadSubmodule`. Gaps: a submodule that lacks the recorded commit fails with Git's message instead of answering; no UI consumes `SubmoduleLoaded` yet. Spec: `StatusEntry.submodule` from porcelain v2 `S<c><m><u>`; `LoadSubmodule` gives the branch, detached state, recorded and checked-out commits, the new commits (capped at 200) and how many are missing.
  - **SW1c, Working Copy rows (UI) (done; gaps: every status refresh reads every submodule again and superseded reads are not killed, to be measured in a release build with many members; the context menu has a fixed width, so long disabled reasons, these and older ones, run past its edge):** the submodule row with its marks, the commit list in place of the diff, staging the row, **Open Submodule** in a new window. Docs: `guide/working-copy.md`, "Submodules".
- **SW2, provenance:** threads of the workspace name the member of each edit and commit (Q36 a); recording a combination as a superproject commit (Q36 c).
  - **SW2a, member-aware capture (D180 (1)) (done 2026-11-06, with gaps):** schema v6 and the hook work as specified; hook cost measured in a release build, 30 runs of a PostToolUse event: plain repository p50 21 ms, p95 22 ms; workspace with three members p50 28 ms, p95 36 ms (budget 100 ms). Gaps: the UI and MCP do not show `member` yet; edits stored before v6 keep no member and read the workspace's `HEAD`; a member's commit links are re-read on each invocation once its HEAD moved. Spec: schema v6; the member of each edit and its state from the member; per-member heads and commit links in the hook pipeline. Docs: `guide/threads.md`, "Threads in a workspace".
  - **SW2b, the pointer (D180 (2)) (done 2026-11-06, with gaps; seen in a window under Xvfb: a member clone with no thread of its own lists the workspace thread under THREADS, `in workspace <name>`, and selecting it opens the workspace's window):** review found that the commit panel asked for no threads in a repository with none of its own, which is every plain member clone; it now always asks. `Event::ThreadRead.detail` is boxed (clippy `large_enum_variant`; the JSON is unchanged). Gaps: workspace threads only on the first page of a commit's lookup; the Files panel shows no member tag (its paths start with the member folder); a read-only open creates the workspace store's `-shm`/`-wal` side files in its `.git` when they are missing; the lookup SQL repeats the commit rule of `FILTER_SQL`. Spec: `workspace add`/`remove` keep `gitmaster/workspaces` in each member's common directory; the commit lookup reads the listed workspaces' stores read-only.
  - **SW2c, Record Combination (D180 (3)) (done 2026-11-06; seen in a window under Xvfb: palette, the commit and its message, the row after it):** a pathspec commit (`git commit -- <gitlinks>`) leaves other staged files staged; a member with no commit yet is refused too. Gaps: palette only (the Working Copy has no repository-wide menu); no workflow trigger fires for it. Spec: engine command and UI action. Docs: `guide/workspaces.md`, "Record a combination".
- **SW3, agent tools (done 2026-11-06, with gaps):** MCP tools that take a member, a workspace status tool, the review loop over every member.
  - **SW3a, read tools (D181) (done 2026-11-06; checked through the real MCP server over stdio on a two-member workspace: `workspace_status` fields, and `provenance` on a member line found the member commit and the thread with `blob_match` and `observed_commit`):** review reverted an unasked change that had taken status refreshes out of the command log. Gaps: `new_commits` is capped at 200. `workspace_status`; `provenance` on a member's path (blame in the member, threads from the workspace). Docs: `reference/mcp-tools.md`, "Tools in a workspace".
  - **SW3b, review across members (D182) (done 2026-11-06; checked in a window and over MCP: a comment from the Code page on a member's line, then `review_comment_resolve` measured the before and after text; the refs are in the workspace and none in the member clone; one member file adds about 9 ms to a snapshot):** gaps: `hash-object` applies the workspace's attributes and filters, not the member's own `.gitattributes`; `Ctrl+Shift+M` did not open the composer after a click on a line number in that run (the **Comment on line** button did), not investigated. a comment on a member's file snapshots that one file into the workspace's object store, so the before and after text and the measured change work as for the workspace's own files.

## Slice 7u: workspace UI (done 2026-11-06, with gaps; D183, D185)

- **SW4a, engine:** move the `gitmaster workspace` logic into `gm-core::workspace`; the engine commands of D183 (1); the CLI keeps its behaviour and tests.
- **SW4a, engine (done 2026-11-06).**
- **SW4b, UI (done 2026-11-06, with gaps; seen in a window under Xvfb: the folded section and its list, the dialog with a member's plan (`creates feat/pay from main`), the name check, creating opens the new workspace's window, the remove dialog with a dirty member blocked until the checkbox, removal keeps the branch and drops the pointer line, Check Out Branch on a detached member). The window review found five defects, fixed the same day and re-checked in a window: Check Out Branch now refuses a detached HEAD whose commits no branch holds (it had left them behind), **Branch for all** takes focus on a click, Escape closes the repository list before the dialog, long paths are cut at the list's edge, and the title is "New workspace". Gaps: branch suggestions only for the window's own repository; the dialog does not scroll with many members; uneven space between folded section headers.)** Spec: the Workspaces section, the New Workspace dialog, the Remove Workspace dialog, **Check Out Branch** on a detached member row. Docs: `guide/workspaces.md`.
- **SW4c, ephemeral on close (D185) (done 2026-11-06; seen in a window under Xvfb through `Ctrl+Q`: a clean ephemeral workspace is removed on exit with its worktree and pointer line, the branch kept; a dirty one holds the exit with the dialog, and Cancel, Keep workspace and Remove workspace each do what the docs say). Gap: the window manager's close button goes through the same state machine (`on_window_should_close`) but could not be driven here (no window manager under Xvfb, and this xdotool has no `windowclose`); not seen.** Spec: remove an ephemeral workspace when its window closes, asking when something would be lost.

## Slice 7v: workspaces without kinds (done 2026-11-07; D186; seen in a window under Xvfb: the × on a hovered row, removing another workspace keeps the window, removing the window's own workspace closes it, a workspace with only the old `gitmaster.workspace.kind` is listed; `--ephemeral` is a usage error)

- **SW5:** remove the ephemeral kind end to end (CLI flag, marker, list formats, dialog toggle, badge, the close check of SW4c); the × on a workspace row; Remove Workspace from the workspace's own window, which then closes. Docs: `guide/workspaces.md`, "Remove a workspace"; `reference/cli.md`, "Workspaces".
- **SW6, branch picker for every member (D195) (done 2026-11-07, with gaps; checked by unit tests of the picker rules and the listing cache, real-Git tests of `ListRepoBranches`, and the model routing; not seen in a window, no display here):** each member row's branch is a button that opens a filterable list of the repository's branches, local first, remote-only ones marked, with **Create branch** for a new name; `ListRepoBranches` reads any repository without opening it. Gaps: the list shows the first 8 matches and does not scroll, as the worktree dialog; a remote branch is listed once per short name across remotes. Spec: `ListRepoBranches`, the picker in the New Workspace dialog. Docs: `guide/workspaces.md`, "Create a workspace".

## UI smoke suite: stale coordinates (found 2026-11-06, repaired 2026-11-07)

`scripts/ui-smoke.sh` had drifted from the app before D184. Repaired: every scenario passes in both themes (agent run 2026-11-07; re-run by the manager: all 31 light scenarios and the first 6 dark ones, stopped by a container restart, no failure). Each scenario now opens only the sidebar section it needs (Worktrees alone for the worktree scenarios, Branches alone otherwise). Fixed coordinates: stage lines (gutter x), branch menu, worktree section crop, remove/lock/prune worktree rows and menus, commit diff split (file row), commit stack rows, split gutter checkboxes, script editor caret. The `v-lock-prune-worktree` scenario passes again since the app stopped pruning missing worktrees on open (96cb74f). Unconfirmed: `Ctrl+End` did not move the caret in the Script editor in one run.

## Slice 8: local review

- Comments, states, anchor without guessing, before and after snapshots.
- Tools `review_comments_list` and `review_comment_resolve`.
- Inline comment on the diff, with Before/After.
- Confirm D11 and Q3 before starting.

## Review reply threads (D198: done 2026-11-07, with gaps)

A reply is a review comment with a parent, and the agent's summary is its reply (review store v4 migrates the existing summaries). The MCP tool `review_comment_reply` writes a reply, and the app shows each thread under its comment on the Local review page and in the Working Copy diff, with a Reply box on the Local review page. Known gaps: in the diff, Reply opens the composer dialog, not an inline box, because a diff row cannot hold a growing text area; no palette action; `scripts/ui-smoke.sh` does not cover replies; the `ai/` plugin skill does not name `review_comment_reply` yet. Nothing was seen in a window.

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

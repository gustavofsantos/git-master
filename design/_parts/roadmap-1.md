# Roadmap

The order of the work. Status marks: **(done)** in the code and verified; **(to do)** not started; no mark = future slice. The order prioritizes reaching something the author uses daily and that already shows a differentiator. The documentation in docs/ is the desired state; this file orders the work toward it, and `scripts/docs-status.sh` lists what the docs still mark as planned.

Last updated: 2026-10-21 (slice H5, lane graph, done). Initial proposal, to be iterated on.

## Order criteria

1. Technical risk first (spikes), before building on top of it.
2. Each slice is vertical and usable on its own.
3. The first differentiator (hooks) appears early. Stacks and MCP come after the base of Working Copy and history.
4. If the most urgent differentiator is stacking, slice 5 can move forward. The order is the author's decision.
5. Workflows come early because they make stack policies and LLM-written commit messages configuration, not code. Order confirmed by the author on 2026-10-19 (Q20).

## Slice 0: spikes and skeleton (done)

Goal: validate what could invalidate the design.

**Scope**

- Spike A (dropped by the author on 2026-10-16): virtualized list in GPUI with 100,000 rows.
- Spike B (done): editable text in GPUI, for the resolver's result panel. Evaluate libraries and licenses (Q7). Verdict in Q7.
- Spike C (done): in-memory rebase with libgit2 (coverage) and `git merge-tree --write-tree`. Compare with gitoxide (Q6). Verdict in Q6.
- Spike D (done, no fsmonitor verdict): status in a massive repository through a subprocess with fsmonitor. Findings in `spikes/d-status/FINDINGS.md`.
- Skeleton (done): Cargo workspace with `gm-schema`, `gm-core`, `gm-ui` and `gm-mcp`; GPUI window; open a repository; minimal operation log; base property tests.

**Done when:** the app opens a large repository and lists its changes without freezing. The spikes have verdicts recorded in design/decisions.md (Q6, Q7).

**Known gaps:**

- Spike D could not measure fsmonitor on this Git build, so it has no verdict.

## Slice 1: Working Copy and commit (done)

**Scope**

- File list with tri-state checkbox, filter and grouping by directory (done).
- Unified diff (histogram), stage per file, hunk and line, with a warning for a −/+ pair (done).
- Commit composer and commit (done).
- Keyboard shortcuts: navigation, stage, filter, refresh and focus (done).

**Done when:** it replaces `git add -p` plus `git commit` in daily use. The code is complete; the author's daily use is the test.

**Known gaps:** none recorded.

## Slice 1b: the basics still missing, before the differentiators (done)

Review of 2026-10-16. With the Working Copy ready, three things are still missing. Without them the app does not replace a Git client day to day. They come before slice 2. The order is: command log, then branch switch, then project switch. The log comes first because the other two operations then show up in it, which helps check them.

**Scope**

1. **Git command log** (done).
   - Only operations with side effects are logged (D28): stage, unstage, commit and checkout. Status, diff and bulk reads are not.
   - Each `git` process becomes a structured entry: program, directory, arguments, exit code, duration, stdout and stderr. It is kept in memory (the last 1,000 entries), in the UI only, never in the repository.
   - A status bar button opens a read-only floating panel: the list (newest at the bottom), failures highlighted, output expandable.
   - Prerequisite (done): stage, unstage and commit run through the `git` executable (D26 revised). Commit now runs Git's native hooks (D18), which did not happen before. Per-line staging (D24) writes the blob and the index with Git commands.
   - Restack (libgit2) changes refs. Logging it is a decision for slice 5.
   - This differs from the engine's operation log (D14, design/architecture.md). This one shows what Git ran. That one shows what was requested and by whom.
2. **Switch branch** (done).
   - `ListBranches` and `Checkout { branch }` commands.
   - Selector in the status bar or the header, with local and remote lists and a filter. A remote branch with no local copy creates a local tracking branch.
   - With a dirty working copy, a checkout that Git refuses shows Git's error in the panel and a notification. No hidden autostash (same line as Q2). See Q11.
3. **Switch project** (done).
   - Native folder picker and a recent list, stored in the app's data directory, never in the repository (principle in docs/concepts/principles.md).
   - `OpenRepo` already exists. Switching explicitly discards the current repository's state (selection, diff, commit draft) and opens the new one.
   - One repository per window in v1. Tabs or several windows come later.
4. **Welcome screen** (done).
   - Shown when the app opens with no project: first run, no command-line argument, current directory outside a repository, and no recent that still exists.
   - Content: the logo, one sentence about the app, an "Open a repository" action (the same picker as item 3), the recent list when there is one (entries missing from disk drop out), and the keyboard shortcut hint.
   - Opening a repository switches the screen without a restart. A folder that is not a repository shows the error on the screen, with no blocking dialog.
   - With no project, actions that need a repository (commit, checkout, command log) are absent, not disabled and empty.
   - English strings. Icon art from `assets/icons/`, in the active theme's colors (design/visual-identity.md).
   - As built: with no argument outside a repository, it opens the newest recent that still exists. With no recents, it shows the screen. A non-repository path on the command line shows Git's error on the screen. There is no keyboard shortcut to open.
5. **Design sidebar** (done, with placeholders).
   - Left column, as in the canvas: "Workspace" (Working Copy with a file count; History, Stashes, Local review and Reflog are placeholders that say they do not exist yet), a "By stack" / "By name" switch, and the branch tree.
   - "By name" is real: local and remote branches grouped by prefix. Clicking a branch checks it out, and the current one gets the HEAD badge. "By stack" shows "No stacks yet" and lists all local branches under "No stack" until slice 4. The drag-to-set-parent hint is a placeholder.
   - Top bar (done, in slice 1c): the repository name, with `branch · path` below (the branch selector stays on that line). On the right: Fetch, Pull, Push with lease and Restack. All are disabled with "Not available yet" until the engine has the command. Fetch, Pull and Push with lease now work (slice 1d). Restack stays disabled and has no counter until the stack model exists. No search in the top bar, by decision of this slice.

**Done when:** project and branch switching work without leaving the app. Every Git command run can be inspected in the panel. Opening the app with no project shows a screen that explains what to do, not an empty window.

**Known gaps:**

- The native folder picker was tested only up to opening it, because the dialog cannot be driven without a display.
- Still missing from the design: stack indicators (`restack N`, `↑N`, amber dot, PR), the scoped status bar, fsmonitor, the operation log and MCP.

## Slice 1c: simple Working Copy, context menus and command palette (done, except the commit and stash menus)

Requested by the author on 2026-10-16, based on two new canvas boards: the branch context menu and the command palette. It comes before slice 2 gets an interface, because slice 2's composer and chips live in the simplified screen.

**Scope**

1. **Simplified Working Copy** (done). `DiscardHunk` was added. On the Staged side, discard is refused.
   - Header "Working Copy" with the count ("6 changed"). A flat list, one row per file: state letter (M, A, D, ?) and full path. No filter, no count field, no checkbox per row.
   - Composer at the bottom of the column: message box and a "Commit on <branch>" button.
   - The diff header shows the path and a `+5 −1` summary, with no extra button bar.
   - Stage and unstage work as in the reference clients. Decided by the author on 2026-10-16 (reference board in the design conversation).
     - A "Staged | Unstaged" selector at the top of the diff, next to the file name, picks which side to view. Both sides currently show together.
     - A summary under the header ("2 chunks, 3 insertions, 4 deletions").
     - Each hunk has "Discard Chunk" (asks for confirmation) and "Stage Chunk" (or "Unstage Chunk" on the staged side) on its `@@` line. Per-line staging (D24) stays, through the gutter.
     - Space and the file's context menu stage or unstage the whole file. A file with anything staged gets a discreet mark next to its state letter. "Read" stays a separate axis (D23).
2. **Action registry** (done). Prerequisite for items 3 and 4. Per-line staging (D24) stays outside it.
   - Each action has a stable id, a title, a shortcut, typed arguments (branch, file) and an "enabled now?" check that gives the reason when it is not. Keys, menus and the palette read the same registry, so they never diverge. Anything the engine does not do yet is a disabled action with the reason "not available yet".
   - Running an action dispatches the same `CommandKind` as before, with `Actor::Human`. No parallel path.
3. **Context menus** (done for branch and file; commit and stash still to do).
   - Right-click on a sidebar branch or a file in the list. `shift-F10` opens the menu for the selected file; `Esc` closes it. Items come from the registry, with the shortcut on the right and separators per group. When an item cannot run, the reason replaces the shortcut. Destructive items are red and ask for confirmation.
   - Branch menu: Checkout, New child branch, Create worktree, Restack this stack, Push with lease, Copy branch name, Rename, Delete. Only Checkout, Copy branch name and Create worktree run (the last since W3).
   - File menu: Stage file and Unstage file (each disabled when there is nothing to do), Discard changes, Copy file path, Reveal in file manager. Discard changes is disabled, because the engine only discards hunks (`DiscardHunk`) and no new core command was added.
   - Design shortcuts on the branch menu: Checkout Enter, New child branch ⌘B, Create worktree ⌥⌘W, Restack ⇧⌘R, Push with lease ⇧⌘P, Copy branch name ⌥⌘C, Rename F2, Delete ⌫. They also show on disabled items, where the reason becomes the item's hint. Only ⌥⌘C runs (on the current branch).
4. **Command palette** (done; `⌘K` on macOS not yet tested there).
   - `Ctrl+K` opens it. One field with fuzzy search. Sections "Actions", "Worktrees" and "Branches", each shown only with results (branches only after typing). Matched text is highlighted, with a result count. Arrows move, Enter runs, Tab picks the action's argument (branch or file), Esc leaves the argument step or closes. Footer hints. Virtualized list. Disabled actions show their reason.
   - It runs through the same path as the key and the menu (`run_action`, `Actor::Human`). Sorting runs off the UI thread, and only the newest query's results are shown.
5. **Worktrees** (decided on 2026-10-17, D31; in sub-slices). Patterns and out-of-scope items are in D31 (design/decisions.md). Each sub-slice enables only its own actions in the registry. The rest stay disabled with "Not available yet".
   - **W1, listing** (done). Lists worktrees from Git's machine-readable output: path, branch or detached HEAD, main checkout (first entry), lock state with reason, `prunable`, and bare. The list comes with every status refresh and answers `ListWorktrees`.
   - **W2, sidebar section** (done). A "Worktrees" section under the branch tree. Main checkout first, a "here" badge on the current one, a "missing" badge for what Git can prune, and a color from a hash of the name. No agent badge (Q12). The watcher observes the common directory's worktree registry, so a worktree created or removed from the CLI updates the section with no refresh.
   - **W3, create** (done). A dialog with three modes: existing branch, new branch from a commit, detached HEAD. The folder defaults to `../<repo>-<branch>` and follows the branch name until the user types another. The folder must not exist (our refusal, with Git's wording). Git's error shows in the dialog. "Create Worktree" is enabled in the registry: branch menu, palette and context menu. Only `Actor::Human`.
   - **W4, open** (done). Double-click a row to open the worktree in its own window. The window is another process of this program, started in the worktree folder. It uses the same open path as a path on the command line, and the current window does not wait for it. The "here" row does nothing. A row whose folder is missing warns in the status bar. When Git refuses a branch used by another worktree (W3), the dialog offers "Open that worktree".
   - **W5, remove with checks** (done). The row menu has Open, Reveal in file manager, Copy path and Remove worktree (red). The removal dialog asks the engine for `InspectWorktree`, a read that changes nothing and lists what removal would lose or refuse.
     - Blocks that force can override: dirty (staged, unstaged or untracked), locked (with Git's reason), and commits that no other local or remote branch has, or that are ahead of upstream (the larger of the two counts).
     - Blocks that force cannot override: main checkout, current window, missing folder.
     - Without force, any block refuses. With force, the user ticks a confirmation checkbox instead of typing the name (changed on 2026-10-17, see D31). The engine re-checks at removal time and runs `git worktree remove` (one `--force`, or two for a locked worktree). Never `rm -rf`. Only `Actor::Human`.
   - **W6, lock and clean** (done). The row menu has Lock or Unlock, whichever applies. On a missing-folder row it has Prune Worktrees. Git's refusals show in the status bar. Cleaning has two steps. `ListPrunable` runs Git's dry run, and the dialog shows each record with Git's reason. `PruneWorktrees` runs only on that same list and refuses if the list changed. Folders are not touched, and locked worktrees are skipped, as Git does. The sidebar shows a "locked" badge.
   - **W7, agent path** (`Actor::Agent`): not in this slice. It goes to slice 7 (MCP), per Q12.
   - **W8, branch menu variant** (done). A branch checked out in a linked worktree (not the main checkout) gets a different menu: Open Worktree, Reveal Worktree in File Manager, Copy Worktree Path, New Child Branch, Restack This Stack, and Remove Worktree (red). It uses the same rules as W4 to W6. Checkout, rename, delete and Create Worktree leave the menu. The main checkout keeps the normal menu.
6. **Automatic refresh, no refresh button** (done; design confirmed by the author on 2026-10-16). The button left the top bar. "Refresh" stays in the registry and on `r` as an emergency exit.
   - Premise: the screen never goes stale, and the user never has to ask for an update.
   - Triggers: the window regains focus; changes in `.git` (HEAD, index, refs, `packed-refs`, `MERGE_HEAD` and similar), watched non-recursively; changes in the working tree, watched recursively. The watcher skips what `.gitignore` ignores (`target/`, `node_modules/`) and `.git`.
   - Coalescing: a short debounce (around 150 ms), and never two status calls in flight. The app's own writes (stage, commit, checkout) do not trigger a second refresh beyond what the engine already does.
   - Scale: the watcher has a directory cap. If the system refuses (inotify limit) or the tree is too large, it falls back to refreshing on focus only, and the status bar says so once. Do not turn on `core.fsmonitor` or `untrackedCache` without consent (Spike D).
   - It never writes to the repository. Status runs off the UI thread, with a discreet indicator only past about 300 ms.

**Done when:** the Working Copy is in the simplified design. Every action on screen exists in the registry and is reachable by key, context menu and palette. A disabled action says why.

**Known gaps:**

- Items 1 and 2: none recorded.
- Item 3 (context menus):
  - Commit and stash have no menus yet.
  - The branch menu does not open from the keyboard (the sidebar has no list focus) and does not close with `Esc`.
  - The Discard confirmation is not exercised while the item is disabled.
  - Reveal is not covered by the smoke test (a click would open the file manager). It offers the path even when the file is gone from disk.
  - The menu key uses the pointer position, not the row's position.
  - Enter on checkout has no keyboard target yet.
- Item 4 (palette):
  - Chunk actions show as disabled with "Pick a chunk in the diff first.", because they run from the diff panel.
  - The file list updates only on the next status event.
  - The smoke test covers only the keyboard: not clicking a row, not `Esc` in the argument step, not the palette without a repository.
  - Highlighting is greedy, so with repeated letters it may mark a less obvious letter.
- Item 5 (worktrees):
  - W1: the newline-separated fallback is covered only by parser tests, because the installed Git supports the null-separated form. A worktree removed by hand shows as `prunable` only on the next refresh. The list does not appear on screen until W2.
  - W2: the section does not scroll, so only six rows show. Lock or unlock from the CLI shows only on the next refresh. A folder removed by hand becomes "missing" only on the next refresh, because the watcher does not observe worktree folders. The design's "no repo" badge has no clear Git equivalent; "missing" is used. Bare worktrees do not appear. The color comes from the hash, and the lane rule of D25 does not exist yet. "here" compares paths as Git stores them, so a symlinked path may not match.
  - W3: the base directory setting (`gitmaster.worktree.root`, the design's key) is not read yet, so the suggested folder is always a sibling of the repository. The new-branch and detached modes have real-Git and validation tests, but the smoke test does not walk them. Sparse checkout, stack parent and the design's `post-worktree-add` hook are not in D31 and are out of this sub-slice.
  - W4: no smoke coverage for opening (checked by hand). The new window is a separate process, because each window has its own engine and a window's repository open cannot switch engines. There is no palette action for opening yet.
  - W5: the design's "also delete the branch" and "undo" are not built. Branch deletion is another action, and there is no undo log. Open review comments and the size of ignored files are not computed; the dialog counts only what Git sees in the worktree. Ignored files are deleted with it, with no size warning. The current-window check resolves symlinks, but the registry (which disables Remove) compares text. The dialog has one field, with no branch list.
  - W6: Lock does not ask for a reason (the command accepts one; the UI sends an empty one). The clean dialog does not close with `Escape`, because it has no focused field; it closes with Cancel or a click outside. "Prune Worktrees" appears in the palette, where it opens the dialog with no target. The list shows the record name (`repo-gone`), not the path. Locking the main checkout gets Git's refusal, with no test of its own. Cleaning does not count the locked records Git skipped.
  - W8: "Open in terminal" from the design does not exist (there is no terminal integration). Worktree actions have no registry shortcut, so ⇧⌘C and ⌘⌫ from the design do not show. The top bar's branch menu does not have the variant, only the sidebar row. The variant is not in the palette.
- Item 6 (refresh):
  - External edits within 150 ms after an app write may be missed.
  - One extra refresh on open.
  - macOS (FSEvents) and trees near the 5,000-directory cap are untested.

## Slice 1d: the basics still missing from daily use (done, with gaps)

Requested by the author on 2026-10-19. Decisions D54 to D59 in design/decisions.md. Nothing here was seen in a window. There was no display, so the UI is covered by unit tests of the pure models and the engine by tests with real Git. `scripts/ui-smoke.sh` did not run (`xdotool` and `scrot` are missing), and no new smoke scenario was written.

**Scope**

1. **Copy from the commit panel** (done, D58).
2. **Create and delete branch** (done, D55). Local branches only.
3. **Worktrees** (done, D59). The branch field suggests from local branches (up to 8, no scrolling). Branches already used in another worktree are not marked. The palette lists worktrees to remove, lock and unlock. The sidebar section has "+" and "×".
4. **Resizable sidebar** (done, D57).
5. **Delete merged branches** (done, D56), from the palette ("Delete Merged Branches…").
6. **Fetch, Pull and Push with lease** (done, D54).

**Done when:** the author's five complaints work in the real window: copy; create and delete branch and worktree; resize the sidebar; delete merged branches; Fetch, Pull and Push. Still to check by hand.

**Known gaps:**

- Item 1: file paths are not selectable or copyable. Selection is per area: selecting in one field does not clear the highlight in another. Esc in a focused area sends `Cancel` to the `TextArea` (no effect here, not checked in a window). Drag, double- and triple-click selection and Ctrl/Cmd+C are untested, because the UI crate has no GPUI test harness.
- Item 2: the ⌫ registry action is not bound to a key. "is not fully merged" is recognized by text (with `LC_ALL=C`). The delete dialog is the only confirmation, in the menu and in the palette.
- Item 3: "New Worktree…" has no shortcut. Only the branch field suggests, not the commit field. Lock still sends an empty reason. The section still cuts off at six rows.
- Item 4: windows narrower than about 500 px still zero the main area, because there is no minimum window size.
- Item 5: squash and rebase merges are not detected. The preview shows at most 15 lines and says how many are left, since the dialog does not scroll. If re-reading status fails after a delete, the report is lost.
- Item 6: no timeout or cancel, so a stuck fetch holds the remote slot until Git finishes. `GIT_TERMINAL_PROMPT=0` does not stop the `ssh` password prompt when the app is opened from a terminal. A status read that fails after the operation is silently ignored. No smoke scenario covers the buttons.

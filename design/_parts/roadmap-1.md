# Roadmap

The order of the work. Status marks: **(done)** in the code and verified; **(to do)** not started; no mark = future slice. The order prioritizes reaching something the author uses daily and that already shows a differentiator. The documentation in docs/ is the desired state; this file orders the work toward it, and `scripts/docs-status.sh` lists what the docs still mark as planned.

Last updated: 2026-10-21 (slice H5, lane graph, done). Initial proposal, to be iterated on.

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
- Spike D (done, no fsmonitor verdict): status in a massive repository through a subprocess with fsmonitor. Findings in `spikes/d-status/FINDINGS.md`.
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

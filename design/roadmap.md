# Roadmap

The order of the work. Status marks: **(done)** in the code and verified; **(to do)** not started; no mark = future slice. The order prioritizes reaching something the author uses daily and that already shows a differentiator. The documentation in docs/ is the desired state; this file orders the work toward it, and `make docs-status` lists what the docs still mark as planned.

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

**Next step (author's decision, 2026-10-21; updated at the end of the 2026-10-22 session):** Slice 3c (code navigation, see the Slice 3c section) comes now. Spikes S-A, S-B and S-C and sub-slices N1, N2 and N3 are done. **N4 (syntax highlighting)** is next and already has a spike. H6, H7 and H8 are postponed. After them come the replacement of workflows with scripts (S-D, WS0 to WS2) and WF3. Spike S-D is isolated and can run earlier. H5 is done. See "How to continue" below.

### How to continue (end of the 2026-10-22 session)

**Where the code is.** Everything is on branch `develop` and pushed. This session delivered, in order: H5 (lane graph), spikes S-A and S-C, N1 (Quick Open), N2 (Code page and file viewer), N3 (Search page with `git grep`) and spike S-B. The agent did not see any of this in a window, because the container has no display. The author used the graph and the UI on macOS and confirmed they work. The author has not tried Quick Open, Code and Search yet. `make lint` and `cargo test --workspace` pass (about 1,520 tests). There are no new smoke scenarios (D88).

**Next work: N4, syntax highlighting in the viewer.** S-B answers the risk question (D92). Before writing code, agree with the author:

1. **Confirm D45 and D92** (tree-sitter compiled into the binary; our own queries; the five first-wave languages: Rust, Python, Go, TypeScript/TSX and JavaScript).
2. **Accept the binary cost**: about +5 MB with the five grammars, measured on Linux only. Run `spikes/nav-b-treesitter/measure.sh` on macOS to get that number before deciding.
3. **Colors**: the 26 capture names in the `highlights.scm` files (list in `FINDINGS.md`) become about ten roles (comment, text, number, keyword, operator, function, type, constant, variable or property, punctuation, attribute or macro). Reuse the colors in `theme.rs`, or create new tokens in design/visual-identity.md? The theme contrast tests (4.5:1 for text) then have to cover the roles in both themes.
4. **Where the query files live**: proposed `crates/gm-core/queries/`, one folder per language, seeded from `spikes/nav-b-treesitter/queries/` (`typescript-highlights-combined.scm`, `typescript-overlay-all.scm`, `rust-defs.scm`, `go-overlay.scm`).

**Proposed done-when for N4 (to agree):**

- `gm-core` gets a pure syntax module. The language comes from the file extension (a pure, tested function). `highlight(language, text) -> Vec<Span { start, end, role }>` is deterministic, with spans in order, without overlap and on character boundaries (property test). Grammars sit behind Cargo features. A file whose language has no grammar stays uncolored.
- It runs like the other reads: a job on its own thread with an `OperationId`, stopping when the file changes. It reuses `ReadFile`: the file event carries the spans, or a `FileHighlighted` event follows right after. Measured times: 0.1 to 0.6 ms for 60 lines, 0.4 s for a 1.5 MB file, 3.6 s for an 11 MB minified file. Limits: the existing 2 MiB limit; a line over about 2,000 bytes is not highlighted (the viewer already cuts it); parsing has a maximum time (`Parser::set_timeout_micros`), and a file that exceeds it shows its text without color.
- The viewer paints the spans on visible lines with the role colors. No color until the answer arrives, and no flicker.
- Tests: capture-name to role mapping (pure); a small sample per language with the expected roles; span properties; the engine drops the answer for a superseded file; contrast of the roles in both themes; `make lint` and `cargo test --workspace`. No smoke (D88).
- Out of N4: highlighting in the diff (D42), injections (code inside markdown), semantic highlighting, C and C++ (quality not verified), a user-chosen theme.

**After N4:** N5 (symbols and the single box, D45, D50) uses the same tree-sitter and the `symbols.db` of D92. N6 covers reviews, MCP and the gate. H6 to H8, and WF3 (now preceded by spike S-D and WS0 to WS2, in slice 3w), remain postponed.

**Minor pending items, by value** (all details are in the known gaps of each sub-slice):

- Quick Open: the list of new files (`--others`) takes about 0.75 s on 100,000 files. A two-phase list (tracked files first, new ones after) fixes it. It is reread only when Quick Open opens.
- Search: no streaming and no progress. Clearing the box does not cancel the running Git. Measure `--threads` for `git grep` on macOS (D91).
- Try on macOS: Quick Open, Code and Search (none has been seen in a window). Run `measure.sh` and `run.sh` of S-B.
- Code: the viewer has no text selection and no line wrap. The tree has no keyboard support. "View file" from the commit waits for N6.

**Preparing a new session** (the cloud container starts clean): `rustup update stable` (`rust-toolchain.toml` asks for the newest stable; the container came with a version that is too old), `~/.cargo/bin` first on `PATH`, `apt-get update && apt-get install libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev pkg-config` (to link `gm-ui`), then `make lint` and `cargo test --workspace`. The spike corpora (`~/.cache/gm-spikes`) do not survive. `spikes/nav-b-treesitter/fetch-corpora.sh` downloads the repositories, and `FINDINGS.md` says how to redo the rest. Files the editor reformats (`cargo fmt`) change lines: if a script's text replacement does not match, reread the file.

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

### H6, scope

Pill with the current branch, all refs, or one ref. Left-clicking a branch in the sidebar, on the history page, sets the scope. Checkout stays on double-click or the context menu. "Current stack" shows disabled with "Not available yet" (D34).

### H7, content search

`git log -S` for fixed text. The field sits in the history header, not in the top bar. It is debounced and cancellable (D35).

### H8, performance gate

Benchmarks, ignored by default, on repos of 50,000 and 200,000 commits, generated with `git fast-import` (D37).

Out of slice 3 (author's decision on 2026-10-17): stack scope, commit owner, stack parent, dashed edge, Stacks tab, Restack counter, Fetch and Pull, reflog, stashes and local review. The stack items stay "Not available yet".

Done when: browses the history of a large repository with smooth scrolling.

## Slice 3d: branch filter in the sidebar (done, with gaps)

Decision D53 (design/decisions.md, proposed on 2026-10-19). The sidebar shows "Mine" (the current branch, and branches whose tip commit is the user's) or "All", with a search by name and a "N of M" count. If no branch matches, "No branches match" shows, with a link to show all. Two actions in the command palette, "Show Only My Branches" and "Show All Branches", switch the scope. The one that changes nothing is disabled, with the reason. The palette and the top selector still list all branches. The chosen scope is saved in `settings.json` (D40).

- Done when: the scope, search and default rules are tested in a pure module (`branch_filter.rs`). Tree rows come from the filtered set, with no empty group header (`sidebar.rs`). The actions have tested rules and plans. The scope survives a restart (round-trip test of the file).
- Known gaps: nothing was seen in a window. The smoke scenario was neither written nor run. On this machine GPUI does not start (no software Vulkan driver), so the UI has not been checked on screen. Remote branches with no readable author are hidden under "Mine". The filter is recomputed every frame, with no cache. Keyboard navigation inside the filtered tree did not change. The search text is not saved. "Mine" looks only at each branch's tip commit. The "Local" section disappears when there are no local branches, also without a filter.

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

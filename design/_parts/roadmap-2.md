## Slice 2: `prepare-commit` hook (done)

Backend and UI are done. Reviewed on 2026-10-16 (D29, D30): only what Git lacks becomes a GitMaster hook. The `pre-commit` hook and the other native Git hooks stay with Git (D18, D29). It will move to a workflow in WF3 (D75).

- Versioned JSON contract in `gm-schema`. Input: current message, staged diff and repo. Output: message. Always JSON (D30).
- Configured as `[gitmaster "prepare-commit.<name>"]`. A repo-level hook runs only after an approval kept outside the repo, tied to the command's hash (D17).
- Hooks run with the login shell's PATH, like `git`.
- Runs are async. The message header shows the run state (step, ready, failed with a one-line reason, cancelled) and a "Cancel" button while it runs. "Suggest message" starts a run (`commit.suggest_message`).
- The answer fills the message only if HEAD, the staged diff and the text are all unchanged. Otherwise it is offered as a suggestion, with "Replace" and "Discard" (keeps the text).
- For a repo-level hook, the panel shows the full command with "Approve" and "Deny". Deny hides the panel for this session only; nothing is saved.
- Each finished step appears in the "Git commands" panel as `hook <name>: <command>`, with exit code, duration and a one-line result (`ok`, `exit 2`, `timed out after 30000 ms`, `cancelled`). Input, output and diff never enter the log (D28).

Done when: a script the author writes can generate the commit message without freezing the UI, and the native `pre-commit` keeps running as before.

Known gaps:

- The command log does not store the command hash (approval uses it; the log shows the full command). A cancelled or timed-out step shows as a failure on the "Git log" button.
- A failed step shows only its exit code in the log. The first line of the hook's error output appears on the chip only.

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

- Flat list of HEAD's first 200 commits, in 48 px rows: ref chips (HEAD, branches, tags), subject, short hash and relative time.
- HEAD with no commits shows a message instead of an empty list.
- "Show History" action in the action registry. The sidebar opens the page.
- Read command, not logged (D28, D33).

Done when: the page lists the real commits of HEAD for linear repos, merges, root commits, HEAD with no commits, detached HEAD and Unicode subjects (checked against real Git), and the UI smoke passes in both themes.

Known gaps:

- Ref chips cannot tell local refs from remote ones (`%D` does not say).
- The list refreshes only when the page opens (H2). No paging beyond 200. No selection. No keyboard shortcut for "Show History".
- The list uses Git's default date order, not `--topo-order` (H5).
- Relative time uses the author date, computed at draw time.

### H2: paging and refresh (done)

- Pages of 200 commits. Near the end of the list, the next page is requested from the last listed commit (cursor by oid). The `git log --topo-order` output is streamed and resumes where it stopped. If the cursor does not match, the read restarts and reads up to it.
- Each watcher status update rereads the top while the page is open, so a commit made outside the app appears at the top.
- A click selects a row by oid. The selection survives updates.
- Reads run on their own thread. A superseded request's answer is dropped by `OperationId`, and its git process is killed. Nothing is written to the commit-graph or the command log.

Done when: a 5,000-commit repo with merges (made with `git fast-import`) pages through exactly what `git rev-list --topo-order HEAD` lists, with no repeats or gaps. An outside commit appears at the top with the selection kept. Superseded answers are dropped. The UI smoke passes in both themes.

Known gaps:

- Refresh rereads only the first page, so rows beyond it come back on scroll.
- Every status update rereads the top, even when only the working tree changed.
- Paging has model and engine tests only: no UI test and no smoke run with 5,000 commits (the performance gate is H8).

### H3: commit detail (done)

- A 360 px panel beside the history: subject, full body (paragraphs kept), full hash, author name and email, author date, short parents, and the file list. Each file has a status letter (A, M, D, R; a rename shows `old → new`) and + and − counts. Binary files show `bin`. More than 200 files show "and N more". No selection shows "Select a commit to see its details."; an error shows Git's own message.
- Merges compare with the first parent (`--diff-merges=first-parent`). A root commit lists every file as A.
- Read on its own thread, with the H2 request model. A new selection kills the ongoing read. Switching repo ends it. Read command, not logged.

Done when: real-Git tests cover add, modify, delete, rename, binary, root commit, merge, multi-paragraph body, Unicode and the file limit. The engine drops superseded answers and stops the read. The UI smoke passes in both themes.

Known gaps:

- `git show` output is read fully into memory before the 200-file limit applies.
- The hash and parents wrap to two lines. The date has no relative time.
- File rows are not clickable yet (H4). Clicking the same row again rereads the commit.
- No UI test for the panel; the smoke checks pixels, not text.
- Killing a running `git show` has no real-Git test.
- Non-UTF-8 names show replacement characters.
- The design's "Range" is not included (outside H3's scope).

### H4: read-only commit diff (done)

- Clicking a file row shows that file's diff in place of the list and panel. Up and Down move to the previous and next file. Esc or "Back to commit" returns to the commit. The header has the path, `+ −`, "N chunks, N insertions, N deletions", and the hunks, as in the Working Copy. There are no stage, chunk or Staged | Unstaged controls.
- Merges compare with the first parent. Root commits use `--root`. Renames show both paths. Binary files show "Binary file".
- The diff pane gained a read-only mode rather than a new component. The view matches the Working Copy's, minus the actions.
- Read on its own thread, with the H2 and H3 request model. Emits no stage.

Done when: real-Git tests cover add, modify, delete, rename with edits, binary, root commit, merge, mode change, large file and non-UTF-8 content. The engine drops superseded answers, stops reads and emits no stage. The UI smoke passes in both themes, with the other history and staging scenarios green.

Known gaps:

- "Back to commit" does not stop the ongoing read. Its answer is dropped when the read ends.
- A non-UTF-8 file name shows replacement characters, and the diff comes out empty, because the path is not passed back to Git.
- A mode change with no content shows "No text changes in this file.", the same as an empty file.
- Up and Down skip the "and N more" files.
- The smoke covers add and delete only.

### H4b-A: split commit diff (done)

- A "Unified | Split" selector in the commit diff header. The choice is saved per user in `settings.json` (D39, D40). Default: Unified.
- Split puts old and new lines side by side, one row per line pair. The pairing is the same as the D24 pair warning. A missing side is blank, with no number or marker. Horizontal scroll is shared by both sides.
- Cursor, navigation and selection follow the unified lines, mapped to their split rows. Changing layout keeps the cursor line.
- No automatic switch to Unified in narrow windows (D41).

Done when: pairing tests (2 vs 3 lines, pure addition, pure removal, interleaved lines, context) pass, and each changed line appears once on one side. Real-Git tests for add, remove and rename with edits pass. The UI smoke passes in both themes, and the choice survives a restart.

Known gaps:

- No keyboard shortcut for the layout.
- Horizontal scroll works with the wheel only, in fixed steps. Its limit is the widest line, not the visible width.
- The choice is saved only from the header. Its own screen is slice 9 (D38).
- Split and unified rows have separate draw functions, so a visual change to one must be checked in the other.
- No UI test for the split panel.

### H4b-B: split Working Copy diff (done, with gaps)

- The same selector and row model as the commit diff. The choice is shared: switching either diff changes both and saves the choice (D39, D40). Stage, unstage and discard work as before. The D24 half-pair warning applies to both.
- Each split cell has its own line box. Only the cursor's cell is highlighted. Keys are the unified ones (D43): Up and Down walk the changed lines in both views, so a −/+ pair visits each of its cells in turn.
- It also closes H4b-A gaps. A second Down pressed while a file loads is no longer lost. The "\ No newline at end of file" marker is drawn after the line text, in both layouts.

Done when: selection by cell and by unified line gives the same result, and with real Git the written index matches. The split-stage UI smoke passes in both themes (single-line stage from the cell checkbox, a second `s` on a half pair, Stage Chunk, and a git check). The other staging and history smoke scenarios stay green in both layouts.

Known gaps:

- No keyboard shortcut for the layout, and no keys to jump between the cells of a pair.
- No UI test for the split panel. The smoke checks git and pixels.
- The half-pair warning is checked by pixels, not by text.
- The end-of-file marker is tested only as a text function. No smoke scenario draws it.
- The horizontal offset resets when the file or side changes. There is no unit test, because the state lives in the GPUI entity.
- Intra-line highlighting waits until after slice 3 (D42).

### H4c: stacked commit diff (done, with gaps)

- Clicking a file shows one continuous diff of all the commit's files, in order, like `git show`, scrolled to the clicked file. Each file has a header with its status letter, its path (old → new for renames) and `+ −`. Binary, renamed-without-text, mode-change and empty files show a notice instead of lines. A file over 5,000 lines shows "and N more lines, not shown.". Past 200 files, the last line reads "and N more files, not shown.". The header shows "N files" and the Unified | Split selector, which applies to every file.
- Keys (D43 to D46). Neither diff binds bare letters. Keys are Ctrl+Shift plus a letter, which is ⇧⌘ on macOS (D44). Ctrl+Shift+N and Ctrl+Shift+B are the next and previous hunk in the Working Copy, and the next and previous file in the commit view. The table below lists them. Tests check that there are no bare letters, no repeated key per screen, and no diff key that clashes with a registry shortcut on macOS (the exception is Ctrl+Shift+P, see Q13).

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
- One read of the commit patch (`git show --format= --patch --histogram --diff-merges=first-parent -M --root`), split per file and paired with the file list by position. It runs on its own thread, with the H3 and H4 request model. Read command, not logged.

Done when: pure tests cover the stacked layout, the notices, the per-file line limit, and the file-at-line and step rules. Real-Git tests cover two files in order, rename, binary, mode, root commit and merge. The engine cancels superseded reads and writes nothing. The UI smoke passes in both themes, with the related history and staging scenarios green, including the split variants.

Known gaps:

- The whole commit patch is in memory before the 200-file limit applies, as in H3 and H4.
- The file header scrolls away; it does not stick.
- The "current" file for next and previous is the one holding the top line.
- The 5,000-line limit hides the rest of a file, with no way to view it on this screen.
- Up and Down move one fixed line, with no acceleration.
- Non-UTF-8 names show replacement characters, as in H4.
- The space at the end is one page, computed from the window height. Scroll-to-top works only after the window has been drawn.
- Not tested on macOS, including the ⇧⌘ shortcuts (D44).
- Ctrl+Shift+P is still the Push With Lease label in the registry (Q13). The Working Copy file list still uses j, k and r (Q14).

### H5: lane graph (done, with gaps)

- History reads now request parents (`%P`): full oids, first parent first. The order is still `--topo-order`, page by page.
- A pure, deterministic layout, built in one top-to-bottom pass that does not look ahead. Each new page extends the layout without redoing earlier rows. Lanes still open at a page's end carry over to the next page. Each commit becomes one row: its node column, plus the lanes that pass, arrive, leave or shift. Drawing only paints the segments.
- The graph column sits left of the subject, up to 8 columns. Beyond that, the row shows `+N`. Rows join seamlessly across the list. The 48 px row height is unchanged. HEAD's node has a ring.
- Colors: five lane accents (not state colors) and a faint trunk color. The theme contrast test covers all six in both themes, in normal and selected rows.

Done when: `%P` tests (root, linear, merge, octopus) pass, and parents match `git rev-list --parents --topo-order`. Layout property tests pass: one row per commit, every parent edge drawn to its parent, the same input gives the same layout, and pages extend a single-pass layout. Case and geometry tests pass (linear, fork and merge, octopus, two roots, parent outside the page, repeated parent, over 8 columns). A real repo with two merges draws every edge. `make lint` and `cargo test --workspace` pass. The author used the graph on macOS on 2026-10-21 and confirmed it works and that the UI is good to use.

Deviations from what was agreed: the layout runs on the UI thread, not off it, because it takes microseconds (20,000 commits in about 45 ms in a debug build, so a 200-row page takes well under 1 ms). The timing test is 20,000 commits in under 2 s in debug, not 200 rows in under 5 ms in release. A lane born from a merge takes its color from the hash of the oid it expects, not from the branch name. A single pass does not look ahead, and looking would change rows already drawn when a new page arrives.

Known gaps:

- No smoke scenario and no UI test of the drawing (D88).
- A lane that shifts changes tint where it joins.
- The `+N` label counts columns in the row, not lanes hidden in the list.
- No hover highlight for lanes.
- The graph column takes 14 px even in linear history.
- Local and remote branch names (`feature` and `origin/feature`) get different colors, because `%D` does not say which is which.
- No dashed edge and no commit owner (D25 stays Proposed).
- The graph shows only HEAD. Choosing other scopes is H6.

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

# Screens

Screens: design decisions. Last updated: 2026-10-19. Status of each decision in `design/decisions.md`. The interactive mockups were made in the design conversation of 2026-10-16. This document keeps the decisions, not the exact look.

Visual references from mature desktop Git clients: a sidebar with a folder tree by prefix, a Working Copy with the commit composer above the file list, a diff with "Stage/Unstage chunk", and a commit context menu.

## Branch sidebar (D10, decided)

Two organizations, with a switch ("By stack" and "By name"):

- **By stack**: an indented tree by dependency. "Stacks" and "No stack" sections, so people who do not use stacking do not pay the cost of the feature. Gradual adoption: the tool works like a regular Git client until the first branch gets a parent.
- **By name**: the classic view, grouped by prefix (`feature/`).

Indicators:

- Action on the **parent** (`restack 3`), because the parent is what changed. Outdated children get only an amber dot.
- `↑N`: commits ahead of the **parent** (not of origin).
- Dragging a branch onto another sets the parent. This uses the same idiom as dragging a commit onto the sidebar, common in desktop Git clients.
- State is calculated outside the UI thread.
- Virtualized list (`uniform_list` in GPUI), designed for hundreds of remote branches.

## Drag a file to move it (D109)

Drag one changed file from the Working Copy list onto a sidebar row. The ghost shows the file name. Each row answers while the pointer is over it:

- **Accepts**: highlighted, with the hint `Commit to <branch>` or `Move to <worktree>`.
- **Refuses**: dimmed, a "not allowed" pointer and the reason in the hint (conflicts, branch checked out elsewhere, missing folder, Git older than 2.38). The branch and the worktree of the window never accept.
- **Checking**: the conflict check runs after a short pause on the row, in the background, and is cancelled when the pointer leaves. The hint reads `Checking…` until the answer arrives.

Drop on a worktree: runs at once and the status bar offers **Open Worktree**. Drop on a branch: a popover on that row asks for the commit message. Drop on the Worktrees header or `+` opens New Worktree; drop on the Branches header or a prefix group asks for a branch name. The same idiom as dragging a branch to set its parent. The keyboard path is the palette and the file menu.

## Restack flow

Four steps: Plan, Running, Conflict, Done. Details in design/stacks-restack.md.

## Conflict resolution (D22)

Reference: conflict resolvers in IDEs. Three panes: left, result (editable), right.

- **Sides are labeled by role, not by ours/theirs.** In a rebase, `ours` and `theirs` are inverted. Show: the branch name, the role ("new parent") and the commit subject.
- Apply on its own what does not conflict, showing the result ("3 changes applied").
- Three actions per hunk: left, right, both. Base on demand (`merge.conflictStyle = zdiff3`).
- Counter "N of M conflicts resolved". Applying with conflicts pending replies with a message, instead of disabling the button.
- **rerere** reapplies recorded resolutions, showing "resolved by recorded resolution" with an option to review or forget it.
- The `resolve-conflict` hook enters as a third labeled option ("script suggestion"), never applied on its own.
- The operation log records who resolved each hunk (human, rerere or script).
- Alternative layout plan (unified or stacked) for small screens (Q9).

## History and graph (D25)

- The **stack lag shows in the graph**: a dashed amber edge when a child branch starts from a commit that is not the parent's tip, with a badge "N commits behind parent".
- **Commit owner comes from the stack metadata** (range `stackParentBase..tip`), with no heuristic based on ref names. Lane color comes from a hash of the branch name (stable across sessions). Do not rely on color alone.
- Two modes: **Commits** and **Stacks** (each node is a branch with its commits, collapsible).
- **Default scope: the stack of HEAD plus `main`**, with a visible chip "Scope: current stack" and an option "all refs". This matters in large repos with many people.
- Deterministic lane layout, in one top-down pass, with checkpoints every N lines so the user can scroll without recalculating.
- Performance: a `commit-graph` file (generation numbers) and changed-path Bloom filters. The revwalk streams in a separate thread.
- Content search (pickaxe, `git log -S` and `-G`).
- Risk: the per-commit graph is only legible with few lanes. Test early with a large real repository.

## Working Copy (D23)

- **Staged and read are orthogonal axes.** Staged means "goes into the commit". Read means "I have reviewed it". "Read" invalidates itself when the hunk's hash changes. It works as a reminder, never as an approval seal.
- One list with a tri-state checkbox (indeterminate) per folder, an always-visible filter, and grouping by directory.
- The composer is always visible, with the `prepare-commit` hook chip. A suggestion does not overwrite what the user typed. The button becomes "Replace". The chip can be cancelled. The `pre-commit` hook is Git's native hook (D29): it gets no chip, and its output appears as the commit error.
- Scale: status streamed ("scanning…"), `core.fsmonitor`, `core.untrackedCache`, sparse-checkout, a virtualized list, and generated files and lockfiles collapsed.
- Keyboard first: `j`/`k` navigate, space stages, `r` marks as read, `a` stages the hunk, `⌘↵` commits.

## Diff and line staging (D24)

- **Exact model, filtered view.** Staging always operates on the exact diff. "Ignore spaces" is only a display filter.
- Mechanics: build the new index blob (the index base plus the selected lines) and write it directly with libgit2, atomically. Testable invariant: staged plus unstaged equals the total change.
- Selection: a checkbox per line in the gutter, a counter per hunk, and a Vim-style visual mode (`V` selects, `s` stages, `u` unstages).
- Warning for a **−/+ pair staged in half**. The index would be left without the old line and without the new one, and may not compile.
- `histogram` (or `patience`) algorithm with an indentation heuristic. Moved-code detection (`--color-moved`). Intra-line diff only in visible hunks.
- Generated files: diff and staging collapsed via `.gitattributes` attributes (read-only).
- Inline review comments, with the agent's before and after (see docs/agents/mcp-server.md and docs/agents/local-review.md).

## Workflows page

The page to view, create, check and run workflows (D76, D84, D98, D101, D233). Workflows are edited in the person's own editor, not here. It has two tabs: **Script** and **Runs**.

- **Script**: the file as it is on disk, read only, with line numbers, the viewer's colors and the problems of the check under it. The page follows the file: a save in the editor shows a moment later.
- **Open in Editor** starts `gitmaster.editor` (or the system's opener) on the file; **Copy Path** copies its path.
- **Runs**: one entry per `ctx` call, with its resolved arguments, result, outcome and duration. **Cancel** and **Approve** act on the run. Today they are palette commands only (Cancel Workflow Run, Approve Workflow Step); the buttons are planned.
- **Dry run** runs on the file as it is on disk.
- **Delete** moves the file to the trash, so it can be recovered. It never deletes with `rm`.

The Flow and TOML tabs were removed in WS2 (D108). The TOML specification is in git history (commit 5f88889).

## Settings screen (D38)

Planned for slice 9 (premium finish), the settings screen is a modal that gathers all app and Git settings in one place.

- **Access**: `⌘,` on macOS, `Ctrl+,` on Linux and Windows. The registered action is `open_settings` in `action_registry.rs`. It also appears in the command palette as `Open Settings`, and as a gear button in the footer. It closes with `Esc`, a click outside it, or the close button.
- **Persistence without a Save button (Principle 5)**: each change is saved at once, when the value changes or the text field loses focus. Writes stay within these boundaries:
  - Git config: written with `git config`, to `~/.gitconfig` (global scope) or `.git/config` (repository scope).
  - App settings: written to `settings.json` in the user's config directory (`~/.config/gitmaster/`, or under `$XDG_CONFIG_HOME`).
  - Security: written to `hook-approvals.json` in the user's data directory (`~/.local/share/gitmaster/`).
  - Nothing is written inside the repository working tree.
- **Sections**: General, Git & Identity, Worktrees (D31), Diff & Files, Hooks & Security (D17, D29, D30), Stacks & Restack, Conflict Resolver, MCP & Agents, and Local Review.

For the field-level details of each section, see docs/reference/configuration.md.

## Labels page and provenance panel (D144, D145; P4, checked in a window on 2026-11-01)

Sidebar entry **Labels** under Threads (palette: Show Labels, New Label, Rename Label, Delete Label). Left column 280 px: a filter box and a list of labels, each with a colour dot, name, the agent's name as a tag when an agent made it, and the attachment count. Right pane: name, description, "Created by", then attachments grouped as Files, Commits, Threads and Labels (a group with none is left out). Each row shows its title, who attached it (You or the agent's name) and when; a commit shows **gone**, **ambiguous** or **rewritten** when its state says so; a file renamed since shows its current path. **Remove** (×) shows on hover, and always on a row an agent attached. **New Label…** has name, description and `#rrggbb`; Rename and Delete are in the header and the row's right-click menu; Delete says how many attachments go.

**Why This Code Is Here:** select lines on the Code page, right-click, and a 360 px panel opens beside the viewer: the range, the commits that own the lines (short id, subject, author, date, lines) and, under each, its threads with the evidence in the order of the docs, `ambiguous`, `unverifiable` edits, the entries around each edit, labels and linked threads, and a cut notice. An edit opens its thread at that entry. With no linked thread it says "No thread is linked to these lines".

Thread rows and headers show label chips (click to filter the list; a **Label: x** chip clears it). **Link to Thread…** is in the header's ⋯ menu; relations show under the header with direction, the agent's name and **Remove**.

Checked in a window (light and dark, Xvfb, 1280×800): the list, the detail pane, the New Label dialog, the viewer menu and the provenance panel with a commit and no thread. Not checked: the narrowest width (the app has no minimum, and below about 840 px the detail pane is squeezed), a result with threads and edits, label chips on the Threads page and the Link to Thread dialog. Known defect: the viewer menu's disabled reason ("Select lines of the file first.") runs past the menu's right edge.

## Threads page (D119, D125; planned)

Sidebar entry **Threads** under Workspace, with a count of threads. History's pattern: a virtualized list on the left, the detail on the right, a search box and filters in the header (agent, branch, date, starred, has edits, touched file).

- **List row:** title, agent chip, branch, relative time, counts (`12 edits`), a `redacted` chip when anything was redacted, a dot while the session is open.
- **Detail, by turn:** the human's prompt, the agent's text, then tool calls as one-line collapsed chips (tool, argument summary, status, duration). An edit is a card: path, `+n −m`, a state (`in working tree`, `in HEAD`, `changed since`), **Open Diff** and **View File**. A redaction shows as a muted `[REDACTED:name]` chip, never as a blank.
- **Search:** one box, no syntax (D50, D128). Results show snippets and open the thread at the matching entry.
- **Header actions and menu:** Rename, Add Note, Star, Tags, Link to Commit, Copy as Markdown, Export, Delete (red, asks for confirmation and says what goes). Entries have **Delete Entry**.
- **Entry points elsewhere:** **Show Threads** on a Working Copy file and on a commit's detail; a `Threads` section in the commit panel lists the threads created during.
- **Empty states:** no integration installed (shows the install step), no threads yet, capture paused (shows the reason, for example an invalid pattern).
- Labels in English; icons from `assets/icons/`; colors from the theme tokens (design/visual-identity.md). Nothing is seen in a window until T5.


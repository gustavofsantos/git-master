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

The page to view, create and edit workflows in the UI (D76, D84, D93, D96). It has two tabs: **Script** and **Runs**.

- **Script**: the text of the script, with line diagnostics. Saving shows a warning about the unreviewed hash.
- **Runs**: one entry per `ctx` call, with its resolved arguments, result, outcome and duration. **Cancel** and **Approve** act on the run.
- **Dry run** runs on the open text.
- **Save** is atomic. It refuses when the file changed on disk.
- **Delete** moves the file to the trash, so it can be recovered. It never deletes with `rm`.

The earlier Flow and TOML tabs are described in git history (commit 5f88889).

## Settings screen (D38)

Planned for slice 9 (premium finish), the settings screen is a modal that gathers all app and Git settings in one place.

- **Access**: `⌘,` on macOS, `Ctrl+,` on Linux and Windows. The registered action is `open_settings` in `action_registry.rs`. It also appears in the command palette as `Open Settings`, and as a gear button in the footer. It closes with `Esc`, a click outside it, or the close button.
- **Persistence without a Save button (Principle 5)**: each change is saved at once, when the value changes or the text field loses focus. Writes stay within these boundaries:
  - Git config: written with `git config`, to `~/.gitconfig` (global scope) or `.git/config` (repository scope).
  - App settings: written to `settings.json` in the user's config directory (`~/.config/gitmaster/` on Linux).
  - Security: written to `hook-approvals.json` in the user's data directory (`~/.local/share/gitmaster/`).
  - Nothing is written inside the repository working tree.
- **Sections**: General, Git & Identity, Worktrees (D31), Diff & Files, Hooks & Security (D17, D29, D30), Stacks & Restack, Conflict Resolver, MCP & Agents, and Local Review.

For the field-level details of each section, see docs/reference/configuration.md.

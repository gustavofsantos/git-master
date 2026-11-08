# Slice 2: Changes

## Scope

1. Composer above the file list (Tower layout). The commit block moves to the top of the left column; its resize strip moves to its bottom edge. The composer keeps its subject counter, Suggest message, the prepare-commit suggestion with Replace / Discard (it never overwrites typed text) and **Commit on <branch>** with ⌘↵.
2. Checkbox per file and tri-state per folder. `change_tree.rs` (pure) groups the files under their folder (one level: the file's parent directory) and gives each file a box (checked = all staged, mixed = part staged, unchecked = none; a conflicted file has none) and each folder the sum of its files. A click on a file's box runs Stage File or Unstage File; a click on a folder's box runs the new registry actions **Stage Folder** / **Unstage Folder** (one Stage/Unstage command per file of that folder). j/k and Space keep working on files; the list scrolls to the selected file through the row mapping.
3. Compact diff header: path, `+a −r`, the chunk count and the switches (Staged | Unstaged, Unified | Split) on one row.
4. Half-staged banner: the warning bar becomes a banner with **Select the pair**, which adds the missing half of each split −/+ pair to the selection, so staging takes whole pairs.

Chunk actions on the @@ header already exist (Stage / Unstage / Discard chunk) and stay.

## Decisions made here (least change)

- **Amend** is not built: it is not in the docs and it adds a Git operation (`commit --amend`) the app does not run today. Left as a question for Gustavo; no space reserved beyond the composer row.
- Subject and description stay one text area: the first line is the subject (the counter already measures it). Splitting the field changes how messages are typed and saved; left for Gustavo.
- Agent-edited files showing thread, turn and edit state: not built in this slice. The data exists per thread (provenance), not per working-copy file; a per-file read is a new engine query. Recorded as a gap.
- Folders are one level (the parent directory), not a nested tree: a tri-state per folder needs no collapse state and the flat order of the status is kept.

## Tasks

| # | Who | Task |
|---|---|---|
| 1 | Opus | `change_tree.rs`, Stage/Unstage Folder actions in the registry, tests. |
| 2 | Haiku | Composer move to the top (`shell_list.rs`, `shell_commit.rs`, `resize_handle.rs` Bottom edge). |
| 3 | Opus | File list rendering with folder rows and boxes (`shell_list.rs`), scroll mapping. |
| 4 | Opus | Diff header compaction and the pair banner (`diff_pane.rs`). |
| 5 | Haiku | Docs: working-copy.md, diff-and-staging.md, first-steps.md (composer at the top), keyboard shortcuts unchanged. |

## Acceptance

- `git add -p` + commit stays replaceable: chunk and line staging unchanged; staging invariant tests (`gm-core` stage_math, `*_real_git.rs`) pass.
- `make check` green.

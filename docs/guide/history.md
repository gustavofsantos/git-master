---
title: History
description: The commit list of HEAD, the commit details, the diff of a commit, copying a hash or message, and the commit graph.
status: partial
order: 30
---

The History page shows the commits of HEAD, with a graph that draws how they connect. Select a commit to see what it changed, then open one of its files to read the diff. The page follows the repository: a commit made outside GitMaster appears at the top of the list while the page is open.

## Open the history

1. Click **History** in the sidebar, or run **Show History** from the command palette (`Ctrl+K`, `⌘K` on macOS).
2. The page needs an open repository. If HEAD points at a branch that has no commits yet, the page says so instead of showing an empty list.

Reading the history does not add entries to the Git commands panel.

## The commit list

Each row shows one commit.

| Part | What it shows |
| --- | --- |
| Ref chips | **HEAD**, the branches and the tags that point at the commit |
| Subject | The first line of the commit message |
| Hash | The short hash |
| Time | The relative time of the author date, such as `just now`, `5 min ago`, `2 days ago` |

The commits are listed in topological order. The header shows the number of commits loaded: `12 commits`, or `First 200 commits` while more pages can be loaded.

### Paging

The list loads 200 commits at a time. When you scroll near the end of the list, the next page loads from the last commit shown.

### Updates

While the page is open, the top of the list is read again whenever the repository changes. A commit that you make outside GitMaster, for example with the `git` command line, appears at the top. Your selection stays on the same commit.

## Select a commit

Click a row to select it. The row is highlighted and the commit details open in the panel on the right. The selection survives updates to the list.

## Commit details

The panel shows the selected commit:

| Field | What it shows |
| --- | --- |
| Subject | The first line of the message |
| Body | The full message, with its paragraphs |
| Hash | The full hash |
| Author | The author's name and email |
| Date | The author date |
| Parents | The short hashes of the parents. A root commit shows `none` |
| Files | Each changed file, with its status letter and the count of added and removed lines |

The status letters are:

- **A** for an added file.
- **M** for a modified file.
- **D** for a deleted file.
- **R** for a renamed file. The path shows `old → new`.

A binary file shows `bin` instead of line counts.

Two rules apply to the file list:

- For a merge commit, the list compares the commit with its first parent.
- For a root commit, every file is listed as added.

The list shows up to 200 files. When the commit changed more, the last line says `and N more`.

If no commit is selected, the panel says **Select a commit to see its details.** If Git cannot read the commit, the panel shows Git's message.

You can select and copy the text of the subject, body, hash, author, date and parents. File paths cannot be selected, because a click on a file opens its diff.

## Open the diff of a file

1. Click a file in the **Files** list of the commit details.
2. The diff of the commit opens, with the file you clicked at the top. The diffs of the other files of the commit follow it, in the same order as the list.
3. Each file has a header with its status letter, its path and the count of added and removed lines. The header above the diff shows the number of files, and the **Unified | Split** selector chooses the layout.

The diff is read-only. It has no stage, unstage or discard controls. Lines show their numbers. The layout you choose is saved and shared with the diff of the Working Copy. For how a diff is read, see [Diff and staging](diff-and-staging.md).

| Key | Action |
| --- | --- |
| `Up`, `Down` | Scroll one line |
| `PageUp`, `PageDown` | Scroll one page |
| `Ctrl+Shift+N` (`⇧⌘N`) | Go to the next file. At the last file, it goes to the first |
| `Ctrl+Shift+P` (`⇧⌘P`) | Go to the previous file. At the first file, it goes to the last |
| `Esc`, or **Back to commit** | Return to the commit details |

A binary file, a rename with no text changes and a mode change show a notice in place of lines. A file with more than 5,000 lines shows its first lines, then `and N more lines, not shown.` When the commit changed more than 200 files, the last line of the diff says `and N more files, not shown.`

For the full key table, see [Keyboard shortcuts](../reference/keyboard-shortcuts.md).

## Copy the hash or message

The commit details have two copy controls:

- **Copy**, beside the hash, copies the full commit hash.
- **Copy message**, beside the body, copies the subject and the body together.

Both actions are also in the command palette as **Copy Commit Hash** and **Copy Commit Message**. They act on the selected commit.

## The commit graph

The graph sits to the left of the subject. It draws one row per commit and one line from each commit to each of its parents.

- A **lane** is a column of the graph. The graph shows up to 8 lanes. When a row needs more, the row shows `+N`, where N is the number of lanes beyond the eighth.
- The commit that HEAD points at has a ring around its node.
- Each lane takes a color from the name of the branch it starts from. The color is the same in every session.
- The trunk branches `main`, `master` and `trunk`, local or remote, use a quiet color.
- The graph does not rely on color alone. Lanes sit in their own columns, and the ring marks HEAD.

As you load more pages, the graph grows with the list. A commit keeps its row when more commits are loaded below it.

## Planned

> **Planned.** The scope control is not available yet.

The scope control chooses which commits the list shows: the current branch, all refs, one ref, or the current stack. The list shows the commits of HEAD until the control is available.

## Stacks in the graph

> **Planned.** Stack-aware graph is not available yet.

When a branch is stacked on a parent, the graph draws the edge between them dashed and amber. When the branch starts from a commit that is not the tip of its parent, the edge carries a badge with the text `N commits behind parent`. A **Commits | Stacks** switch changes the graph: **Commits** shows the commits, and **Stacks** shows one node per branch, with its commits folded under it. Each commit belongs to the stack that owns it, which is read from the stack metadata and not from the branch name. For stacks, see [Stacks](stacks.md).

## Search by content

> **Planned.** Search by content is not available yet.

A search field in the history header finds commits that add or remove a text, the same way as `git log -S`. Changing the text to a regular expression uses `git log -G`. The search waits briefly after you stop typing, and a new search cancels the one in progress.

## File history

> **Planned.** File history is not available yet.

The history of one file lists the commits that changed it, in the same list and detail panel as the history of HEAD.

## Blame

> **Planned.** Blame is not available yet.

Blame shows, for each line of a file, the commit that last changed it.

## Browse files at a commit

> **Planned.** Browsing files at a commit is not available yet.

You can open the tree of files as they were at a commit and read any of them in the code viewer. For the code viewer, see [Code navigation](code-navigation.md).

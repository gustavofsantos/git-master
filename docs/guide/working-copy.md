---
title: Working Copy
description: The changed-files list, staging files, the commit composer, automatic refresh and the Git commands panel.
status: partial
order: 10
---

The Working Copy shows the changes in your repository that are not committed yet. Use it to choose what goes into the next commit, write the commit message and commit.

## Changed files

The header reads **Working Copy** and shows how many files changed, for example **6 changed**. Each changed file takes one row, with a state letter and its full path:

| Letter | State |
| --- | --- |
| `M` | Modified |
| `A` | Added |
| `D` | Deleted |
| `?` | Untracked: a new file that Git does not track yet |

A subtle mark next to the letter shows that the file has something staged.

Select a file to see its diff. The diff is described in [Diff and staging](diff-and-staging.md).

## Stage and unstage a file

Staged changes go into the next commit. Stage a whole file when all of its changes belong in the commit:

1. Select the file in the list.
2. Press `Space` to stage it. On a staged file, `Space` unstages it.

You can also right-click the file, or select it and press `Shift+F10`, and choose from the file menu:

| Item | What it does |
| --- | --- |
| **Stage file** | Stages the file. |
| **Unstage file** | Unstages the file. |
| **Copy file path** | Copies the path of the file. |
| **Reveal in file manager** | Shows the file in your file manager. |
| **View File** | Opens the file in the read-only viewer of the [Code navigation](code-navigation.md) page. |

**Stage file** and **Unstage file** are disabled when there is nothing to do. A disabled item shows the reason in place of its shortcut.

To stage part of a file, see [Diff and staging](diff-and-staging.md).

## Commit

The composer sits at the bottom of the Working Copy. It holds the commit message and the **Commit on <branch>** button, where `<branch>` is the branch you are on.

1. Stage the changes you want in the commit. Only staged changes are committed.
2. Type the commit message in the composer.
3. Click **Commit on <branch>**.

Git's own hooks run during the commit, such as `pre-commit`. If a hook stops the commit, its output appears as the error of the commit. The same output is recorded in the Git commands panel, described below.

### Commit message suggestions

> **Planned.** Workflows that suggest a commit message are not available in the app yet.

A workflow can suggest a message in the composer. A suggestion never overwrites the text you typed. It appears as a suggestion, with **Replace** to take it and **Discard** to keep your text. See the [workflows overview](../workflows/overview.md).

## Automatic refresh

The list never goes stale, so you do not need to refresh it. GitMaster updates the list when:

- The window gets focus.
- Git's state changes in `.git`, such as `HEAD`, the index or the branch references.
- A file in the working tree changes. Files that `.gitignore` ignores, such as `target/` or `node_modules/`, do not count.

Changes made outside GitMaster, for example in a terminal or an editor, appear here without any action. The **Refresh** action, bound to `r`, is still available if you want to force an update.

Reading the status runs in the background, so the window stays responsive. A small indicator appears only when the read takes longer than about 300 ms.

If the operating system stops GitMaster from watching more folders, or the working tree is too large to watch, the list refreshes only when the window gets focus. The status bar says so once.

## Large repositories

> **Planned.** The behavior for very large repositories described here is not available in the app yet.

On a very large repository, the status is read as a stream, and the list shows a scanning state until the read finishes. The list draws only the rows you can see, so long lists stay smooth. GitMaster does not turn on `core.fsmonitor` or `core.untrackedCache` without your consent. Sparse checkout is supported.

## Discard changes

> **Planned.** Discarding all changes in a file is not available in the app yet.

To discard part of a file, use **Discard Chunk** in [Diff and staging](diff-and-staging.md).

## Reviewed marker

> **Planned.** Marking a file as reviewed is not available in the app yet.

A reviewed mark is a reminder that you read a file's changes. It is independent of staging: marking a file as reviewed does not stage it, and staging does not mark it. The mark clears by itself when the change in the file changes. It is a reminder, never an approval.

## Generated files and lockfiles

> **Planned.** Collapsing generated files and lockfiles in the list is not available in the app yet.

Generated files and lockfiles are collapsed in the list. GitMaster reads the rules for generated files from `.gitattributes` and never writes to it.

## Commit and stash menus

> **Planned.** Context menus for commits and stashes are not available in the app yet.

## Git commands panel

Open the **Git commands** panel from the status bar. The panel is read-only. It lists each Git command that changed something: stage, unstage, commit, checkout, fetch, pull, push, and worktree and branch operations. Reads, such as status, diffs and history, are not listed.

Each entry shows the program, its arguments, the exit code, the duration, the standard output and the standard error. Failed commands are highlighted, and the output expands when you open it. The list is kept in memory and holds the last 1000 entries.

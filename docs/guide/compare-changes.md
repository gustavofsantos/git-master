---
title: Compare changes
description: The tabs of the Changes page that show the uncommitted changes, the changes of the branch against its base, and the changes between any two revisions.
status: planned
order: 15
---

> **Planned.** The tabs, the revision pickers and the read-only diff described here are not available in the app yet. The Changes page shows the uncommitted changes only.

The **Changes** page has three tabs at the top of its left column. Each tab answers one question about your code: what you have not committed, what your branch brings, and what separates two revisions.

| Tab | Question it answers |
| --- | --- |
| **Uncommitted** | What have I changed since the last commit? |
| **Branch** | What does this branch change compared with the default branch? |
| **Compare** | What changed between this revision and that one? |

**Uncommitted** is the [Working Copy](working-copy.md), with the commit composer, staging and every other action of that page. It is the tab that opens first. **Branch** and **Compare** are read-only: they never stage, discard or commit anything, and they have no commit composer.

The tab you choose lasts while the window is open. Opening another repository goes back to **Uncommitted**. The command palette has one action per tab: **Show Uncommitted Changes**, **Show Branch Changes** and **Compare Revisions…**.

## Branch changes

The **Branch** tab shows what the branch you are on brings to its base: the changes of its own commits, as if you reviewed it before merging.

The header names the two sides, for example **feature/billing against main**, with the number of files and the lines added and removed, such as `12 files · +340 −96`.

The base is the default branch: the branch of `origin/HEAD`, else the first of `main`, `master`, `trunk` and `develop` that exists. Choose another base in the **Base** picker, which lists the local and the remote branches. When the branch is in a [stack](stacks.md), its parent is listed first and marked **parent**.

The tab compares the tip of the branch with the **merge base** of the branch and its base, the commit where the two histories split. Commits that landed on the base after that point are not shown as changes of your branch. Uncommitted changes are not part of this tab: they are on **Uncommitted**.

| Situation | What the tab shows |
| --- | --- |
| You are on the base branch itself | **You are on the default branch.** Choose another base in the picker to compare with it. |
| HEAD is detached | The commit HEAD points at, named **HEAD (detached)**, against the base. |
| The branch has no commits of its own | **No changes against `main`.** |
| The branch and the base share no history | **`feature` and `main` have no common ancestor.** |
| No default branch is found | **No default branch found.** Choose a base in the picker. |

## Compare two revisions

The **Compare** tab shows the changes between two revisions you name. A revision is anything Git accepts: a branch, a remote branch such as `origin/main`, a tag, a full or short commit hash, `HEAD`, or an expression such as `HEAD~3` or `v1.2^`.

1. Open the **Compare** tab, or run **Compare Revisions…** from the command palette.
2. Type the older revision in **From** and the newer one in **To**. Each field suggests branches and tags as you type. Press `Enter` or click a suggestion to take it.
3. Read the file list and the diff.

**To** starts as `HEAD` and **From** starts empty. Until you fill **From**, the tab says **Type the older revision in From.** **Swap** exchanges the two fields, to read the same change backwards.

The **Mode** selector chooses what is compared:

| Mode | Compares | Use it for |
| --- | --- | --- |
| **Direct** | The tree at **From** with the tree at **To**, exactly. | Two tags, a release and a branch, any two commits. |
| **Since common ancestor** | The merge base of the two revisions with **To**. | What **To** brings over **From**, the way **Branch** does. |

**Direct** is the default. The header shows what was resolved, such as `v1.4.0 (a1b2c3d) → main (e4f5a6b)`, so a moving branch name is tied to the commit it pointed at when the tab was read.

When a revision cannot be resolved, the field is marked and the message of Git appears under it, for example **unknown revision `v9`**. The list keeps the last result until both fields are valid. The tab never changes your repository, so a wrong name costs nothing.

Comparing a revision with itself shows **No changes between these revisions.**

## Changed files and diff

Both tabs show the changed files the way a commit's file list does, and the diff of every file in one continuous view, as in the [stacked diff of a commit](diff-and-staging.md#stacked-diff-of-a-commit):

- Each file has its state letter (`M`, `A`, `D`, `R` for renamed), its path (a renamed file shows `old → new`) and its `+` and `−` counts.
- Click a file to scroll the diff to it. The **Unified | Split** selector applies to every file.
- A binary file, a renamed file without text changes and a file whose mode changed show a notice in place of the lines.

The same two limits apply: a file with more than 5,000 lines shows `and N more lines, not shown.`, and after the first 200 files the last line reads `and N more files, not shown.`

The diff is read-only. Select a file and open its menu (right-click, or `Shift+F10`) to **Copy File Path**.

## Staying current

The tab reads again by itself when the repository changes: a commit, a fetch, a checkout or any other move of a reference. A revision typed as a name, such as `main`, follows the name, so the result is up to date. A revision typed as a hash never moves.

Reading runs in the background, so the window stays responsive, and a new request cancels the one still running. A small indicator appears only when a read takes longer than about 300 ms. These reads do not add entries to the [Git commands panel](working-copy.md#git-commands-panel).

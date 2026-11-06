---
title: Code navigation
description: Open files, browse the project tree and search text in the active worktree with Quick Open, the Code page and the Search page.
status: partial
order: 60
---

GitMaster lets you find a file, read it and search the text of the project without leaving the app. Quick Open finds files by name, the **Code** page shows the folder tree with a read-only viewer, and the **Search** page finds text across the files. Everything runs on your machine, against the files on disk and the data Git already keeps, so there is no network to wait for.

## Why navigate in GitMaster

When you review a change, you often need the code around it: the file a change touches, the other places a name appears, the module a function comes from. Hosted code services answer these questions in a browser. GitMaster answers them in the same window as your changes, from the repository on your disk, so you keep your place in the review.

## The active worktree

Code navigation works on the worktree of the window, the folder with the checkout you opened. See [Worktrees](worktrees.md) for what a worktree is.

It reads the files as they are on disk now. That includes uncommitted edits and new files, and it does not show an older commit. A file is read again each time you open it, so a change made by another program appears the next time you open that file.

Quick Open and Search cover this folder only. Other branches, tags and commits are covered in [Browsing revisions](#browsing-revisions).

## How Git decides what is in the project

GitMaster keeps no list of project files of its own. What counts as part of the project is what Git reports.

- The file list and the text search follow `.gitignore` and sparse checkout the way Git applies them. A file that sparse checkout leaves out of the folder is not listed or searched.
- Reading never changes your repository. Nothing is written into the working tree, and searches and file reads are not listed in the **Git commands** panel.
- Only `git` is required. GitMaster does not need ripgrep, ctags or a language server.

## Speed

Quick Open answers from a file list kept in memory. Each text search runs in the background, so the window keeps responding while it works. When you start a new search, the previous one is stopped, and a late answer from it is discarded.

## Quick Open

Quick Open is the file finder. Press `Ctrl+P` (`⌘P` on macOS), or choose **Quick Open…** in the [command palette](command-palette.md). It needs an open repository.

### Find a file

1. Press `Ctrl+P` (`⌘P` on macOS). A box opens over the window.
2. Type part of the file name or of its path. You do not need the whole name.
3. Press Enter, or click a result, to open the file in the **Code** page.

The results update as you type. Each result shows the file name, with the letters you typed highlighted, and its folder in a quieter style. Up to 50 results are listed and 10 are visible at a time. Scroll the list to see the rest.

To open a file at a line, type a colon and the line number after the name, for example `diff_pane.rs:120`. The file opens at line 120, and that line is marked.

Press `Esc` to close Quick Open without opening anything.

### How results are ranked

- A match in the file name counts for more than a match in its folder. So `diff_pane` finds `diff_pane.rs` ahead of files that only sit inside a folder with that name.
- Matches at the start of a folder or file name, and letters typed next to each other, rank higher.
- A query that spells out the whole file name ranks higher still.
- Results that rank the same are ordered by path, so the same query always gives the same list.

### What the file list contains

Quick Open lists what Git reports for the worktree:

- every file Git tracks;
- every new file that `.gitignore` does not hide.

Ignored files are not listed. A tracked file that you deleted from disk stays in the list until Git stops tracking it. Opening it shows that it is not in the worktree now.

### Submodules

The files of a submodule that is checked out are part of the project: Quick Open lists them, the folder tree shows them under the submodule's folder, and text search looks inside them. Their paths start with the submodule's folder, such as `service-a/src/main.rs`. Each submodule follows its own `.gitignore`. A submodule that is not checked out has no files to list. Only the submodules of the repository you opened are read, not the submodules of a submodule.

In a [workspace](workspaces.md), this lets you find a file or a name in every member at once.

The list is read again each time you open Quick Open or the **Code** page. New files appear the next time you open either one.

## The Code page

Open the **Code** page from the sidebar, or choose **Show Code** in the command palette. The page has two panes: the folder tree on the left and the viewer on the right. Opening a file from Quick Open opens the folders above it in the tree and scrolls the tree to the file.

### Folder tree

- Folders start closed. Click a folder to open it, and click it again to close it.
- Folders are listed first, then files. Each group is sorted by name.
- A folder whose only content is one other folder is shown as one row, such as `a/b/c`.
- Click a file to show it in the viewer.

### File viewer

The viewer shows the file as read-only text.

- Each line has its number in a gutter on the left.
- Long lines are not wrapped. Scroll sideways with the mouse wheel.
- Tabs are shown four columns wide.
- The header shows the path of the file and its size. If only part of the file is shown, the header says so.
- Until the next file has loaded, the previous file stays on screen.

### Files the viewer does not show as text

| Situation | What you see |
|---|---|
| Binary file | The notice **Binary file** in place of the content. |
| File deleted from disk since the list was read | A notice that the file is not in the worktree now. |
| Symbolic link | The link and its target. The viewer never follows the link. |
| Empty file | An empty viewer, without an error. |
| Text that is not UTF-8 | The text, with replacement characters where the bytes are not valid UTF-8. |
| Larger than 2 MiB | The first part of the file, cut at a line break, with the header note "showing the first part". |

The viewer reads only regular files inside the active worktree. Paths that point outside the worktree are refused, and named pipes and devices are not read.

### Syntax highlighting

The viewer colours code by language. The text appears first, and the colours follow a moment later, so opening a file never waits for them.

| Language | Files |
|---|---|
| Rust | `.rs` |
| Python | `.py`, `.pyi`, `.pyw` |
| Go | `.go` |
| JavaScript | `.js`, `.mjs`, `.cjs`, `.jsx` |
| TypeScript | `.ts`, `.mts`, `.cts` |
| TSX | `.tsx` |
| Java | `.java` |
| C | `.c`, `.h` |
| C++ | `.cc`, `.cpp`, `.cxx`, `.c++`, `.hh`, `.hpp`, `.hxx`, `.h++`, `.ipp` |
| C# | `.cs` |
| Ruby | `.rb`, `.rake`, `.gemspec`, `.ru`, `Rakefile`, `Gemfile` |
| Bash | `.sh`, `.bash`, `.zsh`, `.bashrc`, `.bash_profile`, `.profile`, `.zshrc` |
| JSON | `.json`, `.jsonc`, `.geojson` |
| TOML | `.toml`, `Cargo.lock` |
| YAML | `.yml`, `.yaml` |
| HTML | `.html`, `.htm` |
| CSS | `.css` |
| Clojure | `.clj`, `.cljs`, `.cljc`, `.cljd`, `.edn`, `.bb` |

A header with the `.h` extension is read as C, which also reads most C++ headers.

Files in other languages stay plain, and so do binary files.

Twelve roles are coloured, in code and in data and markup files alike: comments, strings, numbers, keywords, operators, functions, types, constants, parameters and names the language reserves (such as `self` and `this`), properties, punctuation, and attributes, macros, decorators and tags. A plain name that is none of these keeps the text colour, so the code does not turn into a wall of colour. Every role keeps a contrast of at least 4.5 to 1 against the viewer background, the selected line and a text selection, in both the light and the dark theme.

Colouring gives up, and the file shows in plain text, in these cases:

- The file needs more than 2 seconds to read. A 2 MiB file of ordinary code takes under 1 second.
- A line is longer than 2,000 characters. Such a line, typical of a minified bundle, stays plain while the rest of the file is coloured.
- Another file is opened before the colours arrive. The colours of the file you left are dropped.

A file with a syntax error is still coloured as far as the parser can read it.

### Keyboard in the file tree

> **Planned.** Keyboard navigation of the file tree is not available in the app yet.

The file tree is navigated with the keyboard, not only with the mouse.

## View File

**View File** opens a file in the viewer on the **Code** page. It is available in two places.

From the Working Copy:

1. Right-click a file in the [Working Copy](working-copy.md) list.
2. Choose **View File**.

From the command palette:

1. Press `Ctrl+K` (`⌘K` on macOS) to open the [command palette](command-palette.md).
2. Type **View File**.
3. Press `Tab` and choose the file.

## Search text

The **Search** page finds text in the files of the active worktree. Open it from the sidebar, with `Ctrl+Shift+F` (`⇧⌘F` on macOS), or choose **Search Text…** in the command palette.

### Start a search

1. Type the text in the search box.
2. Set the switches you need. See the table below.
3. Optionally, narrow the files in the files box.

The search starts 250 ms after you stop typing. Press Enter to start it at once. Changing a switch also starts the search at once.

### Switches

| Switch | Effect |
|---|---|
| **Aa** | Match case. |
| **ab** | Whole word: the text must match complete words. |
| **.\*** | Regular expression: the text is read as a pattern. |
| **new** | Include new files: files Git does not track yet, unless `.gitignore` hides them. Searches include them by default. |

### Example: find every use of a name

1. Press `Ctrl+Shift+F` (`⇧⌘F` on macOS) to open the **Search** page.
2. Type `parse_config` in the search box.
3. Turn on **ab**, so that `parse_config_file` is not matched.
4. Wait for the summary to show the number of matches, or press Enter to start at once.
5. Click a line in the results. The file opens in the **Code** page at that line.

### Limit the files

The files box takes words separated by spaces. Each word is a Git pathspec, so `src` matches everything under the `src` folder. A word that starts with `!` excludes those files. For example, `src crates/ui !docs` searches `src` and `crates/ui` and skips `docs`. Leave the box empty to search every file.

A word that names a submodule's folder, or a path inside it, such as `service-a/src`, applies to that submodule. A word that starts with `!` and names a submodule's folder leaves the whole submodule out.

### Read the results

- Results are grouped by file. Each group shows the file name, its folder, and the number of matches.
- Each matching line is shown with the matched text highlighted.
- Click a file header to fold or unfold its lines.
- Click a line to open the file at that line in the **Code** page.
- The summary line above the results shows the state of the search. While it runs, it says **Searching…**. When it ends, it gives the number of matches and of files. If Git reports an error, for example for an invalid regular expression or pathspec, the summary shows Git's message as it is.

### Limits and order

- A search returns up to 1000 lines in total and up to 100 lines per file. When the limit is reached, the search stops, and the summary says that the results were cut off.
- A line longer than 300 bytes is cut around the matched text.
- Binary files are skipped.
- Results are ordered by path, then by line number. The same search gives the same results in the same order.
- The results of the files in submodules are ordered with the others, by their full path.

### Regular expressions

With **.\*** on, the text is read as a regular expression in Git's extended syntax. Perl shorthand such as `\d` is not available. Write `[0-9]` instead.

## Browsing revisions

> **Planned.** Browsing files at a revision is not available in the app yet.

Browsing lets you read the files of any branch, tag or commit, in the same read-only viewer.

- On the **Code** page, choose a revision to browse it.
- In the context menu of a commit, choose **Browse files at this commit**.

## View file from a commit

> **Planned.** Opening a file from a commit is not available in the app yet.

From the diff or the detail of a commit, choose **View file** to see that file as it was in the commit.

## What it does not do

- It does not edit files. The viewer is read-only, and nothing is written into the worktree.
- It does not index symbols: there is no symbol search, file outline, go to definition or find usages. Use the **Search** page, with the **ab** switch (whole word) for a name.
- It does not provide language-server features, such as semantic references or renaming.
- It does not search Git LFS content, or the submodules of a submodule.

## Comment on a range of lines

Select lines in the viewer by clicking a line number, then Shift-clicking another line number. Then press **Ctrl+Shift+M**. The comment goes to the local review loop, anchored to that text; see [Local review](../agents/local-review.md#comment-on-code).

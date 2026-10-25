---
title: Code navigation
description: Open files, browse the project tree and search text in the active worktree with Quick Open, the Code page and the Search page.
status: partial
order: 60
---

GitMaster lets you find a file, read it and search the text of the project without leaving the app. Quick Open finds files by name, the **Code** page shows the folder tree with a read-only viewer, and the **Search** page finds text across the files. Everything runs on your machine, against the files on disk and the data Git already keeps, so there is no network to wait for.

## Why navigate in GitMaster

When you review a change, you often need the code around it: the file a change touches, the other places a name is used, the module a function comes from. Hosted code services answer these questions in a browser. GitMaster answers them in the same window as your changes, from the repository on your disk, so you keep your place in the review.

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

Files in other languages stay plain, and so do binary files.

Twelve roles are coloured: comments, strings, numbers, keywords, operators, functions, types, constants, parameters and names the language reserves (such as `self` and `this`), properties, punctuation, and attributes, macros, decorators and tags. A plain name that is none of these keeps the text colour, so the code does not turn into a wall of colour. Every role keeps a contrast of at least 4.5 to 1 against the viewer background, the selected line and a text selection, in both the light and the dark theme.

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

### Limit the files

The files box takes words separated by spaces. Each word is a Git pathspec, so `src` matches everything under the `src` folder. A word that starts with `!` excludes those files. For example, `src crates/ui !docs` searches `src` and `crates/ui` and skips `docs`. Leave the box empty to search every file.

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

### Regular expressions

With **.\*** on, the text is read as a regular expression in Git's extended syntax. Perl shorthand such as `\d` is not available. Write `[0-9]` instead.

## Symbol search

> **Planned.** Symbol search is not available in the app yet.

Symbol search finds named definitions such as functions, types and classes. Each result shows its kind, for example `fn`, `struct`, `class` or `trait`, with its file and line.

- Press `Ctrl+Shift+O` (`⇧⌘O` on macOS) to see the outline of the current file.
- Type `#` in Quick Open to search symbols in the whole repository.
- Symbols are found for Rust, TypeScript, JavaScript, Python and Go. Files in other languages are still found by name and by text, but have no symbols.
- The index is built in the background and is kept in `.git/gitmaster/`. While it is still being built, results say that they are partial. Deleting it costs only a rebuild, and it never sits in your working tree.

## Go to definition

> **Planned.** Go to definition is not available in the app yet.

Go to definition moves from an identifier to the place where that name is defined.

- Click an identifier in the viewer, or search for the name in symbol search.
- When one definition matches, it opens directly. When several match, all of them are listed, and you choose one. GitMaster never guesses.
- The search matches names. It does not resolve scope or types, so it cannot tell which definition the code actually uses.

## Find usages

> **Planned.** Find usages is not available in the app yet.

Find usages lists every place a name appears as a whole word. It uses the same text search as the **Search** page.

The results are text matches, not references. The list says so. A name used for two different things appears under both.

## One search box

> **Planned.** The single search box is not available in the app yet.

One search box decides from the text you type which searches to run, so you do not need any syntax. Results appear in sections, in a fixed order.

| You type | Sections, in order |
|---|---|
| A name, such as `diff_pane` or `DiffPane` | Files, Definitions, Usages |
| A file, such as `diff_pane.rs` or `diff_pane.rs:120` | Files |
| A call, such as `open(` | Definitions, Calls |
| Several words, punctuation or quotes, such as `"not available yet"` | Text |

Within a section, results are ordered by how well they match, then by path and line. Each new keystroke cancels the searches that are still running.

## Browsing revisions

> **Planned.** Browsing files at a revision is not available in the app yet.

Browsing lets you read the files of any branch, tag or commit, in the same read-only viewer.

- On the **Code** page, choose a revision to browse it.
- In the context menu of a commit, choose **Browse files at this commit**.

## View file from a commit

> **Planned.** Opening a file from a commit is not available in the app yet.

From the diff or the detail of a commit, choose **View file** to see that file as it was in the commit.

## MCP read tools

> **Planned.** The read tools for code are not available in the MCP server yet.

Agents can search files, search text and symbols, and read a range of lines of a file through the MCP server, with the tools `files_search`, `code_grep`, `symbols_search` and `file_read`. They only read, so they need no approval. See the [MCP server](../agents/mcp-server.md) and the [MCP tools](../reference/mcp-tools.md) reference.

## What it does not do

- It does not edit files. The viewer is read-only, and nothing is written into the worktree.
- It does not provide language-server features, such as semantic references that find every use of a symbol by its meaning.
- It does not rename symbols.
- It does not search inside submodules or Git LFS content.

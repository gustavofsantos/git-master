# Code navigation: engineering design

> **D163 (author, 2026-11-02):** symbols, the outline, go to definition, find usages and the single box are removed from the product. The sections **Single box (D50)** and **Symbols** below are kept as history only; the highlighting findings of spike S-B still hold (D111, D112).

The user-facing behavior is specified in `docs/guide/code-navigation.md`; this file keeps the engineering rationale.

Last updated: 2026-10-25. Status: scope decided (D43). S-A, S-B, S-C, N1, N2, N3 and N4 are done (`design/roadmap.md`, D89 to D92). D44 and D46 were backed by the spikes and by N3. D45 has the spike (D92); the author confirmed it for highlighting (D111) and it stays **proposed** for the symbol index until N5. The rest (N5 and N6, D47 to D51) stays **proposed** in `design/decisions.md`.

## Why

Anyone who reviews agents' changes (Slice 8) or navigates a monorepo needs to find context without leaving the app: open a file by name, search text in the repository, jump to a symbol's definition. Hosting services offer this on the web. Locally it should be faster, because the data is already on disk and there is no network.

This goes beyond an ordinary Git client, on purpose: the product is a Git client joined to tools for working with code, and existing clients are references, not a template (`docs/concepts/principles.md`, Positioning). The scope is decided (D43); the position on the roadmap is still open (Q13).

## What is in

A **Code** page in the sidebar, next to Working Copy and History, and three searches:

| Search | Question it answers | Proposed shortcut |
|---|---|---|
| **Files** (Quick Open) | "open `diff_pane.rs`" | `Ctrl/⌘+P` |
| **Text** (grep) | "where `HistoryStream` appears" | `Ctrl/⌘+Shift+F` |
| **Symbols** | "where `fuzzy_match` is defined" | `Ctrl/⌘+Shift+O` (current file) and `#` in Quick Open (repository) |

Plus a **read-only viewer** with a folder tree, which is where the three searches lead. The shortcuts must be checked against the action registry (`action_registry.rs`); the palette's `Ctrl+K` is already taken.

"Where you stand" means: the active worktree (D31), as it is on disk. Any revision (branch, tag, commit) comes in the last sub-slice (N6).

## Principles applied

- **Git is the source of truth (3).** The file list comes from `git ls-files`, and text search from `git grep`. The app has no notion of its own of "what belongs to the project": `.gitignore`, sparse-checkout and submodules apply as Git applies them.
- **Nothing in the working tree (5).** The symbol index lives in the app's data directory (or in `.git/`, see D45), never next to the code.
- **The UI never blocks (4).** Every search runs on its own thread, with a delay (debounce), is cancellable (it kills the process, like the history's `Canceller`), and a late answer to a superseded request is dropped by `OperationId`. It is the same model as H2 to H4.
- **Determinism (1).** Same input, same result order: the ranking's tie-break is the path, never the order in which threads finish. Navigating to a definition never "guesses": if there is more than one candidate, it lists them all.
- **Reads do not go to the command log (D28).**

## Smoothness as a condition

Structural analysis (tree-sitter) is welcome, but only if the app stays smooth and fast. In practice: the index runs in the background, on a few threads at low priority; no search waits for it (a partial result is flagged); syntax highlighting only processes the visible range; and the D49 gate includes "scrolling and typing without frame drops during indexing". If spike S-B shows that this cannot be met, the first version ships without symbols (files and text), and symbols wait.

## No extra programs (D51)

GitMaster is self-contained: search may not require the engineer to install anything beyond what the app already requires. The only external program is `git` (D26, D32), which Working Copy and History already use. Left out, as executables, are ripgrep, ctags, ast-grep and language servers (LSP). Any additional engine enters as a **Rust library compiled into the binary** (tree-sitter for symbols; `regex` and `ignore` if `git grep` ever falls short), and its compile-time and size cost goes into the spikes.

## Single box (D50)

The user types what they want, with no syntax. A **deterministic classifier** (rules, no LLM, Principle 1) decides which engines run, all in parallel, and the answer arrives in sections that appear as each engine finishes:

| What was typed | Engines | Sections |
|---|---|---|
| `diff_pane`, `DiffPane` (identifier, no space) | files + symbols + usages | Files, Definitions, Usages |
| `gm-ui/diff`, `foo.rs`, `foo.rs:120` | files | Files (opens on the line, if any) |
| `foo(` or `foo()` | symbols + usages, with a call hint | Definitions, Calls |
| several words, punctuation, quotes (`"not available yet"`) | literal text | Text |

- Sections come in fixed order. Within each, score by rule and tie-break by path and line, never by arrival order. Same input, same list.
- Each keystroke cancels the earlier engines (kills their processes), after a short delay.
- Syntax is never required. Filters (`lang:`, `path:`, case, regex) may exist as shortcuts for those who want them, but the box without them must already cover day-to-day use.
- "Semantic" in the sense of search by meaning (describe in natural language and find the code) is something else: it depends on embeddings or an LLM, is not deterministic, and stays out of scope here. An agent can do it through MCP (`code_grep`, `symbols_search`) and propose; the app does not decide on its own.

## File search

- **List:** `git ls-files -z` of the active worktree (tracked files) and, optionally, `--others --exclude-standard` (untracked files that are not ignored). For a revision, `git ls-tree -r -z --name-only <rev>`.
- **Structure:** a single byte buffer with offsets, sorted, with no `String` per path. A monorepo of 1 million files with 60-byte paths costs about 60 MB, not hundreds.
- **Invalidation:** the existing watcher (`gm-core/src/watch`) reports changes to the index, HEAD and refs. The list is rebuilt in the background, and the old one serves until the new one is ready. The rebuild is streamed, with "indexing…" visible, because `ls-files` in a large repository takes seconds.
- **Ranking:** the current `fuzzy.rs` serves actions and branches (hundreds of items). Paths need a variant: a bonus for the file name over the directory, for the start of a segment and for the extension, `/` as a word separator, and parallel search with a top-K heap instead of sorting everything. `fuzzy_match` allocates a `Vec<char>` per call, which does not scale to 1 million candidates. The variant works on bytes (falling back to Unicode only when the query is not ASCII).
- **UI:** reuses `palette_view` (virtualized list, matched fragment highlighted) with a results provider that differs from the actions provider. The choice between "palette mode" and "own overlay" is left to N1, after reading `palette.rs`.
- **Useful filters:** `foo.rs:120` opens at line 120; a folder prefix (`gm-ui/ diff`) narrows the search.

## Text search (grep)

**D44, proposed: `git grep` as the default engine.** Same argument as D32: it is already installed, respects `.gitignore`, works on any revision (blobs) and on the worktree, uses several threads, and streams. Base call:

```
git grep -I -n --column -z --no-color [-i] [-w] [-F|-E|-P] [--untracked] -e <pattern> [<rev>] -- [<pathspec>]
```

- `-I` ignores binaries; `-z` makes the output safe for names with unusual characters; `--column` requires Git 2.19+ (check the app's minimum version).
- **Options in the UI:** match case, whole word, regex (`-E`; `-P` only if Git was built with PCRE), and a pathspec field ("include" and "exclude" with `:(exclude)`), as in hosting services.
- **Limits:** for example 1000 lines in total and 100 per file, with an explicit "N more". The search stops when it hits the limit (it kills the process). Long lines are truncated around the matched fragment.
- **Result:** grouped by file, with a count, the matched fragment highlighted, and a click that opens the viewer at the line. Deterministic order: by path, then by line. The output of `git grep` is parallel, so it is re-sorted before display, or the search runs with `--threads=1` if the cost stays low.
- **Built-in alternative:** if the N3 benchmark shows that `git grep` is not enough, the way out is an in-process search (Rust crates `regex` and `ignore`, compiled into the binary), never an external program (D51).
- **Safety:** the pattern always goes after `-e`, and the pathspec after `--`, so that typed text is never read as an option.

## Symbols

This is the hard part and the one with the highest risk. Four paths:

| Path | For | Against |
|---|---|---|
| **tree-sitter** with `tags.scm`, compiled into the binary | In process, no external dependency, deterministic, incremental; the same grammar provides the viewer's syntax highlighting | One grammar (C code) per language increases build time and binary size; coverage is per language |
| `universal-ctags` | Covers dozens of languages | External program the engineer would have to install (D51): dropped |
| LSP | Precise references and definitions | Heavy, one server per language and per project, non-deterministic state, contradicts "local-first, simple" |
| Regex via `git grep` (`fn \w+`, `class \w+`) | Zero dependencies | False positives and negatives, per language; only serves as plan B |

**D45, proposed: tree-sitter compiled into the binary, with `tags.scm` queries, for a curated set of languages.** It is a library, not a program to install (D51). It remains syntax, not semantics: it does not resolve scope or types (D47). Design points:

- **Classification cache** keyed by blob OID, like the definitions index.

- **Content-addressed index.** The key is the blob OID (or the content hash, for a file modified in the worktree). The same blob on another branch, or in history, reuses the result, and nothing goes stale: the index is derived and disposable (Principle 3). Losing the file only costs a rebuild.
- **Where to store it:** SQLite in `.git/gitmaster/` (the SQLite stores are in `gm-core`, see `design/architecture.md`) or in the app's data directory. Decide in the spike; both respect Principle 5.
- **Built in the background**, at low priority, with a thread limit and visible progress ("symbols: 12,340 / 98,000 files"). Symbol search works with the partial index and says that it is partial.
- **Search:** fuzzy over the name, with a filter by kind (`fn`, `struct`, `class`, `trait`…), file name, and line alongside. The "file overview" (outline) uses the same table, filtered by file.
- **Go to definition** (click, or a shortcut on an identifier in the viewer): looks up the name in the index. One candidate opens directly; several are all listed. No semantic scope resolution.
- **Usages:** "Find usages" is `git grep -w <name>`, and the interface says these are text occurrences, not semantic references. A language without a grammar can still be found by name and by text. We will not pretend to a precision the approach does not have; precise references need LSP, which is out of scope (D47).
- **Languages, in waves:** Rust, TypeScript/JavaScript, Python, Go; then Java, C/C++, C#, Kotlin, Ruby, Swift. A file in a language without a grammar can still be found by name and by text; it just has no symbols.

### Result of spike S-B (2026-10-22)

Details and numbers are in `FINDINGS.md` of the nav-b-treesitter spike (git history, 7932c61); decision proposed in D92. What changes in this design:

- **Tree-sitter compiled into the binary is feasible** (D45): about 9 s on one core per million lines; 109 s to index 104 thousand files on 4 threads (peak of 66 MB); five grammars add about 5 MB to the binary. Only Linux was measured.
- **Our own queries, not the crates' ones.** TypeScript's `tags.scm` alone finds 35% of definitions; added to JavaScript's, 87%; with a supplement for enum, alias and function-field, 99.1%. Rust's counts associated types as types; Go's does not see `type X = Y`. TypeScript's `highlights.scm` also adds to JavaScript's. The seeds are in `queries/` of the nav-b-treesitter spike (git history, 7932c61).
- **Definitions by simple tree walk** where the language allows it (Rust, Python, Go): same result as the query, 2.6 to 3 times cheaper. Query where context is needed (TypeScript, JavaScript). The query does not get cheaper without the reference patterns; "Find usages" remains `git grep -w` (above).
- **Index**: `.git/gitmaster/symbols.db` (SQLite), key: blob, language and query version; 1.19 million symbols in 1.8 s and 113 MB; exact and prefix searches under 0.1 ms; fuzzy search by name uses the N1 matcher, not SQLite.
- **Highlighting**: parse on its own thread; only visible lines are painted (0.1 to 0.6 ms per 60 lines); a 2 MiB limit in the viewer; a maximum parse time; and no highlighting on very long lines.
- **Left out of the first version of symbols:** C, C++ and C#, whose quality was not verified (40% of the files in git/git have syntax errors). Highlighting includes them (D112): an error there only costs the color around it.

## Viewer

- Read-only: shows a file from the worktree (read from disk) or a blob from a revision (`git cat-file --batch`, a long-running process, or a libgit2 blob). No editing (Q7 covers the conflict resolver's editable text, not this). "Open in editor" uses `core.editor`, as already planned in `design/screens.md`.
- Virtualized by line, reusing what `diff_pane` has already solved (width measurement, horizontal scrolling, 4-column tabs). Line number, "go to line", copy `path:line`.
- Guards: a binary becomes "Binary file"; a file above a limit (order of magnitude: 2 MB) opens truncated with a warning, and never freezes; content that is not UTF-8 shows replacement characters, as in the diff.
- **Syntax highlighting** (N4) comes from the same tree-sitter, with `highlights.scm`, and only for the visible range. The diff gets highlighting for free later, but that is a separate decision.
- **Folder tree:** generated from the file list, collapsed by default, with single-child folder paths compacted (`a/b/c`), as in hosting services.
- **Entry points outside the Code page:** the file row in the diff and in the commit detail gains "View file"; the file menu item in Working Copy does too. After H3/H4, it is just a new action in the registry.

## Scale

The targets are massive repositories (`docs/concepts/principles.md`). Therefore:

- No list grows without limit on screen: everything is virtualized, and results have a ceiling.
- No search runs without cancellation; changing the query kills the previous process.
- Sparse checkout: the worktree file list has only what exists; search in a revision may have more. The interface must say which of the two the user is looking at.
- **Performance gate (D49)**, in the style of H8: ignored benchmarks, run on repositories generated with `git fast-import` of 100 thousand and 1 million files. Initial targets, to be calibrated in the spike: first usable file list in seconds, with "indexing…"; a keystroke in Quick Open responds in under 50 ms with the list ready; first grep result in under 1 s on 100 thousand files; the symbol index never raises CPU enough to hurt the UI.

## MCP

Read-tier tools (D20), cheap because the engine already has them: `files_search`, `code_grep`, `symbols_search`, `file_read` (with a line range). Reading needs no approval. They come in Slice 7. The engine must be born with `Command`s and `Event`s that the UI and MCP share (rule 1 of `design/architecture.md`), so nothing here is "UI only".

## Division into sub-slices

Each one is vertical, usable on its own, and has a "Done when" with real Git, like those of Slice 3.

0. **Spikes (first, technical risk).**
   - **S-A, lists and grep at scale:** timings of `git ls-files`, `git grep` (worktree and revision), and streamed reading on a repository of 100 thousand files and another of 1 million files.
   - **S-B, tree-sitter:** compile cost and binary size on Linux and macOS with 4 grammars; quality of `tags.scm` on real repositories; time to index 100 thousand files; where to store the index.
   - **S-C, path matcher:** the fuzzy variant over 1 million paths, with parallel top-K.
   The result of each spike becomes a line in `design/decisions.md` (it confirms or drops D44 and D45).
1. **N1, Quick Open.** Worktree list, path matcher, `file:line`, refresh by the watcher. *Done when:* matcher tests (order, tie-break, Unicode); the list is identical to that of `git ls-files` in a repository with ignored, renamed and sparse-checked-out files; superseded requests are dropped; the smoke scenario passes in both themes. Delivered together with N2 (2026-10-21): the click opens the file in the viewer.
2. **N2, viewer and tree.** Code page, tree, worktree reading, limits for large files and binaries, "View file" from the diff and the commit detail. *Done when:* UTF-8, non-UTF-8, binary, empty, no final newline, and very large files behave without freezing; scrolling is smooth over 100 thousand lines.
3. **N3, grep.** `git grep` with the options above, limits, cancellation, grouped results, and a click that opens N2 at the line. *Done when:* tests with real Git for unusual names, a pattern that looks like an option (`-v`, `--help`), invalid regex, binaries, limits and deterministic order; the S-A benchmark repeated in the app.
4. **N4, syntax highlighting** (done 2026-10-25, D111). Five languages (Rust, Python, Go, TypeScript with TSX, JavaScript), widened by D112 to seventeen grammars, TSX counted apart. *Done when:* the spans cover the file but only the visible lines are built and painted, without touching scroll time; a file in an unknown language stays uncolored.
5. **N5, symbols.** Index by blob, search, file outline, go to definition with candidates, usages as text. *Done when:* the same blob on two branches is indexed once; a partial index is flagged; deleting the index and reopening rebuilds the same result (determinism); the index never lives inside the working tree.
6. **N6, revisions, MCP and gate.** Revision selector on the Code page (reuses the scope pill from H6), "Browse files at this commit" in the commit menu (together with the part of 1c that waits for the menu), the Read tools of MCP, and the D49 benchmarks.

## Where it fits in the roadmap

Proposal: **Slice 3c**, after H5 to H8 and before the stacks. It reuses what Slice 3 builds (reading on its own thread, `OperationId`, virtualization, scope pill) and gives immediate value to anyone who already lives in history. The order is the author's decision. If stacking is more urgent (`design/roadmap.md`, ordering criterion 4), Slice 3c can go after Slice 5 at no cost, except that N6 depends on the commit menu and on Slice 7.

## Out of scope

- Editing files and any write to the worktree.
- Semantic navigation (references, types, rename symbol): that is LSP, not this slice (D47).
- Search in content history (`git log -S`): that is already H7.
- Blame and file history: a natural next step, but they belong to Slice 3 (`git log -- <path>` and `git blame`), not this one.
- Search in submodules and in LFS (Q8).
- Indexing the text of the whole repository into an index of its own (Zoekt style): only if `git grep` does not meet the gate, and then as a new decision.

## Risks

| Risk | Mitigation |
|---|---|
| Tree-sitter grammars weigh on compilation and on the binary, on two OSes | Spike S-B before deciding; languages behind Cargo features; fallback to regex via `git grep` |
| `git grep` slow in a huge monorepo | Required pathspec suggested in the UI, limits, benchmark; in-process search (D51); own index as last resort |
| A list of 1 million paths chokes the matcher | Contiguous buffer, parallel top-K, spike S-C |
| A symbol index creates an expectation of semantic precision | Honest interface text ("text occurrences"), candidates listed, no guessing |
| Scope grows into an IDE | The viewer is read-only by decision; editing and LSP stay in D47 |

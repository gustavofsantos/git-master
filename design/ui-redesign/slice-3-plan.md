# Slice 3: Code and Agents

## Scope

1. **Code: Files | Search.** The Code page and the text search become two tabs of the Code page. Both screens stay (`Screen::Code`, `Screen::Search`); each draws the same tab strip in its header, and a tab runs the registry action (Show Code, Search Text). The search keeps Aa, ab, .*, the pathspec box and its limits. ⇧⌘F still opens Search.
2. **Agents: Threads | Memories | Labels.** The Threads page's tab strip gains a Labels tab; the Labels page draws the same strip in place of its title. The sidebar's Agents page covers all three.
3. **Thread list grouped by day:** a day line ("Today", "Yesterday", the date) before the first thread of each day, UTC days (pure `threads::day_rows`, tested). Keyboard stepping keeps the selected row in view through the row mapping.
4. **Agent filter:** a click on a row's agent chip filters the list to that agent (the engine's `ThreadFilter::harness`); an **Agent: x** chip in the toolbar clears it, as the label filter does. Starred, Has edits and Label stay.
5. **Labels as chips:** the label chips on rows and the **Label: x** filter chip exist; **Tags…** (Thread Tags) is in the row menu.

The reader already has what the brief lists except the items below: header chips, star, Copy as Markdown, the ⋯ menu (rename, note, tags, link to commit, link to thread, export, delete), turn heads, tool groups with counts and failures, Expand all / Collapse all, edit lines with state tags, Open diff and View file.

## Decisions made here (least change)

- Labels stay a page, now a tab of Agents: removing the page would leave Rename and Delete Label with no list to pick from. The brief's "labels become filter chips and Tags…" is met for threads; label management keeps its tab.
- Day lines use UTC calendar days: the app has no time-zone library, and the self-contained rule keeps one out for a label.
- Not built (gaps): the turn outline and collapsed turn ranges in the reader, and the model name on a row (the summary carries no model). Left for the author.

## Tasks

| # | Who | Task |
|---|---|---|
| 1 | Haiku | Code tabs in `shell_code.rs` / `shell_search.rs`. |
| 2 | Opus | Day grouping, agent filter, Labels tab, reachability test of every thread action. |
| 3 | Haiku | Docs: code-navigation.md, threads.md, first-steps.md where they name the pages. |

## Acceptance

- Every thread action from the docs is in a thread menu or the palette (`every_thread_action_is_reachable_from_a_thread_menu_or_the_palette`).
- `make check` green.

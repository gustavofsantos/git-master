# Terminal companion: navigation, modes and key maps

Last updated: 2026-11-13. Decision D241 (and D236 to D240 before it). The behavior a person sees is in `docs/guide/terminal-companion.md`; this file is how it is built.

The companion is modeled the way the author works in it: next to an agent, in a tmux pane, from the keyboard first and the mouse second. Three ideas carry it:

1. **Navigation** is a stack of screens with overlays on top, and every screen is a tree of sections under one cursor.
2. **Modes**, as in vim, decide what a key means.
3. **Keys never call code.** Every action has a name in one registry; a key map, per mode, binds key sequences to names. The default map is built in; a person's own map replaces any part of it (from a configuration file, later).

Everything below is plain Rust in `gm-tui`, with no terminal or drawing in it, so it is tested without a terminal.

## Screens

A screen fills the terminal. It has an address, shown in the mode line, and keeps its own state: cursor, folds, selection, scroll.

| Screen | Address | Opens from |
|---|---|---|
| Status | `status` | the root: always at the bottom, never closed |
| Threads | `threads` | the menu, anywhere |
| Review thread | `thread 12` | a thread in Status or Threads |
| Diff | `diff unstaged`, `diff main..HEAD`, `diff a41f9c2` | the diff menu |
| File | `file src/ledger/posting.rs` | a file |
| Blame | `blame src/ledger/posting.rs` | a line |
| Origin | `origin src/ledger/posting.rs:41-53` | blame, or selected lines |
| Agent thread | `agent 9` | blame, origin, a review thread |

The screens form a **stack**. Opening one pushes it; **back** (`Esc` by default) pops to the screen below, exactly as it was left. Opening an address already on the stack goes back to that screen instead of stacking a copy, so the stack has no loops; it is capped at 32. The mode line shows the stack as a trail, `status › threads › thread 12`, and each step of the trail is clickable.

## Overlays

An overlay is drawn over the top screen and takes the input until it closes. It never changes the screen under it. Overlays stack too: a menu entry can open a text box, and sending it closes both.

| Overlay | What it is |
|---|---|
| Menu | The actions behind a prefix key, as a panel at the bottom (which-key). |
| Text box | A comment, a reply or a commit message: a split at the bottom, the screen still visible above. |
| Confirm | A one-line question before a destructive action. |
| Keys | Every key of the current screen and mode (`?`). |

## Sections

Every screen is a tree of **sections**: in Status, `Unstaged changes › src/ledger/posting.rs › hunk 2 › line 44`; in Threads, `Waiting for you › thread 12 › its messages`. A section has a kind (file, hunk, line, thread, commit, …), may be folded, and knows the target an action works on (a path, a hunk id, a line range, a comment id). One cursor moves over the visible sections; a selection spans lines.

One section tree serves every screen, so drawing, folding, moving, selecting and clicking are written once. A screen supplies its sections and the actions its section kinds offer.

## Modes

| Mode | When | Shown as |
|---|---|---|
| Normal | Moving and acting. | `NORMAL` |
| Visual | Lines are selected. | `VISUAL` |
| Insert | Typing in a text box. | `INSERT` |
| Pending | A prefix key was pressed; the menu shows what may follow. | the menu's title |

The mode decides which key map applies. Insert mode types text, except for the keys its map binds (send, cancel).

## Actions

An **action** is a named thing the companion can do: `cursor.down`, `fold.toggle`, `screen.back`, `stage`, `stage.file`, `review.comment`, `thread.resolve`, `copy.reference`, `menu.leader`, `help.keys`, … Each has:

- a **name**, stable, used by key maps and configuration;
- a **title** and a **group**, for the menu and the keys screen;
- **where it applies**: the modes, the screens and the section kinds; the menu lists only the actions that apply under the cursor or the selection;
- a **behavior**: one-shot (the menu closes after it) or repeat (the menu stays open, for staging hunk after hunk);
- the **effect**: engine commands, a screen to push, an overlay to open, a cursor move.

The same registry feeds the menu, the clickable entries, the keys screen and the docs' key table, so they cannot disagree.

## Key maps

A **key map** binds key sequences to action names, one map per mode. It is a trie:

- a leaf is an action;
- an inner node is a **prefix**: pressing it enters Pending mode and shows the menu of what may follow, filtered to what applies here.

The **leader** is an ordinary prefix bound in the Normal and Visual maps (`Space` by default). Its children are the menu's entries, grouped by their action's group. Other prefixes (`g` for `g g`) work the same way, so every multi-key sequence teaches itself.

Keys are written in one notation in the code and in configuration: `j`, `G`, `<space>`, `<tab>`, `<cr>`, `<esc>`, `<c-s>`, `<s-down>`, `<pagedown>`, sequences separated by spaces (`<space> s`, `g g`).

The **default map** binds the universal keys (arrows, page keys, `Tab`, `Enter`, `Esc`, `Shift`+arrows to select), the vim keys for muscle memory (`j`, `k`, `g g`, `G`, `<c-d>`, `<c-u>`, `V`), the leader (`Space`) and `?`. A person's map, read from a configuration file later, replaces bindings by action name and may rebind the leader to `,` or `g`. Two rules keep it safe: `Ctrl+C` always quits and `Esc` always leaves Pending mode, whatever the map says, so no map can lock a person in; and a map that names an unknown action or binds a key twice is reported, with the default kept for that binding.

## Input routing

A key goes, in order, to: the top overlay (in its mode) → the pending prefix, if any → the top screen's map for the current mode → the global map. The first that binds it acts. A click goes to the hit area under it, which names an action and its target, so a click and its key run the same action.

## Engine events and reads

Engine events reach every screen in the stack, so a screen under another stays current. Only the top screen asks for reads; a screen asks again for what it shows when it comes back to the top.

## Slices

M0 builds the navigator (stack, overlays, sections, modes, registry, key maps, routing, mode line) and moves today's content onto it; M1 to M6 add the screens. The order and status are in `design/roadmap.md`.

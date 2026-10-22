# Spike B: multi-line editable text in GPUI

Question (design/decisions.md Q7): cheapest sound way to get multi-line editable text for the
commit composer (slice 1) and the conflict-result panel. Throwaway code, not committed.
Versions: gpui 0.2.2 (spikes/Cargo.lock, same as rvw/Cargo.lock).

Evidence labels: **observed** = seen in a run here; **tested** = unit test run here;
**code** = read from source only, not exercised.

## Options

| Option | Maintenance | Licence (source) | Size | Features | Fit |
|---|---|---|---|---|---|
| 1. Port rvw text area (this spike) | Same author; one maintainer, active repo (HEAD b47f14d) | MIT per `~/Projects/rvw/Cargo.toml` (`license = "MIT"`). **No LICENSE file** in the rvw checkout. | ~2,280 non-test lines copied or adapted (editor 531, area 432, element 422, math 434, input/keys/pointer 343, shim 98) | Caret, selection, word/line moves, undo/redo with coalescing, soft wrap, scroll-to-caret, wheel, IME (UTF-16 bridge), copy/cut/paste, click count (word/line), placeholder | High for the composer. Not virtualised in shaping (see risks). |
| 2. gpui-component (longbridge) `input/textarea.rs` etc. | Active third-party crate | Apache-2.0 (`~/.cargo/registry/.../gpui-component-0.7.3/Cargo.toml`, LICENSE-APACHE present) | 5,809 lines under `src/input/` alone (not evaluated in depth) | Claims multi-line, code editor mode (file listing only) | **Blocked as-is:** it depends on `gpui = "=0.3.9"` via package `gpui-pre`, not gpui 0.2.2. Using it means moving the whole app to that gpui line first. |
| 3. Zed `crates/editor` | Large, Zed-internal; not published | **GPL-3.0-or-later** (`zed-industries/zed` `crates/editor/Cargo.toml`, `license` field); root `LICENSE-GPL` is GPL-3.0 text. Workspace sets `publish = false`. | Large (not measured) | Full editor | Poor. Copyleft conflicts with a closed premium binary unless the whole product is GPL; it also pulls in Zed's workspace. Needs legal review under Q10. `gpui` itself is Apache-2.0 (`gpui-0.2.2/LICENSE-APACHE`). |
| 4. From scratch | Ours | n/a | Estimated similar to option 1 (the editor and layout math are the bulk) | Only what we write | Viable, but option 1 already has tests for the hard parts. |

Licence checks were read from local files and GitHub raw files on this date. Not checked:
gpui-component's LICENSE text beyond its manifest and file presence; any transitive dependency licences.

## What the port required

- Dependencies cut: `rvw_view::editor::Editor` became a local `editor.rs`. It is std-only (`VecDeque`, `Range`), so no crate was added. `theme` became a 29-line `theme.rs` with the 6 tokens the area reads and one light palette.
- Not copied: `rvw-gui/src/text_input.rs` (758 lines, single-line), other rvw modules.
- `text_area.rs`: one import changed, one doc line changed, one first-paint trace added.
- `spikes/b-text/Cargo.toml` and `spikes/Cargo.lock` unchanged (lock mtime still 18:12; no new crates).
- Attribution: each ported file starts with a line naming the rvw source path, MIT, same author.
- Font: rvw picks from installed families and falls back to "Noto Sans"; the spike hardcodes "Noto Sans". Failure mode (from code): if shaping fails the element paints no lines, silently.

## Run results

- `cargo build --release -p spike-b-text`: builds; 2 dead-code warnings (`Editor::from_text/is_blank/is_empty`, `TextArea::clear`). Not fixed, not used by the spike.
- `timeout 8 cargo run --release -p spike-b-text`: **observed**. Window opened; first paint reported `logical_lines=14 visual_rows=14 content_height=280 viewport=240px`; process still alive at 8 s (exit 124, no panic).
- `cargo test --release -p spike-b-text`: **tested**, `55 passed; 0 failed` (35 editor tests: cursor, selection, undo/redo coalescing, word/line moves, UTF-8 and UTF-16; 20 math tests: wrap rows, caret limit, Up/Down targets, scroll clamp and scroll-to-caret, click kinds, selection segments, height clamp).

## Feature status

| Feature | Status | Basis |
|---|---|---|
| Multi-line text, growing 3-12 rows then scrolling | Layout and scroll path ran on first paint (content 280 px > viewport 240 px). Scroll-to-caret and clamp math tested. | observed (first paint), tested (math) |
| Soft wrap | Math tested. Not observed: the sample at 720 px wide had no wrapped line (14 logical = 14 visual rows). | tested, code. Wrap on screen not observed. |
| Caret, Left/Right/Up/Down, Home/End, Ctrl-word moves | Editor logic tested; key bindings and action wiring read from code | tested, code |
| Selection (shift-moves, select all, multi-row segments) | Logic tested; highlight painting read from code | tested, code |
| Undo/redo with typing coalescing, 200-step cap | Tested | tested |
| Mouse: click, drag, double/triple click, shift-click | Click-kind cycle tested; handlers read from code | tested (kind only), code |
| Wheel scroll | Clamp math tested; handler read from code | tested, code |
| IME (preedit underline, UTF-16 ranges, candidate bounds) | UTF-16 conversions tested; `EntityInputHandler` read from code. Not exercised on Wayland. | tested (conversion), code |
| Clipboard copy/cut/paste | Read from code (`cx.write_to_clipboard` / `read_from_clipboard`) | code |
| Caret blink, focus ring | Read from code | code |
| Placeholder | Read from code; not visible (text is non-empty) | code |
| Rendering correctness (glyphs, colours, caret position) | Not verified visually; no screenshot taken | not observed |

## Recommendation and verdict on Q7

**Option 1 for slice 1: port rvw's text area into the product, keeping its tests**, and move
the editor and math into a GitMaster crate with a LICENSE check first. Reject option 3 (GPL-3.0).
Keep option 2 as a watch item: it is Apache-2.0 but tied to gpui-pre 0.3.9, so it is only an option
after a gpui upgrade decision. Do not build from scratch unless the rvw licence is in doubt.

Risks:
1. **No LICENSE file in rvw.** Before copying into a commercial repo, the author should add a
   LICENSE file or confirm the MIT grant in writing. The manifest alone is weak evidence.
2. **Shaping is not virtualised.** `request_layout` and `prepaint` shape the whole text on every
   frame (`text_area_element.rs`); only paint is culled by bounds. Fine for commit messages
   (kilobytes). For the conflict panel (large files, Q7 "virtualizado"), this needs a measured
   check or a row-based model before adoption.
3. **Silent font failure** (see above).
4. **Dead code** in the copied `Editor`/`TextArea` API is unused by the spike; trim when adopted.
5. Several behaviours are only read from code (IME, clipboard, mouse).

What a human must test in the window (`cargo run -p spike-b-text`):
- Click to place the caret; drag to select; double and triple click; shift-click.
- Up/Down across a soft-wrapped row (needs a narrower window, e.g. 400 px) keeps the x column.
- Typing, Backspace, Enter, Ctrl/Cmd-Z and Shift-Ctrl-Z; Ctrl-A; Home/End on wrapped rows.
- Copy, cut, paste with the system clipboard on Wayland and macOS.
- IME with a CJK input method (preedit underline, candidate window position).
- Wheel scroll with a long message; caret blink and focus change.
- Resize the window: text reflows and the box height changes between 3 and 12 rows.
- On macOS, confirm the font falls back correctly without "Noto Sans".

Also needed (not part of this spike): record the Q7 outcome in design/decisions.md (the roadmap asks for it), and
the rvw LICENSE question as a decision in design/decisions.md.

## Open doubts

- Whether the spike's visible output is correct (only the trace says layout ran).
- Whether gpui-component's input is better than rvw's for the conflict panel; not evaluated beyond its manifest and file layout.
- Zed licence: stated from the manifest and root LICENSE-GPL. Whether any Zed crate other than `editor` is GPL, or what Zed's per-crate licences are, was not checked.

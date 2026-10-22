# Visual identity

Status: icon decided, palette decided, dark screen and typography open. The mockups are on the canvas "GitMaster: the whole" (link in `docs/README.md`). The icon files are in `assets/icons/`.

## Decisions

| # | Decision | Status |
|---|---|---|
| V01 | Icon: direction **Divisa** (rank insignia: stacked chevrons plus one node above). Chevron = "master" and = stack of branches; the node is the HEAD commit. | Decided |
| V02 | The app icons are **6a (dark, default)** and **6b (light)**. The other variants (Lanes, Blue, Two, Insignia) stay as exploration only. | Decided |
| V03 | Palette of the app and the icon: the author's editor theme, with no proper name. Light and dark. | Decided |
| V04 | Icon node in the signature color `#f54e00`. | Decided (applies to the files in `assets/icons/`) |
| V05 | Screens drawn in the light variant. Dark variant not drawn yet. | Open |

## Palette

Theme rules: ink first (code is almost all `fg`), eight accents with the same OKLab lightness, a ramp of five ink tones with a single warm hue, and the cursor in orange, outside the rules.

| Token | Light | Dark |
|---|---|---|
| bg | `#f7f7f4` | `#1a1918` |
| bg_hl | `#eeede7` | `#272623` |
| border | `#dbd9d2` | `#393833` |
| fg_faint | `#8a8882` | `#716f68` |
| fg_comment | `#696761` | `#9d9b92` |
| fg | `#31302d` | `#d2cfc5` |
| fg_emph | `#161614` | `#faf8f4` |
| red | `#ba3e4b` | `#ff8b90` |
| orange | `#aa5100` | `#f69557` |
| yellow | `#846600` | `#d3a82d` |
| green | `#007b2f` | `#64c175` |
| cyan | `#007677` | `#00c0c0` |
| blue | `#006cbc` | `#64b2ff` |
| violet | `#7856c0` | `#b89fff` |
| magenta | `#a9438d` | `#ed8ecf` |
| signature (cursor) | `#f54e00` | `#f54e00` |

### Use in the app

- Surfaces: `bg` in the editor and the panels, `bg_hl` in the background and the sidebar, `border` for the lines.
- States: green = ok/resolved, yellow = warning/stale, red = conflict/error, blue = selection and primary action.
- Graph lanes: base = violet, ui = cyan, api = orange, main = fg_faint. Edge of a stale stack = yellow dashed.
  - Implemented in the history graph (D87): five accents that are not state colors (blue, violet, cyan, magenta, orange), chosen by the hash of the name, and `main`, `master` and `trunk` in `fg_comment`/`faint`. Roles `graph_lanes` and `faint` in `theme.rs`.
- Background tints: the accent mixed over `bg` (9 to 13%). Text on a tint = the accent darkened 20 to 45% toward `fg_emph`.
- The window dots use the accents of the dark variant.

### Roles in `crates/gm-ui/src/theme.rs`

`ground` = `bg_hl` (window background and sidebar); `editor` and `panel` = `bg` (diff, bars, headers, floating panels); `hairline` = `border`; `muted`, `faint`, `placeholder` and `gutter` = `fg_comment` (`fg_faint` does not pass 4.5:1 as text, so it stays only for elements without text); `caret` = signature. Background tints follow the rule above (9 to 13% of the accent over `bg`); text on a tint = the accent mixed with `fg_emph` (30% in light, 20% in dark). The contrast tests in `theme.rs` (4.5:1 for text, 3:1 for marks) cover every pair.

### Point of attention

`#f54e00` is the brand color of another product. As a small accent (node, text cursor) it is acceptable. As the icon background it would evoke that product's brand, so it is not used that way.

## Divisa icon

Grid 512, safety margin 64, chevron angle 28°.

| Version | Chevrons | Stroke | Step | Node | Used at |
|---|---|---|---|---|---|
| Large | 3 | 44 | 90 | r 34 at (256,100) | 64 px or more |
| Medium | 3 | 54 | 92 | r 40 at (256,100) | 32 and 48 px |
| Small | 2 | 70 | 100 | r 50 at (256,130) | 24 px or less |

Dark (6a, default): background `#1a1918`, chevrons from top to bottom `#faf8f4`, `#64b2ff`, `#b89fff`, node `#f54e00`.
Light (6b): background `#f7f7f4`, chevrons `#161614`, `#006cbc`, `#7856c0`, node `#f54e00`.

Monochrome glyph (menu bar, tray): 2 chevrons + node, one color only, no background.

On macOS the icon body fills 824 of 1024 px, and the optical variant is chosen by the body size (small up to 32 px, medium at 64 px, large from 128 px).

## Files in the repository

```
assets/icons/
  README.md
  svg/      source: gitmaster-{dark,light}[-medium|-small|-macos].svg and gitmaster-glyph.svg
  png/      dark/ and light/, gitmaster-{16,24,32,48,64,128,256,512,1024}.png, transparent corners
  macos/    GitMaster-dark.icns, GitMaster-light.icns
  windows/  gitmaster.ico (dark)
```

The SVGs are the source. The PNG, `.icns` and `.ico` files are derived.

## Open questions

- Draw the app in the dark variant.
- Linux: the `.desktop` file and installation in `hicolor`, and how GPUI sets the window icon.
- Typography: the screens use IBM Plex Sans and JetBrains Mono as placeholders. The UI font in GPUI is still to be decided.
- Alternative icon per theme (`GITMASTER_THEME`), or only the dark one as the system default.

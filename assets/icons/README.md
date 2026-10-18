# App icons

The Divisa icon: three stacked chevrons (rank insignia, one per branch of a stack) under a commit node. Design notes and the palette are in `docs/08-identidade-visual.md`.

Two themes: `dark` is the default, `light` is the alternate. Colors come from the app palette (`docs/08`); the node is the signature orange `#f54e00`.

## Layout

| Path | Content |
|---|---|
| `svg/` | Source of truth. `gitmaster-<theme>.svg` (large), `-medium.svg`, `-small.svg`, `-macos.svg` (1024 grid, 824 px body) and `gitmaster-glyph.svg` (single color, for menu bar or tray) |
| `png/<theme>/gitmaster-<N>.png` | 16, 24, 32, 48, 64, 128, 256, 512, 1024, transparent corners |
| `macos/GitMaster-<theme>.icns` | 16 to 1024 with the macOS margin |
| `windows/gitmaster.ico` | Dark theme, 16 to 256 |

## Optical sizes

Three chevrons merge below about 32 px, so the geometry changes with the size.

| Variant | Chevrons | Stroke | Used for |
|---|---|---|---|
| large | 3 | 44 | 64 px and up |
| medium | 3 | 54 | 32 and 48 px |
| small | 2 | 70 | 24 px and below |

In `.icns`, the choice follows the size of the icon body instead: small up to 32 px, medium at 64 px, large from 128 px.

## Regenerating

Edit the SVGs, then rasterize each size from its optical variant, for example:

```sh
rsvg-convert -w 128 -h 128 svg/gitmaster-dark.svg -o png/dark/gitmaster-128.png
rsvg-convert -w 32 -h 32 svg/gitmaster-dark-medium.svg -o png/dark/gitmaster-32.png
rsvg-convert -w 16 -h 16 svg/gitmaster-dark-small.svg -o png/dark/gitmaster-16.png
```

The `.icns` and `.ico` files were built from those PNGs (the `.icns` from the `-macos` variants).

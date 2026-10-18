# GitMaster docs

Design documents (Portuguese), exported verbatim on 2026-10-16 from the author's design workspace (`gitmaster/00` to `gitmaster/07`); `08` was added later from the same place. Start with `00`, then `01` for the decision log.

| File | Content |
|---|---|
| `00-visao-e-principios.md` | Vision, benchmarks, differentiators, principles, non-goals |
| `01-decisoes-e-questoes-abertas.md` | Decision log D01-D27 (Decidido / Proposto) and open questions Q1-Q10 |
| `02-stacks-e-restack.md` | Branch stacks, metadata, restack plan and execution |
| `03-hooks.md` | Hook classes, `.gitconfig` format, JSON contract, reconciliation |
| `04-mcp-e-review-local.md` | MCP tiers and tools, local review loop |
| `05-telas.md` | Screens: sidebar, conflicts, history graph, Working Copy, diff |
| `06-arquitetura.md` | Hexagonal architecture, rules, persistence, crates, risks |
| `07-roadmap.md` | Slices 0-9 |
| `08-identidade-visual.md` | Visual identity: cursorized palette, Divisa icon decisions and geometry, files in `assets/icons/` |
| `09-navegacao-no-codigo.md` | Proposal: Quick Open, grep and symbol search over a read-only code viewer (slice 3c) |
| `rvw-lessons.md` | Lessons from `~/Projects/rvw` (English) and the slice 0 bootstrap plan (done) |

## Design canvas

The screens and the visual identity are drawn in a design canvas, "GitMaster: o todo": https://claude.ai/artifact/Qb4Ht2rnfEC8HueFR5Gtqx#play-15dd8cd198dd

The canvas is the source for mockups; these docs are the source for decisions. Its labels are in Portuguese: translate them in the UI.

The Project copy and this copy can drift. Edit them here from now on if the repository becomes the source of truth, and say so in this file.

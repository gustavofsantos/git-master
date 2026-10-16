# GitMaster

Native desktop Git client in Rust + GPUI, open source, sold as a premium product. Local-first, no headless mode. Differentiators: hooks via `.gitconfig`, first-class branch stacking with restack, MCP for agents.

## State

Design phase done, code not started. The repository holds the design documents only.

- Read `docs/README.md`, then `docs/00-visao-e-principios.md` and `docs/01-decisoes-e-questoes-abertas.md`.
- `docs/06-arquitetura.md` and `docs/07-roadmap.md` say what to build first (slice 0).
- `docs/rvw-lessons.md` has the toolchain pitfalls, GPUI practice and a bootstrap plan for slice 0 based on `~/Projects/rvw`.

## Rules

- Design docs are in Portuguese; code, comments and commit messages in English.
- A decision marked **Proposto** in `01` is not a commitment: confirm with the author before building something that depends on it, and record the outcome in `01`.
- Principles in `00` bind the code: determinism, human approves and agent proposes, Git is the source of truth, the UI never blocks, nothing is written into the repository working tree.
- Conventional commits (`feat:`, `fix:`, `test:`, `docs:`, `build:`, `refactor:`, `ci:`, `chore:`).

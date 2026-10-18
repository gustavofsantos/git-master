# GitMaster

Native desktop app in Rust + GPUI, open source, sold as a premium product. This file tells an agent what the product is, how the repository is organised, where each kind of decision lives and how to work here. It holds no roadmap, no design detail and no status: those live in `docs/`.

## What it is

GitMaster is a powerful Git client joined to the tools an engineer needs to work with code, built so that people who are accountable for their source code can use AI agents safely.

It is **not a clone of any existing client**. Mature Git clients and IDE tooling are references: where they made a choice that works (branch sidebar, per-line staging, three-pane conflict resolution), we adopt the idea and do it well; then we go beyond. Do not copy another product's name, branding, artwork or wording. Naming open source projects and libraries is fine; do not reference paid or proprietary products (competing clients, IDEs, hosted services) in code, docs, UI or commits. The product rests on three pillars:

1. **A Git client that is complete and fast.** Working Copy, history, branches, worktrees, conflicts, command palette. This is the base and has to be excellent on its own.
2. **A workspace for working with code, not only commits.** Navigating the repository where you stand: file search, text search, symbol search, a read-only viewer. Local, so it should feel instant.
3. **Agents under human accountability.** The agent proposes, the human approves, and what the agent did is observed, not self-reported. Exposed through MCP, hooks configured in `.gitconfig`, a local review loop. Branch stacking with restack is first class.

Every feature should serve at least one pillar. If it serves none, question it before building it.

## Non-negotiable qualities

- **Smooth and fast.** The UI never blocks; long lists are virtualized; work runs off the UI thread and is cancellable. A richer feature (for example structural code analysis) is welcome only if the app still feels smooth. Measure in a **release** build; when a feature cannot meet that, it degrades or is left out, it does not ship slow.
- **Self-contained.** The only external program the app may require is `git`. Never make a feature depend on something the engineer must install (ripgrep, ctags, a language server). Extra engines are Rust libraries compiled into the binary.
- **Local-first.** One engineer, their machine. No headless or server mode, no account.
- **Nothing written into the repository working tree.** Configuration and state live in the user's config, in `.git/` or in the app data directory. Many work repositories do not even allow a `.gitignore` edit.

The full principles (determinism, human approves and agent proposes, Git is the source of truth, observed beats reported) are in `docs/00-visao-e-principios.md` and bind the code.

## Where things live

| Question | Source |
|---|---|
| Why the product exists, principles, non-goals | `docs/00-visao-e-principios.md` |
| What has been decided, what is open | `docs/01-decisoes-e-questoes-abertas.md` (D = decision, Q = open question) |
| Feature design | `02` stacks, `03` hooks, `04` MCP and review, `05` screens, `09` code navigation |
| How it is built | `docs/06-arquitetura.md` |
| What to build next, and what is done | `docs/07-roadmap.md` (the only place that tracks status) |
| Visual identity | `docs/08-identidade-visual.md`, icons in `assets/icons/` |
| Toolchain pitfalls and GPUI practice | `docs/rvw-lessons.md` |
| Mockups | design canvas linked in `docs/README.md` (labels in Portuguese: translate them) |

Start with `docs/README.md`, then `00`, then the part of `01`, `06` and `07` that touches your task. The docs are the source for decisions; the canvas is the source for mockups.

### Handling decisions

- **Decidido** in `01` binds the code. Change it only by a new decision from the author.
- **Proposto** is not a commitment. Confirm with the author before building something that depends on it, then record the outcome in `01`.
- When you make a choice the docs do not cover, state it and record it in `01`; do not bury it in code.
- When a slice ends, mark it done in `07` with its known gaps. Do not copy status into this file.

## Language

Design docs are in Portuguese. Code, comments, commit messages and every UI string are in English. Translate Portuguese canvas labels instead of copying them.

## Code layout

Hexagonal: dependencies point inward to the engine. Details and rules in `docs/06-arquitetura.md`.

| Crate | Responsibility |
|---|---|
| `gm-schema` | Shared types (commands, events, operation state, status); serde and JSON Schema. No logic that needs I/O. |
| `gm-core` | The engine: commands in, events out; `GitPort` and its adapters (subprocess `git` first); hooks; watcher; history, diff, worktrees. Testable with fakes, no window, no real repository needed. |
| `gm-ui` | GPUI front end: pure models and view state kept apart from GPUI elements so logic is tested without a window. Action registry, palette, context menus, diff and history views. |
| `gm-mcp` | MCP front end over the same engine handle as the UI. |
| `gitmaster` | Thin binary, the composition root. |

Rules that matter day to day:

- The UI and MCP are two front ends of one engine. A feature is a `Command` plus `Event`s, never logic that lives only in a view.
- Reads run `git` as a subprocess and stream; a superseded request is cancelled (the process is killed) and its late answer is dropped by `OperationId`.
- Reads do not go in the command log; operations with side effects do (D28).
- Put logic in pure functions and models first; the GPUI layer stays thin.

## Working in the repository

- Toolchain: stable Rust from `rust-toolchain.toml`; `~/.cargo/bin` must come before any distro `rustc` on `PATH`. Linux needs the dev packages listed in `docs/rvw-lessons.md`. `scripts/setup-dev.sh` sets up a machine.
- `make build`, `make run`, `make test` (`cargo test --workspace`), `make lint` (clippy with `-D warnings`, then `cargo fmt --check`), `make fmt`, `make check` (lint, audit, tests). Run `make check` before declaring work done; CI runs the same.
- Clippy pedantic is on for the whole workspace. Fix the warning instead of adding an `allow`, unless the workspace table already documents it.
- Tests: pure unit and property tests for logic; **real Git in temporary repositories** for anything that touches Git behaviour (`crates/gm-core/tests/*_real_git.rs`); the UI smoke below for what the user sees. Do not mock Git where a real repository is cheap.
- Commits: conventional (`feat:`, `fix:`, `test:`, `docs:`, `build:`, `refactor:`, `ci:`, `chore:`), one logical change each.
- Match the surrounding code: naming, comment density, idiom.
- With no display available, say plainly that nothing was seen in a window.

## Driving the app headless

`scripts/ui-smoke.sh` launches the release binary on a private virtual display, drives it with `xdotool`, checks the result with `git` and compares screenshots. Each scenario runs in both themes and stops at the first failed assertion, printing the screenshot path. It needs Xvfb, xdotool, scrot, git and ImageMagick, builds the release binary if missing, uses display `:97` and creates fixtures under `~/.cache/gm-spikes/smoke-run`. Screenshots go to `$SMOKE_OUT` (default `~/.cache/gm-spikes/smoke-out`).

To run by hand:

- `Xvfb :97 -screen 0 1440x900x24 &`, then `env -u WAYLAND_DISPLAY DISPLAY=:97 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json GITMASTER_THEME=light|dark ./target/release/gitmaster <repo>`.
- `env -u WAYLAND_DISPLAY` keeps GPUI on X11 inside a Wayland session. Xvfb has no DRI3, so `lvp_icd.json` (lavapipe) renders on the CPU. `GITMASTER_THEME` picks `light` or `dark`.
- The window paints black until its first input event: after launch send `xdotool mousemove 400 300 click 1` and wait about 1 s. A black window then is this quirk, not a crash.
- The window sits at `+82+50`, 1280x800. The click coordinates are constants at the top of `scripts/ui-smoke.sh`; if the layout moves, take a screenshot, read it and update them there.

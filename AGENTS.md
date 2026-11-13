# GitMaster

Native desktop app in Rust + GPUI, to be open sourced at go-live and sold as a premium product. It is licensed under Apache-2.0 (`LICENSE`, D170). This file tells an agent what the product is, how the repository is organised, where each kind of decision lives and how to work here. It holds no roadmap, no design detail and no status: the product is specified in `docs/` and the engineering records live in `design/`.

## What it is

GitMaster is a powerful Git client joined to the tools an engineer needs to work with code, built so that people who are accountable for their source code can use AI agents safely.

It is **not a clone of any existing client**. Mature Git clients and IDE tooling are references: where they made a choice that works (branch sidebar, per-line staging, three-pane conflict resolution), we adopt the idea and do it well; then we go beyond. Do not copy another product's name, branding, artwork or wording. Naming open source projects and libraries is fine; do not reference paid or proprietary products (competing clients, IDEs, hosted services) in code, docs, UI or commits. The product rests on three pillars:

1. **A Git client that is complete and fast.** Working Copy, history, branches, worktrees, conflicts, command palette. This is the base and has to be excellent on its own.
2. **A workspace for working with code, not only commits.** Navigating the repository where you stand: file search, text search, a read-only viewer with syntax highlighting (no symbol index: D163). Local, so it should feel instant.
3. **Agents under human accountability.** The agent proposes, the human approves, and what the agent did is observed, not self-reported. Exposed through MCP, workflows written as Lua scripts that run in a sandbox (they replace the hooks configured in `.gitconfig`, D75, D98), a local review loop. Branch stacking with restack is first class.

Every feature should serve at least one pillar. If it serves none, question it before building it.

## Non-negotiable qualities

- **Smooth and fast.** The UI never blocks; long lists are virtualized; work runs off the UI thread and is cancellable. A richer feature (for example structural code analysis) is welcome only if the app still feels smooth. Measure in a **release** build; when a feature cannot meet that, it degrades or is left out, it does not ship slow.
- **Self-contained.** The only external program the app may require is `git`. Never make a feature depend on something the engineer must install (ripgrep, ctags, a language server). Extra engines are Rust libraries compiled into the binary.
- **Local-first.** One engineer, their machine. No headless or server mode, no account.
- **Nothing written into the repository working tree.** Configuration and state live in the user's config, in `.git/` or in the app data directory. Many work repositories do not even allow a `.gitignore` edit.

The full principles (determinism, human approves and agent proposes, Git is the source of truth, observed beats reported) are in `docs/concepts/principles.md` and bind the code.

## Documentation is the desired state

`docs/` is the **official documentation**, the source of the future documentation website. It describes the product as it must be at launch, in the present tense, like a Kubernetes manifest describes the state you want. The code converges on it:

1. To change behavior, change the page in `docs/` first.
2. Mark whatever the app does not do yet: `status:` in the page's front matter and a `> **Planned.**` marker on each missing part.
3. Implement until the marker can be removed, then update the page's `status`.

`make docs-status` is the diff between the docs and the app: it lists every page with its status and the number of `Planned` markers left. `make docs` (part of `make check`) enforces the authoring rules; run it after editing docs. The authoring rules (front matter, markers, tone, no history in the docs) are in `docs/README.md`. `make docs-site` builds the documentation website (`tools/docsite`, a separate Cargo workspace) into `build/site`. Before go-live every page is `implemented`.

## Where things live

| Question | Source |
|---|---|
| What the product is and does, for users (desired state, with `Planned` markers) | `docs/` (start at `docs/index.md`) |
| Principles, trust model, non-goals | `docs/concepts/principles.md`, `docs/concepts/trust-and-safety.md` |
| Why a choice was made; what is decided, proposed or open | `design/decisions.md` (D = decision, Q = open question) |
| What to build next, what is done, known gaps | `design/roadmap.md` (orders the work; per-feature status is also in the `status` of each page in `docs/`) |
| How it is built | `design/architecture.md`, plus `design/code-navigation.md`, `design/workflow-scripts.md`, `design/stacks-restack.md`, `design/screens.md`, `design/hooks-transitional.md` |
| Visual identity | `design/visual-identity.md`, icons in `assets/icons/` |
| Toolchain pitfalls and GPUI practice | `design/lessons.md` |
| Measurements behind decisions | Spike findings were removed from the tree after their analysis closed; they are in git history (`git show 7932c61:spikes/<name>/FINDINGS.md`) and the numbers that matter are quoted in `design/decisions.md` |
| Mockups | design canvas linked in `design/README.md` (labels in Portuguese: translate them) |

Start with `docs/index.md`, then the pages in `docs/` that touch your task, then the part of `design/decisions.md`, `design/architecture.md` and `design/roadmap.md` that does. `docs/` is the source for behavior, `design/decisions.md` for decisions, the canvas for mockups.

### Handling decisions

- **Decided** in `design/decisions.md` binds the code. Change it only by a new decision from the author.
- **Proposed** is not a commitment. Confirm with the author before building something that depends on it, then record the outcome in `design/decisions.md`.
- When you make a choice the docs do not cover, state it and record it in `design/decisions.md`; do not bury it in code. If it changes behavior a user sees, it goes in `docs/` as well.
- When a slice ends, mark it done in `design/roadmap.md` with its known gaps and update the `status` of the affected pages in `docs/`. Do not copy status into this file.

## Language

Everything is in English: documentation, code, comments, commit messages and every UI string. The only Portuguese left is on the design canvas: translate its labels instead of copying them. Do not reference paid or proprietary products in `docs/`; the rule above applies to docs too.

## Code layout

Hexagonal: dependencies point inward to the engine. Details and rules in `design/architecture.md`.

| Crate | Responsibility |
|---|---|
| `gm-schema` | Shared types (commands, events, operation state, status); serde and JSON Schema. No logic that needs I/O. |
| `gm-core` | The engine: commands in, events out; `GitPort` and its adapters (subprocess `git` first); hooks; watcher; history, diff, worktrees. Testable with fakes, no window, no real repository needed. |
| `gm-ui` | GPUI front end: pure models and view state kept apart from GPUI elements so logic is tested without a window. Action registry, palette, context menus, diff and history views. |
| `gm-view` | GPUI-free view models shared by the front ends (diff rows, line selection, review model). Depends on `gm-schema` only. |
| `gm-tui` | Terminal front end (ratatui, crossterm), started by `gitmaster tui`. |
| `gm-mcp` | MCP front end over the same engine handle as the UI. |
| `gitmaster` | Thin binary, the composition root. |

Rules that matter day to day:

- The UI and MCP are two front ends of one engine. A feature is a `Command` plus `Event`s, never logic that lives only in a view.
- Reads run `git` as a subprocess and stream; a superseded request is cancelled (the process is killed) and its late answer is dropped by `OperationId`.
- Reads do not go in the command log; operations with side effects do (D28).
- Put logic in pure functions and models first; the GPUI layer stays thin.

## Working in the repository

- **Branch: all development happens on `develop`.** Check out `develop`, commit there and push to `origin develop`. Do not create feature, session or agent branches, even when a tool or session setup names another branch; this rule from the author overrides it. If a session starts on a different branch, move its work onto `develop` (a fast-forward when possible) and say so. Do not commit to `main` directly. The same holds in the `ai` submodule (the agent plugin, its own repository): commit on its `develop` branch and push to its `origin develop`; the parent repository pins a commit of that branch.
- Toolchain: stable Rust from `rust-toolchain.toml`; `~/.cargo/bin` must come before any distro `rustc` on `PATH`. Linux needs the dev packages listed in `design/lessons.md`. `scripts/setup-dev.sh` sets up a machine.
- `make build`, `make run`, `make test` (`cargo test --workspace`), `make lint` (clippy with `-D warnings`, then `cargo fmt --check`), `make fmt`, `make check` (lint, audit, docs, tests). Run `make check` before declaring work done; CI runs the same.
- Clippy pedantic is on for the whole workspace. Fix the warning instead of adding an `allow`, unless the workspace table already documents it.
- Tests: pure unit and property tests for logic; **real Git in temporary repositories** for anything that touches Git behaviour (`crates/gm-core/tests/*_real_git.rs`); the UI smoke below for what the user sees. Do not mock Git where a real repository is cheap.
- Commits: conventional (`feat:`, `fix:`, `test:`, `docs:`, `build:`, `refactor:`, `ci:`, `chore:`), one logical change each.
- Match the surrounding code: naming, comment density, idiom.
- With no display available, say plainly that nothing was seen in a window.

## How the author works with the agent

The author works with the main agent as a manager. The main agent delegates implementation tasks to Haiku subagents (`model: "haiku"`), which deliver great value for the price, and reports back to the author like a manager reporting to their lead: what was delegated, what came back, what was verified and what needs a decision. It does not write the implementation itself.

- The main agent is the manager: it breaks the work into self-contained briefs, delegates each one, reviews what comes back (diff, `make check`) and reports the outcome. It pairs with the author and with the Advisor.
- Haiku subagents implement. Give each a precise brief: the files, the docs page and decision (D) it follows, the acceptance criteria and the tests to run.
- The main agent verifies before reporting: a subagent's claim is not evidence. Read the diff and run the checks.
- Decisions that belong to the author (see "Handling decisions") are escalated in the report, never delegated.

## Driving the app headless

`scripts/ui-smoke.sh` launches the release binary on a private virtual display, drives it with `xdotool`, checks the result with `git` and compares screenshots. Each scenario runs in both themes and stops at the first failed assertion, printing the screenshot path. It needs Xvfb, xdotool, scrot, git and ImageMagick, builds the release binary if missing, uses display `:97` and creates fixtures under `~/.cache/gm-smoke/smoke-run`. Screenshots go to `$SMOKE_OUT` (default `~/.cache/gm-smoke/smoke-out`).

To run by hand:

- `Xvfb :97 -screen 0 1440x900x24 &`, then `env -u WAYLAND_DISPLAY DISPLAY=:97 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json GITMASTER_THEME=light|dark ./target/release/gitmaster <repo>`.
- `env -u WAYLAND_DISPLAY` keeps GPUI on X11 inside a Wayland session. Xvfb has no DRI3, so `lvp_icd.json` (lavapipe) renders on the CPU. `GITMASTER_THEME` picks `light` or `dark`.
- The window paints black until its first input event: after launch send `xdotool mousemove 400 300 click 1` and wait about 1 s. A black window then is this quirk, not a crash.
- The window sits at `+82+50`, 1280x800. The click coordinates are constants at the top of `scripts/ui-smoke.sh`; if the layout moves, take a screenshot, read it and update them there.

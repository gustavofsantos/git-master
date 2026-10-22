# Lessons from rvw, and the slice 0 bootstrap plan

Source: `~/Projects/rvw` (Rust + GPUI review hub by the same author). Read on 2026-10-16. Treat as a historical log, not as the current state of rvw. Most useful files there:
`AGENTS.md`, `Cargo.toml`, `Makefile`, `scripts/setup-dev.sh`, `.github/workflows/ci.yml`, `.githooks/pre-push`,
`crates/rvw-gui/src/main.rs` (window, fonts, key bindings, quit), `theme.rs` (font choice from installed families),
`files_panel.rs` and `changes_screen.rs` (`uniform_list`), `text_area/` (multi-line editor), `docs/gui-m1-dogfood-notes.md`, `docs/gui-v1-design.md`.

## Build and toolchain

- `gpui = "0.2"` from crates.io (rvw's lock resolves 0.2.2). No git dependency on Zed.
- On macOS add `gpui` with feature `runtime_shaders`, so Command Line Tools suffice (no full Xcode).
- `xattr` 0.2.3 (pulled by gpui) needs `libc::ENOATTR`, which libc 0.2.190 no longer exports on Linux. rvw's `Cargo.lock` pins `libc` to **0.2.186**. Do the same: `cargo update -p libc --precise 0.2.186`.
- Linux link needs dev packages: `libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev` (plus `build-essential pkg-config`).
- A cloud container can ship a `rustc` older than the stable `rust-toolchain.toml` asks for (`error: rustc 1.97 is not supported … requires rustc 1.99`): run `rustup update stable`. A fresh container also has the runtime `libxkbcommon.so.0` but not the `-dev` packages, so `gm-ui` tests fail to link until `apt-get update && apt-get install …` has run.
- A distro `rustc` earlier on `PATH` than `~/.cargo/bin` breaks builds (E0514, clippy failing in dependencies). Put `~/.cargo/bin` first. `rust-toolchain.toml`: channel stable, components clippy and rustfmt.
- Workspace lints: `clippy::pedantic` warn (priority -1), a few documented allows (`missing_errors_doc`, `must_use_candidate`). `make lint` = `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all --check`. Fix findings; do not relax lints.
- `make check` = lint + audit (`cargo-audit` pinned, installed under `target/tools`) + tests. `.githooks/pre-push` runs it (`make hooks` sets `core.hooksPath`). CI: lint, audit, test, and an MSRV job.
- Edition 2024, `rust-version` pinned in the workspace and checked in CI for the non-GUI crates.

## GPUI practice

- Keep a pure crate (rvw-view) separate from the GPUI crate (rvw-gui): model + `update(event) -> effects`, tested without a window. Same spirit as `gm-core` here. Golden/pure tests carry the logic because nobody can look at a window in CI.
- Virtualize every long list with `uniform_list` from day one. Only visible rows are built.
- Judge performance in a **release** build. The debug build "looked slow"; the release build felt fast.
- A malformed `KeyBinding` string panics at startup. After any change to bindings, run the binary for a few seconds.
- Choose fonts at startup from `cx.text_system().all_font_names()` with a preference list and a fallback. Naming a font that is not installed gives a proportional fallback for code.
- Do not paste a mockup literally (canvas margin around a fake window made the first run look wrong).
- A status/key strip is a promise: every lit control must do something. Audit it before calling a milestone done.
- Quit: `cx.on_window_closed` quits when no windows remain; bind `secondary-q`.
- Windows opened from a desktop launcher have no shell `PATH`. rvw reads `$SHELL -l -i -c env` once in the background (10 s limit) and gives it to every git process. GitMaster needs the same, since hooks may call an agent CLI, `jq`, linters.
- Linux windows: `WindowDecorations::Client` plus own controls where the compositor draws none; set `app_id`.
- Multi-line text editing is the hard part (see Q7 in design/decisions.md). rvw has a ported single-line input and a custom text area; a Sonnet-class agent did the GPUI text area well, a cheaper one was not trusted there.

## Git practice carried over

- Shell out to `git` for plumbing and status; real-git tests in temporary repositories caught spec bugs (line staging on files without a final newline; `refs/worktree/` not protected from gc in other worktrees). Ask for the test that proves the claim.
- Undo that refuses when the repo moved fails exactly when an agent works in parallel. rvw chose checkpoints that always restore and save the current state first.
- Suggestion for status calls (not from rvw): run with `--no-optional-locks` so the app never takes `index.lock` against an agent running `git`.

## Working with agents on this codebase

- Orchestrator plans slices, workers implement; a task that names exact files, signatures and verification commands came back right the first time. Ambiguous numbers get decided by the agent: state the exact rule with an example.
- Workers never commit; the orchestrator runs `make check` and makes one conventional commit per logical change (`feat:`, `fix:`, `test:`, `docs:`, `build:`, `refactor:`, `ci:`, `chore:`).
- Parallel workers only on disjoint files; shared `lib.rs` / `Cargo.toml` are set up by the orchestrator first. Contract stubs (types + `todo!()` signatures) let two workers fill model and view in parallel.
- Build outside `/tmp` (per-user quota filled and tests failed with `Disk quota exceeded`).
- With no display, say plainly that nothing was seen in a window.

## Slice 0 bootstrap plan (skeleton built 2026-10-16; spikes A-D and the choices below still to confirm)

State: this repository has no commits besides these docs. Nothing in `crates/` exists yet.

Proposed skeleton, following `design/architecture.md` and D27:

1. Workspace: `gm-schema`, `gm-core`, `gm-ui`, `gm-mcp`, plus a thin binary crate `gitmaster` as composition root (arch rule 7 needs a home for the binary).
2. `gm-schema`: `Actor`, `Command`, `Event`, `OperationId` (sequential, so the log stays deterministic), `OperationState` with a transition table, `RepoStatus` with serde + schemars.
3. `gm-core`: `GitPort`, `HookPort`, `StorePort`, fakes, a synchronous deterministic `Engine::handle(Command) -> Vec<Event>`, one thread plus channels as the runner, an in-memory operation log, `git status --porcelain=v2 -z --branch` parser behind `GitPort`. Property tests: legal transitions only; same commands give the same log.
4. `gm-ui`: window, repo header (branch, ahead/behind), virtualized change list, `j`/`k`, `r` refresh, open repo from argv or a folder prompt.
5. `gm-mcp`: `repo_status` Read tool over the same engine handle, schema test. No transport yet (D12 socket and shim belong to slice 7).
6. Tooling copied from rvw: `rust-toolchain.toml`, workspace lints, `Makefile` (`check`, `lint`, `fmt`, `hooks`), `.githooks/pre-push`, CI, `scripts/setup-dev.sh`, libc pin.

Choices that deviate from the docs or fill gaps, to confirm:

- Engine runs on a plain thread with `async-channel`, not tokio. Tokio enters with the first async hook (slice 2). Arch rule 4 says tokio.
- `GitPort` first adapter is subprocess only (fits D26 for status); libgit2 arrives with line staging (slice 1) once Q6 is looked at.
- SQLite store is deferred; the operation log is in memory until undo or restack needs persistence.
- `notify` watcher deferred; refresh by key and on window activation.
- The git adapter lives in `gm-core` (D27 says extract `gm-git` later), so "UI never imports `gm-git`" is a convention until that extraction.
- Spikes A-D are separate from the skeleton and still to do.

## Lessons from the WF2 slice (2026-10-20)

GPUI:

- `on_blur` fires only when focus moves to another focusable element. A click on a button, a label or a tab leaves the focus where it was, so a field that commits on blur loses typed text. Commit pending fields explicitly before tab changes, Save and similar actions (`commit_pending_flow_fields`).
- A fixed-width column in a flex row (`w(px(320.)).flex_none()`) still narrowed when its content changed. Pin `min_w` and `max_w` as well.
- Iterate a `HashMap` of UI fields in a sorted order when the order reaches the file (two new `with` keys must land in a fixed order).

Headless runs (Linux cloud container):

- Besides the dev packages above, the smoke run needs `xvfb xdotool scrot imagemagick mesa-vulkan-drivers` (the last one provides lavapipe, `/usr/share/vulkan/icd.d/lvp_icd.json`). `scripts/setup-dev.sh` installs them.
- `scripts/ui-smoke.sh` builds into `CARGO_TARGET_DIR` when it is set, and runs the binary from there.
- Coordinates in the smoke script move with the content: the "Unsaved changes / Run is off" line pushes the tabs down 23 px, and "Show in Flow" sits after the workflow's name. Read a screenshot before changing a constant.

Working with agents (orchestrator plus Haiku workers, each in its own git worktree):

- Workers committed in their worktree branches; the orchestrator reviewed each diff, merged with `--no-ff`, resolved conflicts and ran clippy and the tests on the merge. This worked; parallel workers on the same view file (`shell_workflows.rs`) only conflicted in one function.
- Disk: every worktree builds its own `target/` (2 to 5 GB each with GPUI). Point workers at one shared, warm `CARGO_TARGET_DIR` and remove merged worktrees.
- The worktree guard refuses compound shell commands that name git (`cd … && git …`, pipes into git) and app launches that set `XDG_CONFIG_HOME`. Tell workers to read with the file tools, to run plain separate commands as the guard suggests, and to stop and report when a plain command is refused; never to wrap or disguise a command. Windows are checked by the orchestrator, not by workers.
- A worker that stops with no changes loses its worktree; resuming it then fails. Start a new worker instead.
- Haiku workers did well on pure modules with exact signatures and tests (layout, edits, dry run) and on GPUI views with a named pattern to copy. Every view still needed a look in a window: column width, focus and blur, and click coordinates were only found there.

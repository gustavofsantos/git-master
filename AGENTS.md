# GitMaster

Native desktop Git client in Rust + GPUI, open source, sold as a premium product. Local-first, no headless mode. Differentiators: hooks via `.gitconfig`, first-class branch stacking with restack, MCP for agents.

## State

Slice 0 skeleton exists (`crates/`: `gm-schema`, `gm-core`, `gm-ui`, `gm-mcp`, `gitmaster`). Spikes A-D not done. `make check` is the gate; `make run ARGS=<repo>` opens the window.

- Read `docs/README.md`, then `docs/00-visao-e-principios.md` and `docs/01-decisoes-e-questoes-abertas.md`.
- `docs/06-arquitetura.md` and `docs/07-roadmap.md` say what to build first (slice 0).
- `docs/rvw-lessons.md` has the toolchain pitfalls, GPUI practice and a bootstrap plan for slice 0 based on `~/Projects/rvw`.

## Rules

- Design docs are in Portuguese; code, comments, commit messages and all UI strings in English (the canvas mockups use Portuguese labels: translate them).
- A decision marked **Proposto** in `01` is not a commitment: confirm with the author before building something that depends on it, and record the outcome in `01`.
- Principles in `00` bind the code: determinism, human approves and agent proposes, Git is the source of truth, the UI never blocks, nothing is written into the repository working tree.
- Conventional commits (`feat:`, `fix:`, `test:`, `docs:`, `build:`, `refactor:`, `ci:`, `chore:`).

## Driving the app headless

`scripts/ui-smoke.sh` launches the release binary on a private virtual display, drives it with `xdotool`, and checks the result with `git`. Screenshots go to `$SMOKE_OUT` (default `~/.cache/gm-spikes/smoke-out`).

- Display recipe: `Xvfb :97 -screen 0 1440x900x24 &`, then `env -u WAYLAND_DISPLAY DISPLAY=:97 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json GITMASTER_THEME=light|dark ./target/release/gitmaster <repo>`. `env -u WAYLAND_DISPLAY` keeps GPUI on X11 even inside a Wayland session.
- Software Vulkan: Xvfb has no DRI3, so the hardware path cannot start. `lvp_icd.json` (lavapipe) renders on the CPU instead.
- `GITMASTER_THEME` selects `light` or `dark` for the run.
- Black until first input: the window paints black until it gets its first input event. After launch, send `xdotool mousemove 400 300 click 1` and wait about 1 s. A black window at that point is this quirk, not a crash.
- The window sits at `+82+50`, 1280x800. The coordinates the script clicks are constants at the top of `scripts/ui-smoke.sh`. If the layout moves, take a screenshot, read it, and update them there.
- Usage: `scripts/ui-smoke.sh`. It needs Xvfb, xdotool, scrot, git and ImageMagick (`convert`, `compare`), and builds the release binary if it is missing. It only uses display `:97` and creates fixture repos under `~/.cache/gm-spikes/smoke-run`. Each scenario runs for both themes. On the first failed assertion it prints the screenshot path and exits 1; there are no retries.

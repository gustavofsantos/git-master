# Release readiness

What the first public release needs, as assessed on 2026-11-02. Each item is **done**, **author** (a step only the author can take, outside the code), or **open** (work still to schedule). The per-feature gaps stay in `design/roadmap.md`; this file covers only what going public adds.

## Done

- **License.** Apache-2.0 for code and documentation (D170, `LICENSE`, `NOTICE`), declared in every crate. `CONTRIBUTING.md`, `SECURITY.md` and issue templates added.
- **History.** The full history (473 commits) was scanned for credentials (cloud keys, GitHub and model-provider tokens, private keys, chat tokens): only redaction test fixtures matched. The only personal data is the author's name, email and a home directory name in some paths.
- **Dependencies.** No GPL-only dependency; a few are MPL-2.0 (file-level copyleft, compatible with Apache-2.0).
- **Release pipeline.** A `v*` tag runs CI, builds Linux x86_64 and arm64 archives with the tag as the version, checks `--version`, publishes a GitHub release with checksums (a tag with `-` is a prerelease) and deploys the docs site and `install.sh` to GitHub Pages at `gitmaster.dev`. No macOS runners: macOS builds from source.
- **CI** runs on `develop` as well as `main`, and cancels superseded runs.
- **Preview features** (D169). Stacks and restack, and the conflict resolver, are hidden in the release build; `GITMASTER_ENABLE_STACKS=1` and `GITMASTER_ENABLE_CONFLICT_RESOLVER=1` turn them on, and `make` builds have them on. The docs site marks their pages Preview.
- **Docs.** Every page was checked against the code: markers for built features removed, wrong labels, menus, keys and outputs corrected, examples added (the complete Lua workflows pass `gitmaster workflow check`). `make docs` is clean.
- **Stable tests.** A stress loop (full suite, repeated, half with 24 test threads, on a loaded machine) found wall-clock and race flakes; they are fixed, and one was a product bug (a working-tree snapshot could miss a same-size rewrite in the same timestamp tick).

## Author

- **GitHub Pages.** Settings, Pages: source **GitHub Actions**, custom domain `gitmaster.dev`, then **Enforce HTTPS**. Verify the domain in the account's Pages settings.
- **DNS** for `gitmaster.dev`: apex A records `185.199.108.153`, `185.199.109.153`, `185.199.110.153`, `185.199.111.153`; optional `www` CNAME to `gustavofsantos.github.io`.
- **Branches.** `main` is behind `develop`; `install.sh`'s fallback URL and the docs links read `main`. Merge `develop` into `main` before the first tag. Delete the stale remote branches (`ccr-*`, `claude/*`) before the repository goes public.
- **Private vulnerability reporting.** Turn it on (Settings, Security) so `SECURITY.md` works.
- **The first tag**, for example `v0.1.0`, once the steps above are done.
- **Try the agent integration for real.** The roadmap records that threads capture and the MCP server were never run against a real Claude Code session (Q26). They ship visible; a session end to end before launch is the cheapest check.

## Open

- **Seen in a window.** Most recent UI work was checked by tests only (no display in the agent's environment). Run `scripts/ui-smoke.sh` on a machine with Xvfb and xdotool.
- **Terminals.** `docs/guide/terminals.md` describes a feature with no code (D148). The site marks it Planned; decide whether it stays in the docs for launch.
- **Cancellation timing tests.** About eight tests assert that a cancel lands within 0.5 to 5 s. They are the claims those tests exist for and were left as they are; widen them for debug builds if they flake in CI.
- **Search diagnosis budget.** `DIAGNOSIS_BUDGET` in the threads store skips the "which filter removed the most" diagnosis when the exact search is slow, so the answer can differ under load. Product behavior; decide whether it is acceptable.
- **The premium model** (Q10) is still open.
- **macOS.** No prebuilt binary and no CI build; a macOS build break would be found only by someone building from source.

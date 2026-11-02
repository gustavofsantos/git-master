# Contributing to GitMaster

Thank you for helping. GitMaster is a native Rust and GPUI desktop app, and the only external program it needs at run time is `git`.

## Build and test

- Run `scripts/setup-dev.sh` once on a new machine. It installs the Rust toolchain from `rust-toolchain.toml` and the Linux development packages. The packages are also listed in `design/lessons.md`.
- `make build` builds the app, and `make run` starts it on a repository.
- `make check` runs lint, audit, the documentation checks and the tests. Run it before you open a pull request. CI runs the same checks.

## Behavior is specified in `docs/`

The documentation in `docs/` describes the product as it must be at launch. To change behavior, change the page in `docs/` first, then the code. Read `docs/README.md` for the authoring rules. Decisions and their reasons live in `design/decisions.md`. Coding rules for the codebase are in [`AGENTS.md`](AGENTS.md).

## Commits and pull requests

- Use conventional commit messages: `feat:`, `fix:`, `test:`, `docs:`, `build:`, `refactor:`, `ci:` or `chore:`.
- Keep each commit to one logical change.
- Describe the problem and why the change solves it in the pull request.

## License of contributions

GitMaster is licensed under the Apache License 2.0 (see [`LICENSE`](LICENSE)). Unless you state otherwise, any contribution you submit for inclusion in the work is licensed under the same terms, as set out in section 5 of the license.

## Reporting bugs

Open an issue on GitHub and use the bug report template. Include the output of `gitmaster --version`, your operating system and desktop (X11 or Wayland), the output of `git --version`, the steps to reproduce, what you expected and what happened. If the bug involves a Git operation, attach the Git command log.

For security issues, do not open a public issue. Follow [`SECURITY.md`](SECURITY.md).

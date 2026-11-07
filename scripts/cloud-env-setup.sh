#!/usr/bin/env bash
# Setup script for the Claude Code remote (cloud) environment. Paste it into the
# environment's "Setup script" field, or call it with `bash scripts/cloud-env-setup.sh`.
# It installs everything GitMaster needs to build, test, lint and run the headless
# UI smoke, so a session never has to fix the toolchain itself. Idempotent, and it
# does not depend on the repository being cloned (the optional warm-up at the end
# only runs when it is).
#
# Needs network access to: the Ubuntu/Debian mirrors, static.rust-lang.org,
# sh.rustup.rs, index.crates.io, static.crates.io and github.com (see
# docs/rvw-lessons.md); the Claude GitHub app must also have access to the git-master-ai
# repository, which is a submodule (D194). Set GM_WARM=0 to skip the crate download and
# build warm-up.
set -euo pipefail

# The oldest compiler the workspace accepts (`rust-version` in gm-ui). Keep it in step
# with scripts/setup-dev.sh. `stable` is installed too, as rust-toolchain.toml asks.
MSRV=1.99.0
REPO="${GM_REPO:-/home/user/git-master}"
WARM="${GM_WARM:-1}"

say() { printf '\n==> %s\n' "$*"; }

sudo=""
if [ "$(id -u)" -ne 0 ]; then sudo="sudo"; fi

# A distro rustc earlier in PATH than ~/.cargo/bin breaks builds (E0514), so the
# cargo directory goes first for this script and for every later shell.
export PATH="$HOME/.cargo/bin:$PATH"

say "System packages"
if command -v apt-get >/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  $sudo apt-get update -qq
  $sudo apt-get install -y -qq --no-install-recommends \
    build-essential pkg-config curl ca-certificates git \
    libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev \
    xvfb xdotool scrot imagemagick mesa-vulkan-drivers
else
  echo "apt-get not found: install the packages of scripts/setup-dev.sh by hand" >&2
fi

say "Rust toolchain"
if ! command -v rustup >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --profile minimal --default-toolchain stable
fi
rustup toolchain install stable --profile minimal --component clippy --component rustfmt
rustup default stable
rustup toolchain install "$MSRV" --profile minimal
rustc --version

say "PATH for later shells"
# Login and interactive shells both pick it up; the line is written once.
line='export PATH="$HOME/.cargo/bin:$PATH"'
for rc in "$HOME/.profile" "$HOME/.bashrc"; do
  touch "$rc"
  grep -qxF "$line" "$rc" || printf '%s\n' "$line" >>"$rc"
done

# The plugins live in the git-master-ai submodule at ai/ (D194); the contract test reads them.
# Outside the warm-up block so GM_WARM=0 still gets them.
if [ -e "$REPO/.git" ]; then
  say "Initialising submodules in $REPO"
  git -C "$REPO" submodule update --init --recursive
fi

if [ "$WARM" = "1" ] && [ -f "$REPO/Cargo.toml" ]; then
  say "Warming the crate cache and the build in $REPO"
  cd "$REPO"
  cargo fetch --locked
  # Debug test profile, as `make test` uses: later sessions rebuild only what changed.
  cargo test --workspace --no-run --locked
else
  say "Skipping the warm-up (repository not found at $REPO, or GM_WARM=0)"
fi

say "Ready: run 'make check'"

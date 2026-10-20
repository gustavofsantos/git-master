#!/usr/bin/env bash
# Prepares a machine to build and check GitMaster: the Rust toolchain and the
# system packages GPUI needs. Idempotent.
set -euo pipefail

MSRV=1.99.0
sudo=""
if [ "$(id -u)" -ne 0 ]; then sudo="sudo"; fi

# A distro rustc earlier in PATH than ~/.cargo/bin breaks builds (E0514).
export PATH="$HOME/.cargo/bin:$PATH"

if command -v apt-get >/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  $sudo apt-get update -qq
  $sudo apt-get install -y -qq --no-install-recommends \
    build-essential pkg-config curl ca-certificates git \
    libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev \
    xvfb xdotool scrot imagemagick mesa-vulkan-drivers
fi

if ! command -v rustup >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --profile minimal --default-toolchain stable
fi
rustup toolchain install stable --profile minimal --component clippy --component rustfmt
rustup default stable
rustup toolchain install "$MSRV" --profile minimal

echo "ready: run 'make check'"

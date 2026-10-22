#!/usr/bin/env bash
# Spike S-B: what each grammar costs to compile and adds to the binary.
# Portable (Linux and macOS): bash `time`, `strip` and `wc -c` only.
#
#   ./measure.sh                 builds every case and prints a table
#   ./measure.sh | tee results/compile-$(uname -s).txt
#
# Each case is a clean release build in its own target directory, so the numbers
# include the C compiler on the grammar's parser.c and nothing from a cache. The
# crates themselves must already be downloaded (cargo fetch). Takes a while.
set -u
here=$(cd "$(dirname "$0")" && pwd)
spikes=$(cd "$here/.." && pwd)
work=${TMPDIR:-/tmp}/s-b-measure; mkdir -p "$work"
export PATH="$HOME/.cargo/bin:$PATH"
TIMEFORMAT='%R %U'

echo "host: $(uname -srm); $(rustc --version); cc: $(cc --version 2>&1 | head -1)"
echo "cpus: $( (nproc 2>/dev/null || sysctl -n hw.ncpu) )"
printf '%-12s %10s %10s %14s\n' "grammars" "wall (s)" "user (s)" "stripped (KB)"

case_() {
  name=$1; features=$2
  target="$work/target-$name"; rm -rf "$target"
  start=$( { time CARGO_TARGET_DIR="$target" cargo build --release --offline \
      --manifest-path "$spikes/Cargo.toml" -p spike-nav-b-treesitter \
      ${features:+--features "$features"} >/dev/null 2>&1 ; } 2>&1 )
  bin="$target/release/spike-nav-b-treesitter"
  cp "$bin" "$work/stripped-$name" && strip "$work/stripped-$name" 2>/dev/null
  size=$(( $(wc -c < "$work/stripped-$name") / 1024 ))
  set -- $start
  printf '%-12s %10s %10s %14s\n' "$name" "$1" "$2" "$size"
  rm -rf "$target"
}

case_ none ""
for g in rust python go typescript javascript java c cpp ruby csharp; do case_ $g "$g"; done
case_ wave1 "wave1"
case_ all "all"

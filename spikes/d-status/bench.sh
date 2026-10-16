#!/usr/bin/env bash
# Runs the spike D measurements for one synthetic size.
#   bench.sh <files>            clean + dirty states (dirty = 1000 modified, 10000 untracked)
# Needs CARGO_TARGET_DIR=$HOME/.cache/gm-spikes/target and a release build of the harness.
# Writes results/n<files>-{clean,dirty}.csv and a lock test line for the dirty state.
set -euo pipefail

files=${1:?file count}
here=$(cd "$(dirname "$0")" && pwd)
bin=$CARGO_TARGET_DIR/release/spike-d-status
repo=$HOME/.cache/gm-spikes/repos/n$files
out=$here/results
mkdir -p "$out"
RUNS=${RUNS:-7}

reset() {
  git -C "$repo" fsmonitor--daemon stop >/dev/null 2>&1 || true
  git -C "$repo" config --unset core.fsmonitor 2>/dev/null || true
  git -C "$repo" config --unset core.untrackedCache 2>/dev/null || true
  git -C "$repo" config --unset feature.manyFiles 2>/dev/null || true
  git -C "$repo" config --unset core.commitGraph 2>/dev/null || true
  git -C "$repo" update-index --no-untracked-cache
  git -C "$repo" update-index --index-version 2
}

# Each config prints one CSV line per measurement via the harness.
configure() {
  reset
  case $1 in
    base) ;;
    uc)   git -C "$repo" update-index --untracked-cache
          git -C "$repo" status >/dev/null ;;            # plain status primes the UNTR extension
    mf)   git -C "$repo" config feature.manyFiles true
          git -C "$repo" update-index --index-version 4 ;;
    cg)   git -C "$repo" commit-graph write --reachable -q 2>/dev/null || true
          git -C "$repo" config core.commitGraph true ;;
    mfuc) git -C "$repo" config feature.manyFiles true
          git -C "$repo" update-index --index-version 4
          git -C "$repo" update-index --untracked-cache
          git -C "$repo" status >/dev/null ;;
  esac
}

run_state() {
  local state=$1
  local csv=$out/n$files-$state.csv
  : > "$csv"
  # fsmonitor--daemon is refused by this git build ("not supported on this platform"), so no fsm config.
  for cfg in base uc cg mf mfuc; do
    configure "$cfg"
    echo "config=$cfg" >&2
    "$bin" proc "$repo" "$cfg" "$RUNS" >>"$csv"
    [[ $cfg == base || $cfg == uc ]] && "$bin" proc "$repo" "$cfg-plain" "$RUNS" plain >>"$csv"
  done
  # libgit2 on the plain index (no UNTR, v2): its own view, no fsmonitor or untracked cache.
  configure base
  for mode in tracked untracked_flat untracked_recurse; do
    "$bin" git2 "$repo" base "$RUNS" "$mode" >>"$csv"
  done
  configure base
  "$bin" count "$repo" >>"$csv"
  echo "wrote $csv" >&2
}

case $files in
  50000|200000|500000) ;;
  *) echo "unexpected size $files" >&2 ;;
esac

"$here/gen.sh" clean "$files" "$repo"
run_state clean
"$here/gen.sh" dirty "$files" "$repo"
run_state dirty
configure base
"$bin" lock "$repo" 30 | tee -a "$out/n$files-dirty.csv"

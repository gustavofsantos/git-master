#!/usr/bin/env bash
# Shows and reclaims the disk that Cargo build output takes, for this checkout and for every
# worktree of the repository (each worktree has its own target/ unless CARGO_TARGET_DIR is shared).
#
#   scripts/prune-target.sh              list the size of each worktree's build output
#   scripts/prune-target.sh --incremental  drop the incremental caches (the largest part of a
#                                          debug build that is cheap to rebuild)
#   scripts/prune-target.sh --debug      drop target/debug (a full debug rebuild follows)
#   scripts/prune-target.sh --all        drop every target/ and tools/docsite/target
#
# Add --worktrees to apply the action to every worktree, not only this checkout. Nothing outside
# the build output is touched, and a worktree with a cargo process running is skipped.
set -euo pipefail

action=list
all_worktrees=0
for arg in "$@"; do
  case "$arg" in
    --incremental | --debug | --all) action=${arg#--} ;;
    --worktrees) all_worktrees=1 ;;
    -h | --help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

here=$(git rev-parse --show-toplevel)
if [ "$all_worktrees" = 1 ]; then
  mapfile -t roots < <(git worktree list --porcelain | sed -n 's/^worktree //p')
else
  roots=("$here")
fi

size() { du -sh "$1" 2>/dev/null | cut -f1; }

for root in "${roots[@]}"; do
  dirs=("$root/target" "$root/tools/docsite/target")
  [ -n "${CARGO_TARGET_DIR:-}" ] && [ "$root" = "$here" ] && dirs+=("$CARGO_TARGET_DIR")
  printf '%s\n' "$root"
  for dir in "${dirs[@]}"; do
    [ -d "$dir" ] || continue
    printf '  %6s  %s\n' "$(size "$dir")" "$dir"
  done
  [ "$action" = list ] && continue
  if pgrep -f "cargo .*$root" >/dev/null 2>&1; then
    echo "  skipped: a cargo process is running there"
    continue
  fi
  case "$action" in
    incremental) rm -rf "$root"/target/*/incremental "$root"/target/*/*/incremental ;;
    debug) rm -rf "$root/target/debug" ;;
    all) rm -rf "$root/target" "$root/tools/docsite/target" ;;
  esac
  echo "  after:  $(size "$root/target" || true)"
done

#!/usr/bin/env bash
# Capture screenshots of GitMaster running on real Git repositories.
#
# This is not a test: nothing is asserted except that the window is not black.
# It exists to produce a set of PNGs (one folder per theme) that a person or a
# design agent can read to review the UI: menus, spacing, density, empty states.
#
# Two repositories are used, both scratch copies; the app never touches yours:
#   fixture  a small synthetic repository with history, branches, a merge and a
#            dirty Working Copy (staged, unstaged, untracked files).
#   real     a local clone of this repository with a few uncommitted edits, so
#            the history, the file search, the text search and the code viewer
#            show real, dense content.
# To shoot your own repository instead of `real`, set CAPTURE_REPO=/path/to/repo.
# It is cloned first (local, no network) and the edits are made in the clone.
#
# Usage:  scripts/capture-screens.sh                 every screen, both themes
#         scripts/capture-screens.sh palette history only these screens
#         CAPTURE_THEMES="dark" scripts/capture-screens.sh
#         CAPTURE_OUT=/some/dir scripts/capture-screens.sh
#         CAPTURE_SKIP_BUILD=1 scripts/capture-screens.sh   use the existing binary
#
# Output: $CAPTURE_OUT/<theme>/NN-<screen>.png and $CAPTURE_OUT/INDEX.md.
# Needs the same tools as scripts/ui-smoke.sh: Xvfb, xdotool, scrot, git, ImageMagick.
# A screen that fails is reported and skipped; the others still run.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/gitmaster"
DISPLAY_NUM=":96"
WORK="${CAPTURE_WORK:-$HOME/.cache/gm-capture}"
OUT="${CAPTURE_OUT:-$ROOT/build/screens}"
THEMES=(${CAPTURE_THEMES:-light dark})
SCREENS=(working-copy diff-unified diff-split file-menu branch-picker project-list
  sidebar-branches command-palette command-palette-args quick-open search-text settings
  command-log workflows history commit-detail commit-diff real-history real-working-copy
  real-quick-open real-search-text welcome)

# Window geometry and click targets: the same constants as scripts/ui-smoke.sh.
# The Working Copy list always starts at the same row, so these hold on both repos.
WIN_X=82; WIN_Y=50; WIN_W=1280; WIN_H=800
WAKE_X=400; WAKE_Y=300
NAME_X=420; FIRST_ROW_Y=146; SECOND_ROW_Y=170
LIST_EMPTY_X=540; LIST_EMPTY_Y=458
PROJECT_X=139; PROJECT_Y=61
BRANCH_X=140; BRANCH_Y=81
LOG_BUTTON_X=1300; LOG_BUTTON_Y=837
SIDEBAR_BRANCHES_X=120; SIDEBAR_BRANCHES_Y=425
HISTORY_X=120; HISTORY_Y=175
WORKING_COPY_X=120; WORKING_COPY_Y=147
HISTORY_ROW_X=420; HISTORY_ROW_TOP_Y=162
WORK_LAYOUT_SPLIT_X=1320; WORK_LAYOUT_Y=120

XPID=""; APP_PID=""; RUN_DIR=""; THEME=""; N=0
FAILED=()

cleanup() {
  [ -n "$APP_PID" ] && { kill "$APP_PID" 2>/dev/null; wait "$APP_PID" 2>/dev/null; }
  [ -n "$XPID" ] && { kill "$XPID" 2>/dev/null; wait "$XPID" 2>/dev/null; }
  return 0
}
trap cleanup EXIT

for tool in Xvfb xdotool scrot git convert; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done

if [ -z "${CAPTURE_SKIP_BUILD:-}" ]; then
  echo "building $BIN ..."
  (cd "$ROOT" && cargo build --release -p gitmaster --features preview) || exit 1
fi
[ -x "$BIN" ] || { echo "binary not found: $BIN" >&2; exit 1; }

export DISPLAY="$DISPLAY_NUM"
Xvfb "$DISPLAY_NUM" -screen 0 1440x900x24 >/dev/null 2>&1 &
XPID=$!
sleep 1
kill -0 "$XPID" 2>/dev/null || { echo "Xvfb did not start on $DISPLAY_NUM" >&2; exit 1; }

# --- Fixtures --------------------------------------------------------------------

git_as() { git -c user.name="$1" -c user.email="$2" -c commit.gpgsign=false "${@:3}"; }

# A repository with a believable history, then the dirty state of the smoke fixture
# (so the file rows sit where the constants above expect them).
make_fixture() {
  local repo="$1" d
  rm -rf "$repo"
  mkdir -p "$repo/src/ledger" "$repo/api" "$repo/docs"
  git -C "$repo" init -q -b main
  git -C "$repo" config user.name "Ana Souza"
  git -C "$repo" config user.email "ana@example.invalid"
  git -C "$repo" config commit.gpgsign false
  c() { # c "message" [day offset]: commit everything staged, dated N days ago
    local when
    when="$(date -d "-${2:-0} days" -R)"
    GIT_AUTHOR_DATE="$when" GIT_COMMITTER_DATE="$when" git -C "$repo" commit -q -m "$1" \
      ${3:+--author="$3"}
  }
  printf '# billing-api\n\nLedger and invoicing service.\n' > "$repo/README.md"
  printf 'pub mod total;\npub mod entry;\n' > "$repo/src/ledger/mod.rs"
  cat > "$repo/src/ledger/total.rs" <<'EOF'
impl Ledger {
    fn total(&self) -> Money {
        self.entries.iter()
            .map(|e| e.amount)
            .filter(|e| !e.reversed).sum::<Money>()
    }
}
EOF
  printf 'x\n' > "$repo/api/routes.rs"
  git -C "$repo" add -A; c "chore: scaffold the service" 30
  printf 'pub struct Entry { pub amount: Money, pub reversed: bool }\n' > "$repo/src/ledger/entry.rs"
  git -C "$repo" add -A; c "feat(ledger): add the Entry type" 24 "Bruno Lima <bruno@example.invalid>"
  printf 'fn health() -> &'"'"'static str { "ok" }\n' >> "$repo/api/routes.rs"
  git -C "$repo" add -A; c "feat(api): add a health route" 20
  git -C "$repo" checkout -q -b feature/invoices
  printf 'pub struct Invoice { pub id: u64, pub lines: Vec<Entry> }\n' > "$repo/src/ledger/invoice.rs"
  git -C "$repo" add -A; c "feat(ledger): start the Invoice type" 12 "Bruno Lima <bruno@example.invalid>"
  printf 'impl Invoice { pub fn total(&self) -> Money { todo!() } }\n' >> "$repo/src/ledger/invoice.rs"
  git -C "$repo" add -A; c "feat(ledger): invoice total" 11 "Bruno Lima <bruno@example.invalid>"
  git -C "$repo" checkout -q main
  printf '\nSee docs/ledger.md for the data model.\n' >> "$repo/README.md"
  printf '# Ledger\n\nEvery movement of money is an entry.\n' > "$repo/docs/ledger.md"
  git -C "$repo" add -A; c "docs: describe the ledger" 8
  git -C "$repo" checkout -q -b fix/rounding
  sed -i 's/sum::<Money>()/sum::<Money>().round()/' "$repo/src/ledger/total.rs"
  git -C "$repo" add -A; c "fix(ledger): round the total to cents" 5
  git -C "$repo" checkout -q main
  git -C "$repo" merge -q --no-ff fix/rounding -m "Merge branch 'fix/rounding'"
  git -C "$repo" checkout -q -b release/1.0
  git -C "$repo" checkout -q main
  printf 'pub mod audit;\n' >> "$repo/src/ledger/mod.rs"
  git -C "$repo" add -A; c "feat(ledger): declare the audit module" 2
  git -C "$repo" tag -a v0.9.0 -m "v0.9.0" HEAD~2
  # The dirty state: two edited files and one untracked.
  cat > "$repo/src/ledger/total.rs" <<'EOF'
impl Ledger {
    fn total(&self) -> Money {
        // smoke: first
        self.entries.iter()
            .map(|e| e.amount)
            .filter(|e| !e.reversed).sum::<Money>().round()
        // smoke: second
    }
}
EOF
  printf 'pub mod total;\npub mod ledger;\npub mod audit;\n' > "$repo/src/ledger/mod.rs"
  printf 'new\n' > "$repo/api/handlers.rs"
  # A second repository, so the project list has two rows.
  d="$(dirname "$repo")/payments-web"
  rm -rf "$d"; mkdir -p "$d/src"
  git -C "$d" init -q -b main
  printf 'console.log("hi")\n' > "$d/src/index.js"
  git -C "$d" add -A
  git_as "Ana Souza" ana@example.invalid -C "$d" commit -q -m "init"
  printf 'console.log("hello")\n' > "$d/src/index.js"
}

# A scratch clone of a real repository, with a few edits so its Working Copy has content.
make_real() {
  local src="$1" repo="$2"
  rm -rf "$repo"
  git clone -q --no-hardlinks "$src" "$repo" || return 1
  git -C "$repo" config user.name "Ana Souza"
  git -C "$repo" config user.email "ana@example.invalid"
  git -C "$repo" config commit.gpgsign false
  printf '\n<!-- screenshot edit -->\n' >> "$repo/README.md"
  local f
  f="$(git -C "$repo" ls-files '*.rs' | head -1)"
  [ -n "$f" ] && printf '\n// screenshot edit\n' >> "$repo/$f"
  f="$(git -C "$repo" ls-files '*.md' | sed -n 3p)"
  [ -n "$f" ] && printf '\nA new paragraph added for the screenshot.\n' >> "$repo/$f"
  printf 'scratch notes\n' > "$repo/NOTES.local.txt"
}

# --- App control ------------------------------------------------------------------

launch() { # launch THEME REPO [CWD]
  RUN_DIR="$WORK/run"
  mkdir -p "$RUN_DIR/xdg/gitmaster" "$RUN_DIR/xdg-config"
  [ -f "$RUN_DIR/gitconfig" ] || printf '[user]\n\tname = Ana Souza\n\temail = ana@example.invalid\n' > "$RUN_DIR/gitconfig"
  local args=(); [ -n "${2:-}" ] && args=("$2")
  (cd "${3:-$WORK}" && exec env -u WAYLAND_DISPLAY DISPLAY="$DISPLAY_NUM" \
    VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
    XDG_DATA_HOME="$RUN_DIR/xdg" XDG_CONFIG_HOME="$RUN_DIR/xdg-config" GIT_CONFIG_GLOBAL="$RUN_DIR/gitconfig" \
    GITMASTER_THEME="$1" "$BIN" "${args[@]}") >"$RUN_DIR/app.log" 2>&1 &
  APP_PID=$!
  sleep 4
  kill -0 "$APP_PID" 2>/dev/null || { echo "app exited at startup, see $RUN_DIR/app.log" >&2; return 1; }
  xdotool mousemove "$WAKE_X" "$WAKE_Y" click 1   # the window is black until its first input
  sleep 1
}

quit() { [ -n "$APP_PID" ] && { kill "$APP_PID" 2>/dev/null; wait "$APP_PID" 2>/dev/null; APP_PID=""; }; return 0; }

# shot NAME: crop the window out of the virtual screen into <theme>/NN-NAME.png.
shot() {
  local name="$1" out
  N=$((N + 1))
  out="$OUT/$THEME/$(printf '%02d' "$N")-$name.png"
  xdotool mousemove_relative 1 0   # lavapipe sometimes shows an unpainted frame
  sleep 0.7
  scrot -o "$WORK/full.png"
  convert "$WORK/full.png" -crop "${WIN_W}x${WIN_H}+${WIN_X}+${WIN_Y}" +repage "$out"
  local mean
  mean="$(convert "$out" -format '%[fx:mean]' info:)"
  if ! awk -v m="$mean" 'BEGIN { exit !(m > 0.03) }'; then
    echo "  black frame: $name" >&2; FAILED+=("$THEME/$name"); rm -f "$out"; return 1
  fi
  echo "  $(basename "$out")"
}

park() { xdotool mousemove "$LIST_EMPTY_X" "$LIST_EMPTY_Y"; sleep 0.5; }
click() { xdotool mousemove "$1" "$2" click "${3:-1}"; sleep "${4:-1}"; }
keys() { xdotool key "$@"; sleep 1; }
typed() { xdotool type --delay 25 -- "$1"; sleep 1; }
open_branches() { click "$SIDEBAR_BRANCHES_X" "$SIDEBAR_BRANCHES_Y"; }

# --- Screens ----------------------------------------------------------------------
# Each screen starts from the app freshly launched on the fixture, Working Copy page.

screen_working_copy() { click "$NAME_X" "$FIRST_ROW_Y"; park; shot working-copy; }
screen_diff_unified() { click "$NAME_X" "$SECOND_ROW_Y"; park; shot diff-unified; }
screen_diff_split() {
  click "$NAME_X" "$FIRST_ROW_Y"
  click "$WORK_LAYOUT_SPLIT_X" "$WORK_LAYOUT_Y" 1 1.5; park; shot diff-split
}
screen_file_menu() { click "$NAME_X" "$FIRST_ROW_Y" 3; shot file-menu; keys Escape; }
screen_branch_picker() { click "$BRANCH_X" "$BRANCH_Y" 1 1.5; shot branch-picker; typed feat; shot branch-picker-filtered; keys Escape; keys Escape; }
screen_project_list() { click "$PROJECT_X" "$PROJECT_Y"; shot project-list; keys Escape; }
screen_sidebar_branches() { open_branches; click "$NAME_X" "$FIRST_ROW_Y"; park; shot sidebar-branches; }
screen_command_palette() { keys ctrl+k; sleep 0.5; shot command-palette; typed "stage"; shot command-palette-filtered; keys Escape; }
screen_command_palette_args() { keys ctrl+k; typed "stage file"; keys Tab; sleep 0.5; shot command-palette-args; keys Escape; }
screen_quick_open() { park; keys ctrl+p; sleep 0.5; typed "ledger"; shot quick-open; keys Escape; }
screen_search_text() { park; keys ctrl+shift+f; sleep 0.5; typed "total"; sleep 1.5; shot search-text; keys Escape; }
screen_settings() { park; keys ctrl+comma; sleep 1; shot settings; keys Escape; }
screen_command_log() {
  click "$NAME_X" "$FIRST_ROW_Y"; keys space   # one side effect, so the log has a row
  click "$LOG_BUTTON_X" "$LOG_BUTTON_Y"; shot command-log
}
screen_workflows() { park; keys ctrl+k; typed "show workflows"; keys Return; sleep 1.5; shot workflows; }
screen_history() { click "$HISTORY_X" "$HISTORY_Y" 1 1.5; park; shot history; }
screen_commit_detail() {
  click "$HISTORY_X" "$HISTORY_Y" 1 1.5
  click "$HISTORY_ROW_X" "$((HISTORY_ROW_TOP_Y + 48))" 1 1.5; park; shot commit-detail
}
screen_commit_diff() {
  click "$HISTORY_X" "$HISTORY_Y" 1 1.5
  click "$HISTORY_ROW_X" "$((HISTORY_ROW_TOP_Y + 48))" 1 1.5
  click 1082 505 1 1.5; park; shot commit-diff
}
screen_real_history() { click "$HISTORY_X" "$HISTORY_Y" 1 1.5; park; shot real-history; }
screen_real_working_copy() { click "$NAME_X" "$FIRST_ROW_Y"; park; shot real-working-copy; }
screen_real_quick_open() { park; keys ctrl+p; sleep 0.5; typed "architecture"; shot real-quick-open; keys Return; sleep 1.5; shot real-file-viewer; }
screen_real_search_text() { park; keys ctrl+shift+f; sleep 0.5; typed "Command"; sleep 2; shot real-search-text; }

# screen NAME: the repository and launch mode it needs.
repo_for() {
  case "$1" in
    real-*) echo "$WORK/real" ;;
    *) echo "$WORK/fixture" ;;
  esac
}

run_screen() {
  local name="$1" fn
  fn="screen_${name//-/_}"
  if [ "$name" = welcome ]; then
    mkdir -p "$WORK/plain"; rm -rf "$RUN_DIR/xdg/gitmaster/recent.json"
    launch "$THEME" "" "$WORK/plain" || { FAILED+=("$THEME/$name"); return; }
    shot welcome; quit; return
  fi
  declare -F "$fn" >/dev/null || { echo "  unknown screen: $name" >&2; FAILED+=("$THEME/$name"); return; }
  local repo; repo="$(repo_for "$name")"
  mkdir -p "$WORK/run/xdg/gitmaster"
  printf '[\n  "%s/payments-web",\n  "%s"\n]\n' "$WORK" "$repo" > "$WORK/run/xdg/gitmaster/recent.json"
  launch "$THEME" "$repo" || { FAILED+=("$THEME/$name"); return; }
  "$fn" || FAILED+=("$THEME/$name")
  quit
}

# --- Main -------------------------------------------------------------------------
mkdir -p "$WORK" "$OUT"
[ "$#" -gt 0 ] && SCREENS=("$@")

for theme in "${THEMES[@]}"; do
  THEME="$theme"; N=0
  rm -rf "$OUT/$theme"; mkdir -p "$OUT/$theme"
  echo "== theme: $theme"
  for s in "${SCREENS[@]}"; do
    # The fixtures are rebuilt for every screen: a staged file or a checkout from the
    # previous screen must not leak into the next one.
    make_fixture "$WORK/fixture"
    case "$s" in real-*) make_real "${CAPTURE_REPO:-$ROOT}" "$WORK/real" || { FAILED+=("$theme/$s"); continue; } ;; esac
    echo "-- $s"
    run_screen "$s"
  done
done

# --- Index ------------------------------------------------------------------------
{
  echo "# GitMaster screenshots"
  echo
  echo "Generated by scripts/capture-screens.sh on $(date -u +%F). Window ${WIN_W}x${WIN_H}."
  echo "\`fixture\` shots are a synthetic repository; \`real-\` shots are a clone of a real one."
  for theme in "${THEMES[@]}"; do
    echo; echo "## $theme"; echo
    for f in "$OUT/$theme"/*.png; do [ -e "$f" ] && echo "- $theme/$(basename "$f")"; done
  done
} > "$OUT/INDEX.md"

echo
echo "screenshots: $OUT"
if [ "${#FAILED[@]}" -gt 0 ]; then
  echo "failed: ${FAILED[*]}" >&2
  exit 1
fi

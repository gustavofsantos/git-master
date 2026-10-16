#!/usr/bin/env bash
# End-to-end smoke test for the GitMaster window.
#
# Launches the real release binary on a private Xvfb display (:97), drives it
# with xdotool, and asserts the outcome with git. Screenshots go to $SMOKE_OUT.
# Each scenario runs once per theme (light, dark) on a freshly created repo.
#
# Usage:  scripts/ui-smoke.sh
#         SMOKE_OUT=/some/dir scripts/ui-smoke.sh
#
# Exits 0 when every scenario passes. On the first failed assertion it prints
# the screenshot path and exits 1; there are no retries by design.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/gitmaster"
DISPLAY_NUM=":97"
RUN_ROOT="$HOME/.cache/gm-spikes/smoke-run"
SMOKE_OUT="${SMOKE_OUT:-$HOME/.cache/gm-spikes/smoke-out}"
THEMES=(light dark)
SCENARIOS=(a-stage-lines b-stage-file c-commit d-refresh e-command-log)

# --- Layout constants (screen coordinates; the window sits at +82+50) ---------
# If the layout shifts, re-take a screenshot and update these. Check the rows
# with the Read tool on a screenshot before changing anything else.
WIN_X=82; WIN_Y=50; WIN_W=1280; WIN_H=800   # window geometry, used to crop shots
WAKE_X=400; WAKE_Y=300                      # first click: black until first input
FILE_CHECK_X=112                            # file row checkbox (dirs are at 122)
NAME_X=218                                  # file name column
ROW_HANDLERS_Y=407                          # api/handlers.rs (untracked)
ROW_MOD_Y=455                               # src/ledger/mod.rs
ROW_TOTAL_Y=479                             # src/ledger/total.rs
LIST_EMPTY_X=300; LIST_EMPTY_Y=650          # empty list area, takes list focus
DIFF_CHECK_X=601                            # gutter checkbox in the diff pane
DIFF_LINE_A_Y=224                           # first pure-insertion row in total.rs
DIFF_LINE_B_Y=312                           # second pure-insertion row in total.rs
COMPOSER_X=300; COMPOSER_Y=255              # commit message box
LOG_BUTTON_X=1300; LOG_BUTTON_Y=837         # "Git log" button, right end of the status bar

COMMIT_MSG="smoke: stage mod.rs and commit it"

# --- Process state, cleaned up by trap ----------------------------------------
XPID=""
APP_PID=""
RUN_DIR=""
LABEL=""

cleanup() {
  if [ -n "$APP_PID" ]; then
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
    APP_PID=""
  fi
  if [ -n "$XPID" ]; then
    kill "$XPID" 2>/dev/null || true
    wait "$XPID" 2>/dev/null || true
    XPID=""
  fi
}
trap cleanup EXIT

fail() {
  # $1: message. Saves a screenshot of the failing state, then exits non-zero.
  local shot="$SMOKE_OUT/${LABEL}-FAILED.png"
  if [ -n "$RUN_DIR" ]; then
    scrot -o "$shot" 2>/dev/null || true
  fi
  echo "FAIL [$LABEL]: $1" >&2
  echo "screenshot: $shot" >&2
  exit 1
}

# --- 1. Tools and binary ------------------------------------------------------
for tool in Xvfb xdotool scrot git convert compare; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done

if [ ! -x "$BIN" ]; then
  echo "building $BIN ..."
  (cd "$ROOT" && cargo build --release -p gitmaster)
fi
[ -x "$BIN" ] || { echo "binary not found after build: $BIN" >&2; exit 1; }

mkdir -p "$SMOKE_OUT" "$RUN_ROOT"

# --- Helpers -----------------------------------------------------------------
export DISPLAY="$DISPLAY_NUM"

start_xvfb() {
  Xvfb "$DISPLAY_NUM" -screen 0 1440x900x24 >/dev/null 2>&1 &
  XPID=$!
  sleep 1
  kill -0 "$XPID" 2>/dev/null || fail "Xvfb did not start on $DISPLAY_NUM (is it already in use?)"
}

# Fresh repo with the fixture: total.rs and mod.rs committed, then total.rs has
# two pure insertions, mod.rs gets one line, and api/handlers.rs is untracked.
make_repo() {
  local repo="$1"
  rm -rf "$repo"
  mkdir -p "$repo/src/ledger" "$repo/api"
  git -C "$repo" init -q -b main
  git -C "$repo" config user.name "gm smoke"
  git -C "$repo" config user.email "smoke@example.invalid"
  git -C "$repo" config commit.gpgsign false

  cat > "$repo/src/ledger/total.rs" <<'EOF'
impl Ledger {
    fn total(&self) -> Money {
        self.entries.iter()
            .map(|e| e.amount)
            .filter(|e| !e.reversed).sum::<Money>()
    }
}
EOF
  printf 'pub mod total;\npub mod entry;\n' > "$repo/src/ledger/mod.rs"
  printf 'x\n' > "$repo/api/routes.rs"
  git -C "$repo" add -A
  git -C "$repo" commit -q -m "init"

  cat > "$repo/src/ledger/total.rs" <<'EOF'
impl Ledger {
    fn total(&self) -> Money {
        // smoke: first
        self.entries.iter()
            .map(|e| e.amount)
            .filter(|e| !e.reversed).sum::<Money>()
        // smoke: second
    }
}
EOF
  printf 'pub mod total;\npub mod entry;\npub mod audit;\n' > "$repo/src/ledger/mod.rs"
  printf 'new\n' > "$repo/api/handlers.rs"
}

launch_app() {
  # $1: theme, $2: repo
  env -u WAYLAND_DISPLAY DISPLAY="$DISPLAY_NUM" \
    VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
    GITMASTER_THEME="$1" "$BIN" "$2" >"$RUN_DIR/app.log" 2>&1 &
  APP_PID=$!
  sleep 4
  kill -0 "$APP_PID" 2>/dev/null || fail "app exited at startup (see $RUN_DIR/app.log)"
  # The window stays black until its first input event.
  xdotool mousemove "$WAKE_X" "$WAKE_Y" click 1
  sleep 1
}

stop_app() {
  kill "$APP_PID" 2>/dev/null || true
  wait "$APP_PID" 2>/dev/null || true
  APP_PID=""
}

# Full screen, cropped to the window, saved as <scenario>-<theme>.png.
# Fails if the window is (almost) all black.
shoot() {
  # $1: output file
  # lavapipe sometimes shows an unpainted frame: a 1 px mouse nudge forces a redraw.
  xdotool mousemove_relative 1 0
  sleep 0.7
  scrot -o "$RUN_DIR/full.png"
  convert "$RUN_DIR/full.png" -crop "${WIN_W}x${WIN_H}+${WIN_X}+${WIN_Y}" +repage "$1"
  local mean
  mean="$(convert "$1" -format '%[fx:mean]' info:)"
  awk -v m="$mean" 'BEGIN { exit !(m > 0.03) }' || fail "screenshot is black (mean=$mean): $1"
}

staged_names() { git -C "$RUN_DIR/repo" diff --cached --name-only; }

# --- 3a. Stage two changed lines with the diff pane checkboxes -----------------
scenario_a() {
  local repo="$RUN_DIR/repo" theme="$1" cached added expected
  xdotool mousemove "$NAME_X" "$ROW_TOTAL_Y" click 1; sleep 1
  xdotool mousemove "$DIFF_CHECK_X" "$DIFF_LINE_A_Y" click 1; sleep 0.7
  xdotool mousemove "$DIFF_CHECK_X" "$DIFF_LINE_B_Y" click 1; sleep 0.7
  xdotool mousemove "$LIST_EMPTY_X" "$LIST_EMPTY_Y"; sleep 0.5
  xdotool key s; sleep 1.5

  cached="$(git -C "$repo" diff --cached -U0 --no-color -- src/ledger/total.rs)"
  added="$(sed -n '/^@@/,$p' <<<"$cached" | grep '^[-+]' || true)"
  expected="$(printf '+        // smoke: first\n+        // smoke: second')"
  [ "$added" = "$expected" ] || fail "git diff --cached changed lines differ. got:
$added
expected:
$expected"
  git -C "$repo" diff --quiet -- src/ledger/total.rs \
    || fail "total.rs still has unstaged changes after staging both lines"

  shoot "$SMOKE_OUT/a-stage-lines-$theme.png"
}

# --- 3b. Stage and unstage a whole file from its row checkbox ------------------
scenario_b() {
  local repo="$RUN_DIR/repo" theme="$1" staged
  xdotool mousemove "$FILE_CHECK_X" "$ROW_MOD_Y" click 1; sleep 1.5
  staged="$(staged_names)"
  grep -qxF -- 'src/ledger/mod.rs' <<<"$staged" || fail "mod.rs not staged after row checkbox click"

  xdotool mousemove "$FILE_CHECK_X" "$ROW_MOD_Y" click 1; sleep 1.5
  staged="$(staged_names)"
  grep -qxF -- 'src/ledger/mod.rs' <<<"$staged" && fail "mod.rs still staged after second row checkbox click"

  shoot "$SMOKE_OUT/b-stage-file-$theme.png"
}

# --- 3c. Stage a file, type a message, commit with ctrl+Return -----------------
scenario_c() {
  local repo="$RUN_DIR/repo" theme="$1" staged status got
  xdotool mousemove "$FILE_CHECK_X" "$ROW_MOD_Y" click 1; sleep 1.5
  staged="$(staged_names)"
  grep -qxF -- 'src/ledger/mod.rs' <<<"$staged" || fail "mod.rs not staged before commit"

  xdotool mousemove "$COMPOSER_X" "$COMPOSER_Y" click 1; sleep 0.7
  xdotool type --delay 20 -- "$COMMIT_MSG"; sleep 0.7
  xdotool key ctrl+Return; sleep 1.5

  got="$(git -C "$repo" log -1 --format=%B)"
  [ "$got" = "$COMMIT_MSG" ] || fail "last commit message is '$got', expected '$COMMIT_MSG'"

  status="$(git -C "$repo" status --porcelain)"
  grep -qF 'src/ledger/mod.rs' <<<"$status" && fail "committed mod.rs still listed in git status:
$status"
  grep -qF 'src/ledger/total.rs' <<<"$status" \
    || fail "total.rs vanished from git status, but it was never committed:
$status"

  shoot "$SMOKE_OUT/c-commit-$theme.png"
}

# --- 3d. Refresh after an external git-visible change --------------------------
scenario_d() {
  local repo="$RUN_DIR/repo" theme="$1" before after differing
  xdotool mousemove "$LIST_EMPTY_X" "$LIST_EMPTY_Y" click 1; sleep 1
  scrot -o "$RUN_DIR/before-full.png"
  convert "$RUN_DIR/before-full.png" -crop "${WIN_W}x${WIN_H}+${WIN_X}+${WIN_Y}" +repage "$RUN_DIR/before.png"

  # Change the repo outside the app. The list must not change until refresh.
  printf '// external edit\n' >> "$repo/api/routes.rs"
  sleep 1
  xdotool key r; sleep 1.5

  shoot "$SMOKE_OUT/d-refresh-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/before.png" "$SMOKE_OUT/d-refresh-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare before/after screenshots: $differing"
  [ "$differing" -gt 0 ] || fail "screenshot did not change after refresh"
}

# --- 3e. Open the command log panel after a stage ------------------------------
scenario_e() {
  local theme="$1" differing
  xdotool mousemove "$FILE_CHECK_X" "$ROW_MOD_Y" click 1; sleep 1.5
  grep -qxF -- 'src/ledger/mod.rs' <<<"$(staged_names)" || fail "mod.rs not staged"
  shoot "$RUN_DIR/closed.png"

  xdotool mousemove "$LOG_BUTTON_X" "$LOG_BUTTON_Y" click 1; sleep 1
  shoot "$SMOKE_OUT/e-command-log-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/closed.png" "$SMOKE_OUT/e-command-log-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  # The panel covers a large area; a sliver of change would be only the button tint.
  [ "$differing" -gt 20000 ] || fail "the log panel did not open (only $differing pixels changed)"
}

run_scenario() {
  # $1: scenario name, $2: theme
  local name="$1" theme="$2"
  LABEL="$name-$theme"
  RUN_DIR="$RUN_ROOT/$LABEL"
  rm -rf "$RUN_DIR"
  mkdir -p "$RUN_DIR"
  make_repo "$RUN_DIR/repo"
  launch_app "$theme" "$RUN_DIR/repo"
  case "$name" in
    a-stage-lines) scenario_a "$theme" ;;
    b-stage-file)  scenario_b "$theme" ;;
    c-commit)      scenario_c "$theme" ;;
    d-refresh)     scenario_d "$theme" ;;
    e-command-log) scenario_e "$theme" ;;
    *) fail "unknown scenario $name" ;;
  esac
  stop_app
  echo "PASS  $LABEL"
}

# --- 2/3. Run everything -------------------------------------------------------
start_xvfb
for theme in "${THEMES[@]}"; do
  for scenario in "${SCENARIOS[@]}"; do
    run_scenario "$scenario" "$theme"
  done
done
echo "all scenarios passed; screenshots in $SMOKE_OUT"

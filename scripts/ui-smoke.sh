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
SCENARIOS=(a-stage-lines b-stage-file c-commit d-refresh e-command-log f-open-project g-switch-branch h-welcome i-stage-chunk j-discard-chunk k-file-menu l-branch-menu m-command-palette n-typed-text-kept o-suggestion-replace p-hook-failure q-repo-hook-approval r-title-bar)

# --- Layout constants (screen coordinates; the window sits at +82+50) ---------
# If the layout shifts, re-take a screenshot and update these. Check the rows
# with the Read tool on a screenshot before changing anything else.
WIN_X=82; WIN_Y=50; WIN_W=1280; WIN_H=800   # window geometry, used to crop shots
WAKE_X=400; WAKE_Y=300                      # first click: black until first input
NAME_X=420                                  # file path column: a click selects, space stages
ROW_HANDLERS_Y=194                          # api/handlers.rs (untracked)
ROW_MOD_Y=146                               # src/ledger/mod.rs
ROW_TOTAL_Y=170                             # src/ledger/total.rs
LIST_EMPTY_X=540; LIST_EMPTY_Y=458          # empty list area, takes list focus
DIFF_CHECK_X=841                            # gutter checkbox in the diff pane
DIFF_LINE_A_Y=239                           # first pure-insertion row in total.rs
DIFF_LINE_B_Y=327                           # second pure-insertion row in total.rs
CHUNK_Y=173                                 # hunk header row of total.rs (its buttons)
CHUNK_DISCARD_X=1181                        # "Discard Chunk" in that row
CHUNK_STAGE_X=1298                          # "Stage Chunk" in that row (Unstage on Staged)
CONFIRM_DISCARD_X=1243; CONFIRM_CANCEL_X=1317   # the inline confirmation in that row
COMPOSER_X=540; COMPOSER_Y=690              # commit message box
COMMIT_BUTTON_X=530; COMMIT_BUTTON_Y=798    # "Commit on <branch>" under the message box
LOG_BUTTON_X=1300; LOG_BUTTON_Y=837         # "Git log" button, right end of the status bar
PROJECT_X=139; PROJECT_Y=61                 # project button at the left of the top bar
RECENT_OTHER_X=150; RECENT_OTHER_Y=185      # second row of the open list: billing-api
BRANCH_X=140; BRANCH_Y=81                   # branch button, right of the project button
SIDEBAR_FEATURE_X=160; SIDEBAR_FEATURE_Y=423  # feature/x row in the sidebar (non-current)
MENU_STAGE_X=470; MENU_STAGE_Y=163          # "Stage File" in the file row's menu, opened at NAME_X, ROW_MOD_Y
MENU_CHECKOUT_X=190; MENU_CHECKOUT_Y=439    # "Checkout Branch" in the branch menu, opened at SIDEBAR_FEATURE_X/Y
CHIP_X=316; CHIP_Y=590; CHIP_W=142; CHIP_H=28   # the hook chip's text in the message header, window-relative
TITLE_BAR_H=44                              # the title bar: name row over the branch and path row
TOOLBAR_Y=72; TOOLBAR_FETCH_X=1082; TOOLBAR_RESTACK_X=1322   # Fetch and Restack buttons, right of the title bar

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
  # The per-run fixture repos are scratch. Screenshots live in $SMOKE_OUT and
  # stay; the rest of ~/.cache/gm-spikes is not touched.
  case "$SMOKE_OUT/" in
    "$RUN_ROOT"/*) ;;
    *) rm -rf "$RUN_ROOT" ;;
  esac
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
  # $1: theme, $2: repo argument (empty for none), $3: working directory
  # (default: the run's directory). The recent list is private to this run.
  # The subshell execs the app, so APP_PID is the app itself.
  local args=()
  if [ -n "$2" ]; then args=("$2"); fi
  (cd "${3:-$RUN_DIR}" && exec env -u WAYLAND_DISPLAY DISPLAY="$DISPLAY_NUM" \
    VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
    XDG_DATA_HOME="$RUN_DIR/xdg" GIT_CONFIG_GLOBAL="$RUN_DIR/gitconfig" \
    GITMASTER_THEME="$1" "$BIN" "${args[@]}") >"$RUN_DIR/app.log" 2>&1 &
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

# Selects mod.rs from its row, then space: stages it, or unstages it when it has
# nothing unstaged left.
space_on_mod_file() {
  xdotool mousemove "$NAME_X" "$ROW_MOD_Y" click 1; sleep 0.7
  xdotool key space
}

# --- 3b. Stage and unstage a whole file with space on its row ------------------
scenario_b() {
  local repo="$RUN_DIR/repo" theme="$1" staged
  space_on_mod_file; sleep 1
  staged="$(staged_names)"
  grep -qxF -- 'src/ledger/mod.rs' <<<"$staged" || fail "mod.rs not staged after space on its row"

  space_on_mod_file; sleep 1
  staged="$(staged_names)"
  grep -qxF -- 'src/ledger/mod.rs' <<<"$staged" && fail "mod.rs still staged after second space"

  shoot "$SMOKE_OUT/b-stage-file-$theme.png"
}

# --- 3c. Stage a file, type a message, commit with ctrl+Return -----------------
scenario_c() {
  local repo="$RUN_DIR/repo" theme="$1" staged status got
  space_on_mod_file; sleep 1
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

# --- 3d. The window follows an external git-visible change by itself ---------------
scenario_d() {
  local repo="$RUN_DIR/repo" theme="$1" before after differing
  xdotool mousemove "$LIST_EMPTY_X" "$LIST_EMPTY_Y" click 1; sleep 1
  scrot -o "$RUN_DIR/before-full.png"
  convert "$RUN_DIR/before-full.png" -crop "${WIN_W}x${WIN_H}+${WIN_X}+${WIN_Y}" +repage "$RUN_DIR/before.png"

  # Change the repo outside the app and touch nothing: the watcher refreshes it.
  printf '// external edit\n' >> "$repo/api/routes.rs"
  sleep 2.5

  shoot "$SMOKE_OUT/d-refresh-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/before.png" "$SMOKE_OUT/d-refresh-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare before/after screenshots: $differing"
  [ "$differing" -gt 0 ] || fail "screenshot did not change after an external edit"
}

# --- 3e. Open the command log panel after a stage ------------------------------
scenario_e() {
  local theme="$1" differing
  space_on_mod_file; sleep 1
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

# A second repository with its own name and content, for the project switch.
make_other_repo() {
  local repo="$1"
  rm -rf "$repo"
  mkdir -p "$repo/src"
  git -C "$repo" init -q -b main
  git -C "$repo" config user.name "gm smoke"
  git -C "$repo" config user.email "smoke@example.invalid"
  git -C "$repo" config commit.gpgsign false
  printf 'fn main() {}\n' > "$repo/src/main.rs"
  git -C "$repo" add -A
  git -C "$repo" commit -q -m "init"
  printf 'fn main() { billing() }\n' > "$repo/src/main.rs"
  printf 'untracked\n' > "$repo/notes.txt"
}

# What the repository looks like from outside git: its status and every path
# in the working tree. Equal before and after means nothing was written there.
repo_state() {
  git -C "$1" status --porcelain --ignored --untracked-files=all
  (cd "$1" && find . -path ./.git -prune -o -print | sort)
}

# --- 3f. Switch the repository from the top bar's project list -----------------
scenario_f() {
  local repo="$RUN_DIR/repo" other="$RUN_DIR/billing-api" theme="$1"
  local recents="$RUN_DIR/xdg/gitmaster/recent.json"
  local before_repo before_other after_repo after_other differing top_line
  before_repo="$(repo_state "$repo")"
  before_other="$(repo_state "$other")"
  # The pointer rests off the button, so its hover tint is not in the shots.
  xdotool mousemove "$LIST_EMPTY_X" "$LIST_EMPTY_Y"; sleep 0.5
  shoot "$RUN_DIR/closed.png"

  xdotool mousemove "$PROJECT_X" "$PROJECT_Y" click 1; sleep 1
  shoot "$SMOKE_OUT/f-project-list-$theme.png"

  # Escape closes the list and leaves the repository as it was.
  xdotool key Escape; sleep 1
  xdotool mousemove "$LIST_EMPTY_X" "$LIST_EMPTY_Y"; sleep 0.5
  shoot "$RUN_DIR/escaped.png"
  differing="$(compare -metric AE "$RUN_DIR/closed.png" "$RUN_DIR/escaped.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -lt 2000 ] || fail "Escape did not close the project list ($differing pixels changed)"

  # Reopen, then pick the other repository.
  xdotool mousemove "$PROJECT_X" "$PROJECT_Y" click 1; sleep 1
  xdotool mousemove "$RECENT_OTHER_X" "$RECENT_OTHER_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/f-project-switched-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/f-project-list-$theme.png" "$SMOKE_OUT/f-project-switched-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the view did not change after picking the other repository ($differing pixels)"

  # The recent list written in the background: the picked repository is first.
  sleep 1
  top_line="$(sed -n 2p "$recents")"
  grep -qF "$other" <<<"$top_line" || fail "recent.json does not list billing-api first: $(cat "$recents")"
  [ "$(grep -c '^  "' "$recents")" -eq 2 ] || fail "recent.json should hold the two repositories once each: $(cat "$recents")"

  # Neither repository was written to.
  after_repo="$(repo_state "$repo")"
  after_other="$(repo_state "$other")"
  [ "$before_repo" = "$after_repo" ] || fail "the first repository changed while the app ran:
$(diff <(printf '%s\n' "$before_repo") <(printf '%s\n' "$after_repo") || true)"
  [ "$before_other" = "$after_other" ] || fail "the second repository changed while the app ran:
$(diff <(printf '%s\n' "$before_other") <(printf '%s\n' "$after_other") || true)"
}

# Branches for g-switch-branch, made before the app opens the repository. Only
# this scenario gets them, so the fixture of the others is unchanged.
prepare_branches() {
  local repo="$1" tmp="$RUN_DIR/conflict-clone"
  # Same tree as main: the checkout goes through with the work in progress.
  git -C "$repo" branch feature/x
  # A branch that changes api/routes.rs. Below, that file is dirty, so git
  # refuses the checkout and leaves the work alone.
  rm -rf "$tmp"
  git clone -q "$repo" "$tmp"
  printf 'changed on conflict\n' > "$tmp/api/routes.rs"
  git -C "$tmp" add api/routes.rs
  git -C "$tmp" -c user.name="gm smoke" -c user.email="smoke@example.invalid" \
    -c commit.gpgsign=false commit -q -m "conflict: routes"
  git -C "$tmp" push -q "$repo" HEAD:refs/heads/conflict
  rm -rf "$tmp"
  printf '// dirty\n' >> "$repo/api/routes.rs"
}

# --- 3g. Switch branches from the top bar's branch list ------------------------
scenario_g() {
  local repo="$RUN_DIR/repo" theme="$1" head differing
  xdotool mousemove "$LIST_EMPTY_X" "$LIST_EMPTY_Y"; sleep 0.5
  shoot "$RUN_DIR/closed.png"

  # Opening lists the branches; main is current, so the list opens on it.
  xdotool mousemove "$BRANCH_X" "$BRANCH_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/g-switch-branch-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/closed.png" "$SMOKE_OUT/g-switch-branch-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the branch list did not open ($differing pixels changed)"

  # Typing filters the list: only feature/x contains "feat".
  xdotool type --delay 20 -- feat; sleep 0.7
  shoot "$SMOKE_OUT/g-switch-branch-filter-$theme.png"

  # Escape clears the filter and keeps the list open.
  xdotool key Escape; sleep 0.7
  shoot "$RUN_DIR/g-cleared.png"
  differing="$(compare -metric AE "$RUN_DIR/closed.png" "$RUN_DIR/g-cleared.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "Escape closed the list instead of clearing the filter"

  # The list is conflict, feature/x, main, and Escape leaves the highlight on
  # the first row. Down, Up, Down ends on feature/x; Enter checks it out.
  xdotool key Down; sleep 0.4
  xdotool key Up; sleep 0.4
  xdotool key Down; sleep 0.4
  xdotool key Return; sleep 1.5
  head="$(git -C "$repo" rev-parse --abbrev-ref HEAD)"
  [ "$head" = "feature/x" ] || fail "HEAD is '$head' after picking feature/x, expected 'feature/x'"

  # The list is gone; only the branch name in the top bar changed.
  shoot "$SMOKE_OUT/g-switch-branch-done-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/g-switch-branch-$theme.png" "$SMOKE_OUT/g-switch-branch-done-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the view did not change after the checkout ($differing pixels)"
  differing="$(compare -metric AE "$RUN_DIR/closed.png" "$SMOKE_OUT/g-switch-branch-done-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 100 ] || fail "the top bar did not show the new branch ($differing pixels changed)"

  # A refused checkout: conflict changes the dirty file, so git says no.
  xdotool mousemove "$BRANCH_X" "$BRANCH_Y" click 1; sleep 1.5
  xdotool type --delay 20 -- conf; sleep 0.5
  xdotool key Return; sleep 1.5
  head="$(git -C "$repo" rev-parse --abbrev-ref HEAD)"
  [ "$head" = "feature/x" ] || fail "HEAD moved to '$head' by a checkout git refused"
  grep -qxF '// dirty' "$repo/api/routes.rs" || fail "the uncommitted change in api/routes.rs was touched"
  shoot "$SMOKE_OUT/g-switch-branch-refused-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/g-switch-branch-done-$theme.png" "$SMOKE_OUT/g-switch-branch-refused-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 500 ] || fail "the refused checkout showed no notice ($differing pixels changed)"
}

# --- 3h. First run: a folder that is not a repository, no recent repositories --
# The welcome screen replaces the window. Then the same folder with recents that
# still exist: the screen lists them under git's reason. Then no argument, so the
# app falls back to the newest recent repository and shows its working copy.
scenario_h() {
  local plain="$RUN_DIR/plain" theme="$1" recents="$RUN_DIR/xdg/gitmaster/recent.json"
  local before_plain recents_before differing
  mkdir -p "$plain"
  before_plain="$(find "$plain" | sort)"

  launch_app "$theme" "" "$plain"
  shoot "$SMOKE_OUT/h-welcome-$theme.png"
  stop_app
  [ -e "$plain/.git" ] && fail "a .git was created in the folder the app started in"
  [ "$(find "$plain" | sort)" = "$before_plain" ] || fail "the folder the app started in changed"
  [ -e "$recents" ] && fail "a recent list was written although no repository opened"

  printf '[\n  "%s",\n  "%s"\n]\n' "$RUN_DIR/billing-api" "$RUN_DIR/repo" > "$recents"
  recents_before="$(cat "$recents")"
  launch_app "$theme" "$plain" "$plain"
  shoot "$SMOKE_OUT/h-welcome-recents-$theme.png"
  stop_app
  differing="$(compare -metric AE "$SMOKE_OUT/h-welcome-$theme.png" "$SMOKE_OUT/h-welcome-recents-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the recent list and the reason did not show ($differing pixels changed)"
  [ "$(cat "$recents")" = "$recents_before" ] || fail "recent.json changed although no repository opened"

  launch_app "$theme" "" "$plain"
  shoot "$SMOKE_OUT/h-welcome-fallback-$theme.png"
  stop_app
  differing="$(compare -metric AE "$SMOKE_OUT/h-welcome-recents-$theme.png" "$SMOKE_OUT/h-welcome-fallback-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "no fallback to the newest recent repository ($differing pixels changed)"
  [ "$(find "$plain" | sort)" = "$before_plain" ] || fail "the folder the app started in changed"
}

# --- 3i. Stage one hunk with its header button ---------------------------------
scenario_i() {
  local repo="$RUN_DIR/repo" theme="$1" cached
  xdotool mousemove "$NAME_X" "$ROW_TOTAL_Y" click 1; sleep 1
  xdotool mousemove "$CHUNK_STAGE_X" "$CHUNK_Y" click 1; sleep 1.5

  cached="$(git -C "$repo" diff --cached -U0 --no-color -- src/ledger/total.rs)"
  grep -qF -- '+        // smoke: first' <<<"$cached" || fail "Stage Chunk did not stage the first line"
  grep -qF -- '+        // smoke: second' <<<"$cached" || fail "Stage Chunk did not stage the second line"
  git -C "$repo" diff --quiet -- src/ledger/total.rs \
    || fail "total.rs still has unstaged changes after Stage Chunk"

  shoot "$SMOKE_OUT/i-stage-chunk-$theme.png"
}

# --- 3j. Discard one chunk: Cancel keeps it, Discard reverts it ----------------
scenario_j() {
  local repo="$RUN_DIR/repo" theme="$1"
  xdotool mousemove "$NAME_X" "$ROW_TOTAL_Y" click 1; sleep 1

  xdotool mousemove "$CHUNK_DISCARD_X" "$CHUNK_Y" click 1; sleep 0.8
  xdotool mousemove "$CONFIRM_CANCEL_X" "$CHUNK_Y" click 1; sleep 0.8
  git -C "$repo" diff --quiet -- src/ledger/total.rs \
    && fail "Cancel in the discard confirmation changed total.rs"

  xdotool mousemove "$CHUNK_DISCARD_X" "$CHUNK_Y" click 1; sleep 0.8
  shoot "$SMOKE_OUT/j-discard-confirm-$theme.png"
  xdotool mousemove "$CONFIRM_DISCARD_X" "$CHUNK_Y" click 1; sleep 1.5

  git -C "$repo" diff --quiet -- src/ledger/total.rs \
    || fail "total.rs still has changes after Discard Chunk"
  grep -qF -- 'smoke: first' "$repo/src/ledger/total.rs" \
    && fail "the discarded line is still in total.rs"
  git -C "$repo" diff --cached --quiet \
    || fail "Discard Chunk touched the index"
  shoot "$SMOKE_OUT/j-discard-chunk-$theme.png"
}

# --- 3k. Right-click a file row, then Stage File in its menu -----------------
scenario_k() {
  local repo="$RUN_DIR/repo" theme="$1" staged differing
  shoot "$RUN_DIR/closed.png"

  xdotool mousemove "$NAME_X" "$ROW_MOD_Y" click 3; sleep 1
  shoot "$SMOKE_OUT/k-file-menu-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/closed.png" "$SMOKE_OUT/k-file-menu-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 5000 ] || fail "the file menu did not open ($differing pixels changed)"
  grep -qxF -- 'src/ledger/mod.rs' <<<"$(staged_names)" && fail "mod.rs was staged before the menu was used"

  xdotool mousemove "$MENU_STAGE_X" "$MENU_STAGE_Y" click 1; sleep 1.5
  staged="$(staged_names)"
  grep -qxF -- 'src/ledger/mod.rs' <<<"$staged" || fail "mod.rs not staged by Stage File in the menu"
  shoot "$SMOKE_OUT/k-file-menu-staged-$theme.png"
}

# --- 3l. Right-click a branch in the sidebar, then Checkout Branch in its menu -
scenario_l() {
  local repo="$RUN_DIR/repo" theme="$1" head differing
  shoot "$RUN_DIR/closed.png"

  xdotool mousemove "$SIDEBAR_FEATURE_X" "$SIDEBAR_FEATURE_Y" click 3; sleep 1
  shoot "$SMOKE_OUT/l-branch-menu-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/closed.png" "$SMOKE_OUT/l-branch-menu-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 5000 ] || fail "the branch menu did not open ($differing pixels changed)"

  # The Copy Branch Name row shows its shortcut at the right end, after a label
  # that stops well to the left. The strip there is blank without it.
  convert "$SMOKE_OUT/l-branch-menu-$theme.png" -crop "80x20+300+527" +repage "$RUN_DIR/shortcut.png"
  local ink
  ink="$(convert "$RUN_DIR/shortcut.png" -format '%[fx:standard_deviation]' info:)"
  awk -v m="$ink" 'BEGIN { exit !(m > 0.02) }' || fail "Copy Branch Name shows no shortcut at the row's end (sd=$ink)"

  xdotool mousemove "$MENU_CHECKOUT_X" "$MENU_CHECKOUT_Y" click 1; sleep 1.5
  head="$(git -C "$repo" rev-parse --abbrev-ref HEAD)"
  [ "$head" = "feature/x" ] || fail "HEAD is '$head' after Checkout Branch in the menu, expected 'feature/x'"
  shoot "$SMOKE_OUT/l-branch-menu-checked-out-$theme.png"
}

# --- 3m. Ctrl+K opens the palette; type, Tab to a file, Enter stages it ---------
scenario_m() {
  local repo="$RUN_DIR/repo" theme="$1" staged differing
  xdotool mousemove "$LIST_EMPTY_X" "$LIST_EMPTY_Y" click 1; sleep 1
  shoot "$RUN_DIR/closed.png"

  xdotool key ctrl+k; sleep 1.5
  shoot "$SMOKE_OUT/m-command-palette-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/closed.png" "$SMOKE_OUT/m-command-palette-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the palette did not open ($differing pixels changed)"

  # The query ranks Stage File first among the actions.
  xdotool type --delay 20 -- "stage file"; sleep 1.5
  shoot "$SMOKE_OUT/m-command-palette-filter-$theme.png"

  # Tab opens the argument step of Stage File: the files, filtered by "mod".
  xdotool key Tab; sleep 1.5
  xdotool type --delay 20 -- mod; sleep 1.5
  shoot "$SMOKE_OUT/m-command-palette-pick-$theme.png"

  # Enter runs Stage File on the highlighted file and closes the palette.
  xdotool key Return; sleep 1.5
  staged="$(staged_names)"
  grep -qxF -- 'src/ledger/mod.rs' <<<"$staged" || fail "mod.rs not staged by Stage File in the palette"
  grep -qxF -- 'src/ledger/total.rs' <<<"$staged" && fail "the palette staged a file other than the one picked"
  shoot "$RUN_DIR/m-done.png"

  # Ctrl+K opens it again, Escape closes it: the screen is as it was.
  xdotool key ctrl+k; sleep 1
  xdotool key Escape; sleep 1
  shoot "$RUN_DIR/m-escaped.png"
  differing="$(compare -metric AE "$RUN_DIR/m-done.png" "$RUN_DIR/m-escaped.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -lt 2000 ] || fail "Escape did not close the palette ($differing pixels changed)"
}


# --- Prepare-commit hooks (slice 2) -------------------------------------------
# The hook fixture is a script outside the repository. Each run appends a line to
# hook-runs.log, waits two seconds, then answers with the message or fails with
# the exit code.
write_hook() {
  # $1: script path, $2: message to answer with, $3: exit code (0 answers)
  cat > "$1" <<'HOOK'
#!/bin/sh
# smoke fixture: prepare-commit hook
echo run >> '__MARKER__'
id=$(sed -n 's/.*"invocation_id":"\([^"]*\)".*/\1/p')
sleep 2
if [ __EXIT__ -ne 0 ]; then
  echo 'llm: no model configured' >&2
  exit __EXIT__
fi
printf '{"invocation_id":"%s","output":{"message":"__MESSAGE__"}}\n' "$id"
HOOK
  sed -i -e "s|__MARKER__|$RUN_DIR/hook-runs.log|" -e "s|__EXIT__|$3|" -e "s|__MESSAGE__|$2|" "$1"
  chmod +x "$1"
}

# Opens the palette, types $1 and runs the first match with Enter.
run_from_palette() {
  xdotool key ctrl+k; sleep 1
  xdotool type --delay 20 -- "$1"; sleep 1
  xdotool key Return; sleep 1
}

hook_ran() { [ -s "$RUN_DIR/hook-runs.log" ]; }

# --- 3n. Text typed while the hook runs is kept; the answer is a suggestion ---
scenario_n() {
  local repo="$RUN_DIR/repo" theme="$1" got
  run_from_palette "suggest commit"
  xdotool mousemove "$COMPOSER_X" "$COMPOSER_Y" click 1; sleep 0.5
  xdotool type --delay 20 -- "my own"; sleep 3
  hook_ran || fail "the hook did not run"
  shoot "$SMOKE_OUT/n-typed-text-kept-$theme.png"

  # Enable Commit with a staged file, then commit whatever the composer holds.
  git -C "$repo" add src/ledger/mod.rs; sleep 2.5
  xdotool mousemove "$COMPOSER_X" "$COMPOSER_Y" click 1; sleep 0.5
  xdotool key ctrl+Return; sleep 1.5
  got="$(git -C "$repo" log -1 --format=%B)"
  [ "$got" = "my own" ] || fail "commit message is '$got': the typed text was overwritten by the hook"
}

# --- 3o. Repository changed during the run: Replace fills the composer -------
scenario_o() {
  local repo="$RUN_DIR/repo" theme="$1" got
  run_from_palette "suggest commit"
  # Staging while the hook runs moves the repository: its answer becomes a suggestion.
  sleep 0.5
  git -C "$repo" add src/ledger/mod.rs
  sleep 3
  hook_ran || fail "the hook did not run"
  shoot "$SMOKE_OUT/o-suggestion-replace-$theme.png"

  run_from_palette "replace with suggestion"; sleep 0.5
  xdotool mousemove "$COMPOSER_X" "$COMPOSER_Y" click 1; sleep 0.5
  xdotool key ctrl+Return; sleep 1.5
  got="$(git -C "$repo" log -1 --format=%B)"
  [ "$got" = "feat: from hook" ] || fail "after Replace the commit message is '$got', expected the suggestion"
}

# --- 3p. A failed hook shows a failed chip (error colour in the header) -------
scenario_p() {
  local theme="$1" shot="$SMOKE_OUT/p-hook-failure-$1.png" red error_hex
  run_from_palette "suggest commit"; sleep 3
  hook_ran || fail "the hook did not run"
  shoot "$shot"
  case "$theme" in
    light) error_hex="#89323a" ;;
    dark) error_hex="#fea1a4" ;;
  esac
  red="$(convert "$shot" -crop "${CHIP_W}x${CHIP_H}+${CHIP_X}+${CHIP_Y}" +repage \
    -fuzz 10% -fill white -opaque "$error_hex" -fuzz 0 -fill black +opaque white \
    -format '%[fx:mean]' info:)"
  awk -v m="$red" 'BEGIN { exit !(m > 0.01) }' \
    || fail "no failure-coloured text in the hook chip (mean=$red): $shot"
}

# --- 3q. A repository hook waits for approval; approval is stored outside the repo
scenario_q() {
  local repo="$RUN_DIR/repo" theme="$1" hook="$RUN_DIR/hook.sh"
  local approvals="$RUN_DIR/xdg/gitmaster/hook-approvals.json" want differing got status_before
  status_before="$(git -C "$repo" status --porcelain)"
  xdotool mousemove "$LIST_EMPTY_X" "$LIST_EMPTY_Y" click 1; sleep 1
  shoot "$RUN_DIR/approval-asked.png"

  run_from_palette "suggest commit"; sleep 3
  hook_ran && fail "a repository hook ran before it was approved"
  [ -e "$approvals" ] && fail "an approval was stored before the user approved"

  run_from_palette "approve repository hook"; sleep 1.5
  shoot "$RUN_DIR/approved.png"
  differing="$(compare -metric AE "$RUN_DIR/approval-asked.png" "$RUN_DIR/approved.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare approval screenshots: $differing"
  [ "$differing" -gt 2000 ] || fail "the approval prompt did not go away after Approve ($differing pixels changed)"

  [ -f "$approvals" ] || fail "Approve stored nothing in $approvals"
  want="$(printf '%s' "sh $hook" | sha256sum | cut -d' ' -f1)"
  grep -qF "$want" "$approvals" || fail "the stored approval does not name the command hash"
  [ "$(git -C "$repo" status --porcelain)" = "$status_before" ] \
    || fail "the working copy changed: approval must live outside the repository"

  run_from_palette "suggest commit"; sleep 3.5
  hook_ran || fail "the approved hook did not run"
  shoot "$SMOKE_OUT/q-repo-hook-approval-$theme.png"
  git -C "$repo" add src/ledger/mod.rs; sleep 2.5
  xdotool mousemove "$COMPOSER_X" "$COMPOSER_Y" click 1; sleep 0.5
  xdotool key ctrl+Return; sleep 1.5
  got="$(git -C "$repo" log -1 --format=%B)"
  [ "$got" = "feat: from repo hook" ] || fail "commit message is '$got', expected the approved hook's answer"
}

# --- 3r. Title bar: Fetch and Restack are disabled; a click says why ----------
scenario_r() {
  local theme="$1" differing
  shoot "$SMOKE_OUT/r-title-bar-$theme.png"
  # A click on a disabled button runs nothing and writes the reason in the
  # status bar, which therefore changes. Comparing the bar's strip is the
  # check, since the screenshot cannot be read as text here.
  convert "$SMOKE_OUT/r-title-bar-$theme.png" -crop "${WIN_W}x26+0+$((WIN_H - 26))" +repage "$RUN_DIR/status-before.png"
  xdotool mousemove "$TOOLBAR_FETCH_X" "$TOOLBAR_Y" click 1; sleep 1
  shoot "$RUN_DIR/after-fetch.png"
  convert "$RUN_DIR/after-fetch.png" -crop "${WIN_W}x26+0+$((WIN_H - 26))" +repage "$RUN_DIR/status-after.png"
  differing="$(compare -metric AE "$RUN_DIR/status-before.png" "$RUN_DIR/status-after.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 300 ] || fail "Fetch did not show its reason in the status bar ($differing pixels changed)"

  xdotool mousemove "$TOOLBAR_RESTACK_X" "$TOOLBAR_Y" click 1; sleep 1
  shoot "$SMOKE_OUT/r-title-bar-clicked-$theme.png"
}

run_scenario() {
  # $1: scenario name, $2: theme
  local name="$1" theme="$2"
  LABEL="$name-$theme"
  RUN_DIR="$RUN_ROOT/$LABEL"
  rm -rf "$RUN_DIR"
  mkdir -p "$RUN_DIR" "$RUN_DIR/xdg/gitmaster"
  : > "$RUN_DIR/gitconfig"
  make_repo "$RUN_DIR/repo"
  make_other_repo "$RUN_DIR/billing-api"
  case "$name" in
    n-typed-text-kept | o-suggestion-replace)
      write_hook "$RUN_DIR/hook.sh" "feat: from hook" 0
      printf '[gitmaster "prepare-commit.llm"]\n\tcommand = sh %s\n' "$RUN_DIR/hook.sh" > "$RUN_DIR/gitconfig"
      ;;
    p-hook-failure)
      write_hook "$RUN_DIR/hook.sh" "unused" 3
      printf '[gitmaster "prepare-commit.llm"]\n\tcommand = sh %s\n' "$RUN_DIR/hook.sh" > "$RUN_DIR/gitconfig"
      ;;
    q-repo-hook-approval)
      write_hook "$RUN_DIR/hook.sh" "feat: from repo hook" 0
      git -C "$RUN_DIR/repo" config gitmaster.prepare-commit.team.command "sh $RUN_DIR/hook.sh"
      ;;
  esac
  if [ "$name" = g-switch-branch ]; then prepare_branches "$RUN_DIR/repo"; fi
  if [ "$name" = l-branch-menu ]; then git -C "$RUN_DIR/repo" branch feature/x; fi
  if [ "$name" = h-welcome ]; then
    # Starts with no recent list at all; scenario_h makes its own launches.
    scenario_h "$theme"
    echo "PASS  $LABEL"
    return
  fi
  # Most recent first: the app opens repo, which moves it to the top.
  printf '[\n  "%s",\n  "%s"\n]\n' "$RUN_DIR/billing-api" "$RUN_DIR/repo" > "$RUN_DIR/xdg/gitmaster/recent.json"
  launch_app "$theme" "$RUN_DIR/repo"
  case "$name" in
    a-stage-lines) scenario_a "$theme" ;;
    b-stage-file)  scenario_b "$theme" ;;
    c-commit)      scenario_c "$theme" ;;
    d-refresh)     scenario_d "$theme" ;;
    e-command-log) scenario_e "$theme" ;;
    f-open-project) scenario_f "$theme" ;;
    g-switch-branch) scenario_g "$theme" ;;
    h-welcome) ;;
    i-stage-chunk) scenario_i "$theme" ;;
    j-discard-chunk) scenario_j "$theme" ;;
    k-file-menu) scenario_k "$theme" ;;
    l-branch-menu) scenario_l "$theme" ;;
    m-command-palette) scenario_m "$theme" ;;
    n-typed-text-kept) scenario_n "$theme" ;;
    o-suggestion-replace) scenario_o "$theme" ;;
    p-hook-failure) scenario_p "$theme" ;;
    q-repo-hook-approval) scenario_q "$theme" ;;
    r-title-bar) scenario_r "$theme" ;;
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

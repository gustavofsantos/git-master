#!/usr/bin/env bash
# End-to-end smoke test for the GitMaster window.
#
# Launches the real release binary on a private Xvfb display (:97), drives it
# with xdotool, and asserts the outcome with git. Screenshots go to $SMOKE_OUT.
# Each scenario runs once per theme (light, dark) on a freshly created repo.
#
# Usage:  scripts/ui-smoke.sh                  every scenario, both themes
#         scripts/ui-smoke.sh s-worktrees      only the named scenarios
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
SCENARIOS=(a-stage-lines b-stage-file c-commit d-refresh e-command-log f-open-project g-switch-branch h-welcome i-stage-chunk j-discard-chunk k-file-menu l-branch-menu m-command-palette n-typed-text-kept o-suggestion-replace p-hook-failure q-repo-hook-approval r-title-bar s-worktrees t-create-worktree u-remove-worktree v-lock-prune-worktree h-history h-history-unborn h-history-refresh h-commit-detail h-commit-diff h-commit-diff-split)

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
SECTION_CROP="240x84+0+612"                  # the WORKTREES rows at the sidebar's foot, window-relative
HISTORY_X=120; HISTORY_Y=175                 # "History" row under Workspace in the sidebar
WORKING_COPY_X=120; WORKING_COPY_Y=147       # "Working Copy" row, back from the history page
HISTORY_ROW_X=420; HISTORY_ROW_TOP_Y=162     # the history list's first row, 48 px rows below it
DETAIL_PANEL_CROP="360x700+920+88"           # the commit detail panel, window-relative
COMMIT_FILE_X=1082; COMMIT_FILE_Y=505        # api/new.rs, the first file row of the feat commit
SPLIT_FILE_X=1082; SPLIT_CALC_Y=449          # src/calc.rs, the file row of the split fixture's commit
LAYOUT_SPLIT_X=1320; LAYOUT_UNIFIED_X=1257; LAYOUT_Y=196   # "Split" and "Unified" in the diff header

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

# Always build: cargo is a no-op when current, and a stale binary would pass or fail for the wrong code.
echo "building $BIN ..."
(cd "$ROOT" && cargo build --release -p gitmaster)
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
    XDG_DATA_HOME="$RUN_DIR/xdg" XDG_CONFIG_HOME="$RUN_DIR/xdg-config" GIT_CONFIG_GLOBAL="$RUN_DIR/gitconfig" \
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

# --- 3h. The history page from the sidebar -------------------------------------
# Clicking History replaces the Working Copy with the commits of HEAD, newest
# first, and changes nothing in the repository. Going back to the Working Copy
# restores the screen it had before.
scenario_history() {
  local repo="$RUN_DIR/repo" theme="$1" status_before differing
  # Positive control: a space on a file row stages it, so the check further down
  # can tell a leaked key from a key that reached nothing. The control undoes itself.
  space_on_mod_file; sleep 1
  grep -qxF -- 'src/ledger/mod.rs' <<<"$(staged_names)" \
    || fail "the control failed: space on mod.rs did not stage it"
  space_on_mod_file; sleep 1
  # Focus the file list first: a space there stages, so a leaked key would show.
  xdotool mousemove "$LIST_EMPTY_X" "$LIST_EMPTY_Y" click 1; sleep 1
  shoot "$RUN_DIR/history-working-$theme.png"
  status_before="$(git -C "$repo" status --porcelain=v1; git -C "$repo" rev-parse HEAD)"

  xdotool mousemove "$HISTORY_X" "$HISTORY_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/h-history-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/history-working-$theme.png" "$SMOKE_OUT/h-history-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the history page did not replace the Working Copy ($differing pixels changed)"
  # The rows are text on the page background: the share of dark pixels in the
  # list area is zero on a blank page.
  convert "$SMOKE_OUT/h-history-$theme.png" -crop "1020x300+330+140" +repage "$RUN_DIR/history-list.png"
  local dark
  dark="$(convert "$RUN_DIR/history-list.png" -colorspace gray -threshold 50% -negate -format '%[fx:mean]' info:)"
  awk -v d="$dark" 'BEGIN { exit !(d > 0.0003) }' || fail "the history list is blank (dark share $dark)"

  [ "$(git -C "$repo" status --porcelain=v1; git -C "$repo" rev-parse HEAD)" = "$status_before" ] \
    || fail "opening the history changed the repository"
  # The list is gone from the page, so a space must not reach it.
  xdotool key space; sleep 1
  [ "$(git -C "$repo" status --porcelain=v1; git -C "$repo" rev-parse HEAD)" = "$status_before" ] \
    || fail "a key pressed on the history page changed the repository"

  xdotool mousemove "$WORKING_COPY_X" "$WORKING_COPY_Y" click 1; sleep 1.5
  shoot "$RUN_DIR/history-back-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/history-working-$theme.png" "$RUN_DIR/history-back-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -lt 2000 ] || fail "the Working Copy did not come back ($differing pixels differ)"
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

  # The failed step is in the command log: the panel opens over the window.
  xdotool mousemove "$LOG_BUTTON_X" "$LOG_BUTTON_Y" click 1; sleep 1
  local logged="$SMOKE_OUT/p-hook-log-$theme.png" differing
  shoot "$logged"
  differing="$(compare -metric AE "$shot" "$logged" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the command log did not open after the hook run (only $differing pixels changed): $logged"
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

# --- 3s. Worktrees section: two worktrees, then one removed from the CLI -------
# The section sits at the sidebar's foot. The screenshot cannot be read as text
# here, so the rows are read from pixels: the window's own worktree carries the
# accent "here" tag, and removing the other worktree from the CLI must change
# the rows without a manual refresh.
scenario_s() {
  local theme="$1" accent differing tagged
  case "$theme" in
    light) accent="006cbc" ;;
    dark) accent="64b2ff" ;;
    *) fail "no accent colour for theme $theme" ;;
  esac
  shoot "$SMOKE_OUT/s-worktrees-$theme.png"
  convert "$SMOKE_OUT/s-worktrees-$theme.png" -crop "$SECTION_CROP" +repage "$RUN_DIR/section-before.png"
  # The share of accent pixels in the section: white where the colour is, black elsewhere.
  tagged="$(convert "$RUN_DIR/section-before.png" -fuzz 10% -fill white -opaque "#$accent" \
    -fill black +opaque white -format '%[fx:mean]' info:)"
  awk -v m="$tagged" 'BEGIN { exit !(m > 0.01) }' || fail "no 'here' tag in the worktrees section (accent share $tagged)"

  git -C "$RUN_DIR/repo" worktree remove "$RUN_DIR/repo-feature"
  sleep 2.5
  shoot "$RUN_DIR/after-remove.png"
  convert "$RUN_DIR/after-remove.png" -crop "$SECTION_CROP" +repage "$RUN_DIR/section-after.png"
  differing="$(compare -metric AE "$RUN_DIR/section-before.png" "$RUN_DIR/section-after.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare section screenshots: $differing"
  [ "$differing" -gt 300 ] || fail "the section did not drop the removed worktree ($differing pixels changed)"
}

# --- 3t. Create a worktree from the palette's dialog --------------------------
# Typing in the palette picks the branch; Enter in the dialog creates the
# worktree on the sibling folder. Git's own refusal (main is checked out in the
# repository) stays in the dialog, and nothing is added. The check is git's
# listing, plus pixels for the dialog's appearing and leaving.
scenario_t() {
  local repo="$RUN_DIR/repo" theme="$1" differing listed
  local wt="$RUN_DIR/repo-feature-x"
  xdotool mousemove "$LIST_EMPTY_X" "$LIST_EMPTY_Y" click 1; sleep 1
  shoot "$RUN_DIR/closed.png"

  # The palette's Create Worktree takes a branch: Tab opens the branch step, and
  # Enter on feature/x opens the dialog with that branch chosen.
  xdotool key ctrl+k; sleep 1
  xdotool type --delay 20 -- "create worktree"; sleep 1
  xdotool key Tab; sleep 1
  xdotool type --delay 20 -- "feature/x"; sleep 1
  xdotool key Return; sleep 1.5
  shoot "$SMOKE_OUT/t-create-worktree-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/closed.png" "$SMOKE_OUT/t-create-worktree-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the create dialog did not open ($differing pixels changed)"

  # Enter creates it: the existing branch on the sibling folder, then the dialog is gone.
  xdotool key Return; sleep 2.5
  listed="$(git -C "$repo" worktree list --porcelain)"
  grep -qxF "worktree $wt" <<<"$listed" || fail "git worktree list does not show $wt: $listed"
  grep -qxF "branch refs/heads/feature/x" <<<"$listed" || fail "the new worktree is not on feature/x"
  [ "$(git -C "$wt" rev-parse --abbrev-ref HEAD)" = "feature/x" ] || fail "$wt is not on feature/x"
  [ -z "$(git -C "$wt" status --porcelain)" ] || fail "git status in $wt is not clean after the create"
  shoot "$RUN_DIR/t-created.png"
  differing="$(compare -metric AE "$SMOKE_OUT/t-create-worktree-$theme.png" "$RUN_DIR/t-created.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the dialog stayed open after the create ($differing pixels changed)"

  # A refused create: main is checked out in the repository, so git refuses and
  # its reason stays in the dialog. Nothing is added to the list.
  xdotool key ctrl+k; sleep 1
  xdotool type --delay 20 -- "create worktree"; sleep 1
  xdotool key Tab; sleep 1
  xdotool type --delay 20 -- "main"; sleep 1
  xdotool key Return; sleep 1.5
  xdotool key Return; sleep 2
  shoot "$SMOKE_OUT/t-create-refused-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/t-created.png" "$SMOKE_OUT/t-create-refused-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the refused create showed no dialog with its reason ($differing pixels changed)"
  [ "$(git -C "$repo" worktree list --porcelain | grep -c '^worktree ')" -eq 2 ] || fail "a refused create added a worktree"

  # Escape closes the dialog and leaves the list as it was.
  xdotool key Escape; sleep 1
  shoot "$RUN_DIR/t-closed.png"
  differing="$(compare -metric AE "$SMOKE_OUT/t-create-refused-$theme.png" "$RUN_DIR/t-closed.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "Escape did not close the create dialog ($differing pixels changed)"
  [ "$(git -C "$repo" worktree list --porcelain | grep -c '^worktree ')" -eq 2 ] || fail "closing the dialog changed the worktrees"
}

# --- 3u. Remove a dirty worktree from its menu (W5) ---------------------------
# The worktree's row menu opens the remove dialog, which lists the uncommitted
# change. Without the confirmation checkbox ticked, Remove is disabled and
# nothing goes. Ticking it enables Remove; the removal runs: git no longer lists
# the worktree and its folder is gone. The row positions are the WORKTREES rows
# at the sidebar's foot.
WT_ROW_X=182; WT_ROW_Y=731                  # the feature/x worktree row, right-clicked
MENU_REMOVE_X=252; MENU_REMOVE_Y=831        # "Remove Worktree" in that row's menu
CONFIRM_CHECK_X=486; CONFIRM_CHECK_Y=243  # the confirmation checkbox in the dialog
REMOVE_BUTTON_X=894; REMOVE_BUTTON_Y=280  # "Remove worktree" in the dialog
scenario_u() {
  local theme="$1" wt="$RUN_DIR/repo-feature" differing
  shoot "$RUN_DIR/u-before.png"
  xdotool mousemove "$WT_ROW_X" "$WT_ROW_Y" click 3; sleep 1
  shoot "$SMOKE_OUT/u-remove-menu-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/u-before.png" "$SMOKE_OUT/u-remove-menu-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 2000 ] || fail "the worktree menu did not open ($differing pixels changed)"

  xdotool mousemove "$MENU_REMOVE_X" "$MENU_REMOVE_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/u-remove-dialog-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/u-before.png" "$SMOKE_OUT/u-remove-dialog-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the remove dialog did not open ($differing pixels changed)"

  # Blocked: the button is disabled until the checkbox is ticked, so git keeps the worktree.
  xdotool mousemove "$REMOVE_BUTTON_X" "$REMOVE_BUTTON_Y" click 1; sleep 1.5
  [ -d "$wt" ] || fail "a removal without the confirmation deleted $wt"
  git -C "$RUN_DIR/repo" worktree list --porcelain | grep -qxF "worktree $wt" \
    || fail "git worktree list lost $wt without the confirmation"

  # Ticking the checkbox changes the dialog, and only then does Remove go.
  xdotool mousemove "$CONFIRM_CHECK_X" "$CONFIRM_CHECK_Y" click 1; sleep 1
  shoot "$SMOKE_OUT/u-remove-confirmed-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/u-remove-dialog-$theme.png" "$SMOKE_OUT/u-remove-confirmed-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 50 ] || fail "ticking the confirmation changed nothing in the dialog ($differing pixels changed)"
  [ -d "$wt" ] || fail "ticking the confirmation alone removed $wt"

  xdotool mousemove "$REMOVE_BUTTON_X" "$REMOVE_BUTTON_Y" click 1; sleep 2.5
  [ ! -e "$wt" ] || fail "the removed worktree folder $wt is still there"
  if git -C "$RUN_DIR/repo" worktree list --porcelain | grep -qxF "worktree $wt"; then
    fail "git worktree list still shows $wt after the removal"
  fi
  shoot "$RUN_DIR/u-after.png"
  convert "$RUN_DIR/u-after.png" -crop "$SECTION_CROP" +repage "$RUN_DIR/u-section-after.png"
  convert "$RUN_DIR/u-before.png" -crop "$SECTION_CROP" +repage "$RUN_DIR/u-section-before.png"
  differing="$(compare -metric AE "$RUN_DIR/u-section-before.png" "$RUN_DIR/u-section-after.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare section screenshots: $differing"
  [ "$differing" -gt 300 ] || fail "the sidebar still shows the removed worktree ($differing pixels changed)"
}

# --- 3v. Lock, unlock and prune worktrees from their menus (W6) ----------------
# The feature worktree's menu locks it, then unlocks it; Git's list shows each
# change. The gone worktree's menu opens the prune dialog, which lists the stale
# record; Prune removes that record and the folder of the live worktree stays.
# Row and menu positions are those of the WORKTREES rows at the sidebar's foot.
WT_FEATURE_X=182; WT_FEATURE_Y=703          # feature/x row, right-clicked
WT_GONE_X=182; WT_GONE_Y=731                # gone-branch row, right-clicked
MENU_LOCK_X=242; MENU_LOCK_Y=798            # "Lock Worktree" (or "Unlock Worktree") in feature/x's menu
MENU_PRUNE_X=250; MENU_PRUNE_Y=798          # "Prune Worktrees" in gone-branch's menu
PRUNE_BUTTON_X=912; PRUNE_BUTTON_Y=325     # "Prune" in the prune dialog
wt_lock_reason() {
  # Prints "locked" when git lists the worktree at $1 as locked, nothing otherwise.
  git -C "$RUN_DIR/repo" worktree list --porcelain \
    | awk -v want="worktree $1" '$0 == want { on = 1; next } /^worktree / { on = 0 } on && /^locked/ { print "locked" }'
}
scenario_v() {
  local theme="$1" feature="$RUN_DIR/repo-feature" gone="$RUN_DIR/repo-gone" differing
  shoot "$RUN_DIR/v-before.png"
  convert "$RUN_DIR/v-before.png" -crop "$SECTION_CROP" +repage "$RUN_DIR/v-section-before.png"

  xdotool mousemove "$WT_FEATURE_X" "$WT_FEATURE_Y" click 3; sleep 1
  xdotool mousemove "$MENU_LOCK_X" "$MENU_LOCK_Y" click 1; sleep 1.5
  [ "$(wt_lock_reason "$feature")" = locked ] || fail "git does not list $feature as locked after Lock Worktree"
  shoot "$SMOKE_OUT/v-locked-$theme.png"

  xdotool mousemove "$WT_FEATURE_X" "$WT_FEATURE_Y" click 3; sleep 1
  xdotool mousemove "$MENU_LOCK_X" "$MENU_LOCK_Y" click 1; sleep 1.5
  [ -z "$(wt_lock_reason "$feature")" ] || fail "git still lists $feature as locked after Unlock Worktree"

  xdotool mousemove "$WT_GONE_X" "$WT_GONE_Y" click 3; sleep 1
  xdotool mousemove "$MENU_PRUNE_X" "$MENU_PRUNE_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/v-prune-dialog-$theme.png"
  differing="$(compare -metric AE "$RUN_DIR/v-before.png" "$SMOKE_OUT/v-prune-dialog-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the prune dialog did not open ($differing pixels changed)"

  xdotool mousemove "$PRUNE_BUTTON_X" "$PRUNE_BUTTON_Y" click 1; sleep 2.5
  if git -C "$RUN_DIR/repo" worktree list --porcelain | grep -qxF "worktree $gone"; then
    fail "git worktree list still shows the stale $gone after Prune"
  fi
  [ ! -e "$RUN_DIR/repo/.git/worktrees/repo-gone" ] || fail "the admin record of $gone is still there"
  [ -d "$feature" ] || fail "pruning removed the live worktree folder $feature"
  shoot "$RUN_DIR/v-after.png"
  convert "$RUN_DIR/v-after.png" -crop "$SECTION_CROP" +repage "$RUN_DIR/v-section-after.png"
  differing="$(compare -metric AE "$RUN_DIR/v-section-before.png" "$RUN_DIR/v-section-after.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare section screenshots: $differing"
  [ "$differing" -gt 300 ] || fail "the sidebar still shows the pruned worktree ($differing pixels changed)"
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
  if [ "$name" = s-worktrees ]; then git -C "$RUN_DIR/repo" worktree add -q -b feature/x "$RUN_DIR/repo-feature"; fi
  if [ "$name" = l-branch-menu ]; then git -C "$RUN_DIR/repo" branch feature/x; fi
  if [ "$name" = t-create-worktree ]; then git -C "$RUN_DIR/repo" branch feature/x; fi
  if [ "$name" = v-lock-prune-worktree ]; then
    # A live worktree to lock, and one whose folder is gone: a stale record to prune.
    git -C "$RUN_DIR/repo" worktree add -q -b feature/x "$RUN_DIR/repo-feature"
    git -C "$RUN_DIR/repo" worktree add -q -b gone-branch "$RUN_DIR/repo-gone"
    rm -rf "$RUN_DIR/repo-gone"
  fi
  if [ "$name" = u-remove-worktree ]; then
    # A dirty worktree: the removal is blocked until the confirmation is ticked.
    git -C "$RUN_DIR/repo" worktree add -q -b feature/x "$RUN_DIR/repo-feature"
    printf 'scratch\n' > "$RUN_DIR/repo-feature/scratch.txt"
  fi
  if [ "$name" = h-history ]; then
    # Two commits beyond the first, so the list has rows to show. Only the index
    # is committed, so the working changes stay as make_repo left them.
    git -C "$RUN_DIR/repo" commit -q --allow-empty -m "smoke: history one"
    git -C "$RUN_DIR/repo" commit -q --allow-empty -m "smoke: history two"
  fi
  if [ "$name" = h-history-unborn ]; then
    scenario_history_unborn "$theme"
    echo "PASS  $LABEL"
    return
  fi
  if [ "$name" = h-history-refresh ]; then
    scenario_history_refresh "$theme"
    echo "PASS  $LABEL"
    return
  fi
  if [ "$name" = h-commit-detail ]; then
    make_detail_repo "$RUN_DIR/detail"
    scenario_commit_detail "$theme"
    echo "PASS  $LABEL"
    return
  fi
  if [ "$name" = h-commit-diff ]; then
    # An unstaged change, so that a stage key leaking into the diff would show.
    make_detail_repo "$RUN_DIR/detail"
    printf 'unstaged\n' >> "$RUN_DIR/detail/src/a.rs"
    scenario_commit_diff "$theme"
    echo "PASS  $LABEL"
    return
  fi
  if [ "$name" = h-commit-diff-split ]; then
    make_split_repo "$RUN_DIR/detail"
    scenario_commit_diff_split "$theme"
    echo "PASS  $LABEL"
    return
  fi
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
    s-worktrees) scenario_s "$theme" ;;
    t-create-worktree) scenario_t "$theme" ;;
    u-remove-worktree) scenario_u "$theme" ;;
    v-lock-prune-worktree) scenario_v "$theme" ;;
    h-history) scenario_history "$theme" ;;
    *) fail "unknown scenario $name" ;;
  esac
  stop_app
  echo "PASS  $LABEL"
}

# --- 3j. An unborn HEAD says so on the history page ----------------------------
# A repository with no commit: the history page shows one line and no list.
scenario_history_unborn() {
  local theme="$1" spread
  git init -q -b main "$RUN_DIR/unborn"
  launch_app "$theme" "$RUN_DIR/unborn"
  xdotool mousemove "$WAKE_X" "$WAKE_Y" click 1; sleep 1
  xdotool mousemove "$HISTORY_X" "$HISTORY_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/h-history-unborn-$theme.png"
  stop_app
  # The line is text in the middle of the page; a blank page has no spread at all.
  spread="$(convert "$SMOKE_OUT/h-history-unborn-$theme.png" -crop "480x60+520+400" +repage \
    -colorspace gray -format '%[fx:standard_deviation]' info:)"
  awk -v d="$spread" 'BEGIN { exit !(d > 0.01) }' \
    || fail "the unborn HEAD line is not on the history page (spread $spread)"
}

# --- 3k. A commit made outside the app reaches the open history page ------------
# The watcher refreshes the status when a ref moves, and the open page re-reads its top.
scenario_history_refresh() {
  local theme="$1" repo="$RUN_DIR/refresh" differing
  git init -q -b main "$repo"
  git -C "$repo" -c user.name=smoke -c user.email=smoke@smoke commit -q --allow-empty -m "smoke: first"
  launch_app "$theme" "$repo"
  # The first click wakes the window: until then it paints black.
  xdotool mousemove "$WAKE_X" "$WAKE_Y" click 1; sleep 1
  xdotool mousemove "$HISTORY_X" "$HISTORY_Y" click 1; sleep 1.5
  shoot "$RUN_DIR/refresh-before-$theme.png"
  git -C "$repo" -c user.name=smoke -c user.email=smoke@smoke commit -q --allow-empty \
    -m "smoke: made outside the app"
  sleep 3
  shoot "$SMOKE_OUT/h-history-refresh-$theme.png"
  stop_app
  convert "$RUN_DIR/refresh-before-$theme.png" -crop "1040x400+240+88" +repage "$RUN_DIR/list-before.png"
  convert "$SMOKE_OUT/h-history-refresh-$theme.png" -crop "1040x400+240+88" +repage "$RUN_DIR/list-after.png"
  differing="$(compare -metric AE "$RUN_DIR/list-before.png" "$RUN_DIR/list-after.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  # One new row of text moves the list: about 3000 pixels here, and none when nothing changed.
  [ "$differing" -gt 1000 ] || fail "the outside commit did not reach the open history ($differing pixels changed)"
}

# --- 3l. The commit detail panel (H3) ------------------------------------------
# A fixture with every kind of change the panel lists, and a merge. Selecting a
# row fills the panel; the repository does not change.
make_detail_repo() {
  local repo="$1"
  rm -rf "$repo"
  mkdir -p "$repo/src" "$repo/api"
  git -C "$repo" init -q -b main
  git -C "$repo" config user.name "gm smoke"
  git -C "$repo" config user.email "smoke@example.invalid"
  git -C "$repo" config commit.gpgsign false
  printf 'fn a() {}\n' > "$repo/src/a.rs"
  printf 'old\n' > "$repo/old.txt"
  printf 'gone\n' > "$repo/gone.txt"
  printf '\0\1\2\3' > "$repo/logo.bin"
  git -C "$repo" add -A
  git -C "$repo" commit -q -m "init"
  git -C "$repo" branch side
  # Add, modify, delete, rename and a binary rewrite, in one commit with a three-paragraph body.
  printf 'fn a() {}\nfn b() {}\n' > "$repo/src/a.rs"
  printf 'new\n' > "$repo/api/new.rs"
  git -C "$repo" rm -q gone.txt
  git -C "$repo" mv old.txt renamed.txt
  printf '\0\9\9\9\0' > "$repo/logo.bin"
  git -C "$repo" add -A
  git -C "$repo" commit -q -m "feat: add, modify, delete, rename" \
    -m "First paragraph of the body." \
    -m "Second paragraph, with a second line." \
    -m "Third paragraph."
  git -C "$repo" checkout -q side
  printf 'topic\n' > "$repo/topic.txt"
  git -C "$repo" add topic.txt
  git -C "$repo" commit -q -m "side: add topic"
  git -C "$repo" checkout -q main
  git -C "$repo" merge -q --no-ff side -m "Merge branch 'side'"
}

# Screen y of the history row whose subject is $1, in the order the app lists them.
history_row_y() {
  local index
  index="$(git -C "$RUN_DIR/detail" log --topo-order --format=%s HEAD | grep -nxF -- "$1" | head -1 | cut -d: -f1)"
  [ -n "$index" ] || fail "no commit named '$1' in the detail fixture"
  echo $(( HISTORY_ROW_TOP_Y + 48 * (index - 1) ))
}

# How many pixels of the detail panel differ between two window screenshots.
panel_changed() {
  local differing
  convert "$1" -crop "$DETAIL_PANEL_CROP" +repage "$RUN_DIR/panel-a.png"
  convert "$2" -crop "$DETAIL_PANEL_CROP" +repage "$RUN_DIR/panel-b.png"
  differing="$(compare -metric AE "$RUN_DIR/panel-a.png" "$RUN_DIR/panel-b.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  echo "$differing"
}

scenario_commit_detail() {
  local repo="$RUN_DIR/detail" theme="$1" status_before differing
  launch_app "$theme" "$repo"
  xdotool mousemove "$WAKE_X" "$WAKE_Y" click 1; sleep 1
  xdotool mousemove "$HISTORY_X" "$HISTORY_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-detail-empty-$theme.png"
  status_before="$(git -C "$repo" status --porcelain=v1; git -C "$repo" rev-parse HEAD)"

  xdotool mousemove "$HISTORY_ROW_X" "$(history_row_y "Merge branch 'side'")" click 1; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-detail-merge-$theme.png"
  differing="$(panel_changed "$SMOKE_OUT/h-commit-detail-empty-$theme.png" "$SMOKE_OUT/h-commit-detail-merge-$theme.png")"
  [ "$differing" -gt 2000 ] || fail "selecting the merge did not fill the detail panel ($differing pixels changed)"

  xdotool mousemove "$HISTORY_ROW_X" "$(history_row_y "feat: add, modify, delete, rename")" click 1; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-detail-feat-$theme.png"
  differing="$(panel_changed "$SMOKE_OUT/h-commit-detail-merge-$theme.png" "$SMOKE_OUT/h-commit-detail-feat-$theme.png")"
  [ "$differing" -gt 2000 ] || fail "selecting another commit did not replace the panel ($differing pixels changed)"
  stop_app

  [ "$(git -C "$repo" status --porcelain=v1; git -C "$repo" rev-parse HEAD)" = "$status_before" ] \
    || fail "reading a commit detail changed the repository"
}

# --- 3m. A file of a commit opens its diff, read-only (H4) ---------------------
# The feat commit has an added, a deleted, a binary and a renamed file, and a
# modified one. Down steps to the next file; Escape goes back to the commit.
# The fixture has an unstaged change: no key of the diff may stage or unstage it.
scenario_commit_diff() {
  local repo="$RUN_DIR/detail" theme="$1" status_before differing
  launch_app "$theme" "$repo"
  xdotool mousemove "$WAKE_X" "$WAKE_Y" click 1; sleep 1
  xdotool mousemove "$HISTORY_X" "$HISTORY_Y" click 1; sleep 1.5
  xdotool mousemove "$HISTORY_ROW_X" "$(history_row_y "feat: add, modify, delete, rename")" click 1; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-diff-commit-$theme.png"
  status_before="$(git -C "$repo" status --porcelain=v1; git -C "$repo" rev-parse HEAD)"

  xdotool mousemove "$COMMIT_FILE_X" "$COMMIT_FILE_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-diff-added-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/h-commit-diff-commit-$theme.png" "$SMOKE_OUT/h-commit-diff-added-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "the file diff did not replace the commit view ($differing pixels changed)"

  # Positive control: Down reaches the diff pane and moves to the next file.
  xdotool key Down; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-diff-deleted-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/h-commit-diff-added-$theme.png" "$SMOKE_OUT/h-commit-diff-deleted-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 2000 ] || fail "Down did not move to the next file ($differing pixels changed)"

  # Stage, unstage and discard keys do nothing in a commit's diff.
  xdotool key a s space u; sleep 1
  [ "$(git -C "$repo" status --porcelain=v1; git -C "$repo" rev-parse HEAD)" = "$status_before" ] \
    || fail "a key pressed in a commit's diff changed the repository"

  xdotool key Escape; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-diff-back-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/h-commit-diff-commit-$theme.png" "$SMOKE_OUT/h-commit-diff-back-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -lt 2000 ] || fail "Escape did not go back to the commit view ($differing pixels changed)"
  stop_app

  [ "$(git -C "$repo" status --porcelain=v1; git -C "$repo" rev-parse HEAD)" = "$status_before" ] \
    || fail "reading a commit's diff changed the repository"
}

# --- 3k. Split layout of a commit's diff (H4b-A, D39 to D41) --------------------
# One commit with a replacement (two lines become three), a pure addition and a
# pure deletion, so the split view shows paired rows and both kinds of empty side.
make_split_repo() {
  local repo="$1"
  rm -rf "$repo"
  mkdir -p "$repo/src"
  git -C "$repo" init -q -b main
  git -C "$repo" config user.name "gm smoke"
  git -C "$repo" config user.email "smoke@example.invalid"
  git -C "$repo" config commit.gpgsign false
  printf 'fn one() -> i32 { 1 }\nfn two() -> i32 { 2 }\nfn keep() {}\n' > "$repo/src/calc.rs"
  printf 'fn gone() {}\nfn stays() {}\n' > "$repo/src/gone.rs"
  git -C "$repo" add -A
  git -C "$repo" commit -q -m "init"
  printf 'fn one() -> u64 { 1 }\nfn two() -> u64 { 2 }\nfn three() -> u64 { 3 }\nfn keep() {}\n' > "$repo/src/calc.rs"
  printf 'pub fn fresh() {}\n' > "$repo/src/added.rs"
  git -C "$repo" rm -q src/gone.rs
  git -C "$repo" add -A
  git -C "$repo" commit -q -m "feat: replace, add and delete"
}

# Waits up to five seconds for the settings file to hold the layout $2.
wait_for_layout_saved() {
  local file="$1" layout="$2" tries=0
  until grep -qs "\"diff_layout\": \"$layout\"" "$file"; do
    tries=$((tries + 1))
    [ "$tries" -lt 50 ] || fail "the layout '$layout' was not saved to $file"
    sleep 0.1
  done
}

scenario_commit_diff_split() {
  local repo="$RUN_DIR/detail" theme="$1" differing
  local settings="$RUN_DIR/xdg-config/gitmaster/settings.json"
  launch_app "$theme" "$repo"
  xdotool mousemove "$WAKE_X" "$WAKE_Y" click 1; sleep 1
  xdotool mousemove "$HISTORY_X" "$HISTORY_Y" click 1; sleep 1.5
  xdotool mousemove "$HISTORY_ROW_X" "$(history_row_y "feat: replace, add and delete")" click 1; sleep 1.5
  xdotool mousemove "$SPLIT_FILE_X" "$SPLIT_CALC_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-diff-split-unified-$theme.png"

  xdotool mousemove "$LAYOUT_SPLIT_X" "$LAYOUT_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-diff-split-split-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/h-commit-diff-split-unified-$theme.png" "$SMOKE_OUT/h-commit-diff-split-split-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 20000 ] || fail "Split did not change the diff ($differing pixels changed)"

  # A pure addition and a pure deletion: one side is empty, and the file is not calc.rs.
  xdotool key Up; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-diff-split-added-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/h-commit-diff-split-split-$theme.png" "$SMOKE_OUT/h-commit-diff-split-added-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -gt 2000 ] || fail "src/added.rs did not open in the split view ($differing pixels changed)"
  # Two Down presses back to back: the second one is pressed while the first file
  # loads, and must still step on. Up then returns to calc.rs only if both landed.
  xdotool key Down Down; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-diff-split-gone-$theme.png"
  xdotool key Up; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-diff-split-recheck-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/h-commit-diff-split-split-$theme.png" "$SMOKE_OUT/h-commit-diff-split-recheck-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -lt 2000 ] || fail "the second Down press was lost: Up from src/gone.rs did not return to src/calc.rs ($differing pixels changed)"

  xdotool mousemove "$LAYOUT_UNIFIED_X" "$LAYOUT_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-diff-split-back-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/h-commit-diff-split-unified-$theme.png" "$SMOKE_OUT/h-commit-diff-split-back-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -lt 2000 ] || fail "Unified did not come back ($differing pixels changed)"

  # The split choice is saved and comes back after a restart of the app (D40).
  xdotool mousemove "$LAYOUT_SPLIT_X" "$LAYOUT_Y" click 1; sleep 1.5
  wait_for_layout_saved "$settings" split
  stop_app
  launch_app "$theme" "$repo"
  xdotool mousemove "$WAKE_X" "$WAKE_Y" click 1; sleep 1
  xdotool mousemove "$HISTORY_X" "$HISTORY_Y" click 1; sleep 1.5
  xdotool mousemove "$HISTORY_ROW_X" "$(history_row_y "feat: replace, add and delete")" click 1; sleep 1.5
  xdotool mousemove "$SPLIT_FILE_X" "$SPLIT_CALC_Y" click 1; sleep 1.5
  shoot "$SMOKE_OUT/h-commit-diff-split-restart-$theme.png"
  differing="$(compare -metric AE "$SMOKE_OUT/h-commit-diff-split-split-$theme.png" "$SMOKE_OUT/h-commit-diff-split-restart-$theme.png" null: 2>&1 || true)"
  read -r differing _ <<<"$differing"
  [[ "$differing" =~ ^[0-9]+$ ]] || fail "could not compare screenshots: $differing"
  [ "$differing" -lt 2000 ] || fail "the split layout was not kept across a restart ($differing pixels changed)"
  stop_app
}

# --- 2/3. Run everything -------------------------------------------------------
# The arguments name the scenarios to run; none means all of them.
selected=("$@")
if [ ${#selected[@]} -eq 0 ]; then selected=("${SCENARIOS[@]}"); fi
for name in "${selected[@]}"; do
  case " ${SCENARIOS[*]} " in
    *" $name "*) ;;
    *) echo "unknown scenario: $name (known: ${SCENARIOS[*]})" >&2; exit 2 ;;
  esac
done
start_xvfb
for theme in "${THEMES[@]}"; do
  for scenario in "${selected[@]}"; do
    run_scenario "$scenario" "$theme"
  done
done
echo "all scenarios passed; screenshots in $SMOKE_OUT"

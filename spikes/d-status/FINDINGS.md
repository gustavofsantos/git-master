# Spike D: status in very large repositories

Measured by a worker agent on 2026-10-16; written up by the orchestrator from its report. Raw numbers: `results/*.csv`. Reproduce: `export CARGO_TARGET_DIR=$HOME/.cache/gm-spikes/target; cd spikes && cargo build -p spike-d-status --release --offline; cd d-status && RUNS=7 ./bench.sh 50000` (also 200000, 500000); lock test: `$CARGO_TARGET_DIR/release/spike-d-status lock <repo> 30`.

## Environment

git 2.53.0 (Ubuntu), i5-1135G7 (nproc 8), NVMe, ext4, inotify `max_user_watches` 65536. Synthetic repos from `gen.sh` (13-byte files, 50 per directory; "dirty" = 1000 modified + 10000 untracked). The only real repo available, `~/Projects/rvw` (314 files), is too small to matter (2 ms). All runs are warm (no root for `drop_caches`).

**`git fsmonitor--daemon` is not supported on this git build** ("not supported on this platform"), so fsmonitor is unmeasured.

## Numbers (median of 7 warm runs, seconds)

Subprocess, the command `SubprocessGit` runs (`--no-optional-locks status --porcelain=v2 -z --branch --untracked-files=normal`), clean tree:

| config | 50k | 200k | 500k |
|---|---|---|---|
| untuned | 0.146 | 0.353 | 0.610 |
| untracked cache (primed) | 0.028 | 0.098 | 0.223 |
| `feature.manyFiles` + untracked cache | 0.027 | 0.094 | 0.242 |
| commit-graph | 0.057 | 0.238 | 0.576 |
| `feature.manyFiles` alone | 0.054 | 0.228 | 0.608 |

Dirty tree (11 002 records): untuned 0.078 / 0.247 / 0.534; untracked cache 0.054 / 0.137 / 0.280; manyFiles + untracked cache 0.042 / 0.124 / 0.309.

libgit2 (`Repository::statuses`, no untracked cache or fsmonitor), clean: 0.16 / 0.53–0.61 / 1.45–1.73. Dirty 500k: 1.41–1.48. Entry counts match the subprocess.

Index-lock test (concurrent `git add` + `git commit` loop while status runs 30 times): with `--no-optional-locks`, 0 status failures and 0 commit failures at every size. Plain `git status`: commits failed 39/43, 32/33, 41/41 with `Unable to create index.lock`. Small sample, indicative only.

## Verdict

1. **Subprocess stays the status backend** (confirms D26). libgit2 is about 2.5x slower at 500k and has no untracked-cache path.
2. **Keep `--no-optional-locks` on every background status.** Plain status broke concurrent commits almost every time; the app's command broke none. This matters for agents running `git` in parallel.
3. **Latency off the UI thread is enough at these sizes.** Untuned: 0.15 s (50k), 0.35 s (200k), 0.61 s (500k). Pass criterion "opens a large repo and lists changes without freezing" holds because status never runs on the UI thread (docs/concepts/principles.md).
4. **Do not auto-enable untracked cache or fsmonitor.** They write `.git` metadata; docs/concepts/principles.md says nothing is written into the repository without consent. Proposed rule: if a warm status exceeds 1 s, offer the tuning to the user.
5. **Untracked cache caveat (inferred from git docs, not measured):** `--no-optional-locks` status never writes the index, so the cache only gets primed when a plain git command runs (terminal, IDE). Needs a never-primed run to confirm.
6. **fsmonitor: no verdict.** Do not recommend `core.fsmonitor` until verified on a git build where the daemon works (macOS and Windows builds are the usual ones).
7. commit-graph has no measurable effect on status.

## Open doubts

No cold-cache numbers; uniform synthetic layout (untracked files in existing directories, so flat versus recursive untracked is not exercised); libgit2 with an untracked cache untested; fsmonitor unmeasured; point 5 unverified; lock test sample is small.

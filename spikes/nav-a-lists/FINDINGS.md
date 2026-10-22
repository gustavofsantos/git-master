# Spike S-A: file lists and grep at scale (docs/09)

Measured on 2026-10-21 by the agent, in a cloud container (4 vCPU, virtual disk, git 2.43.0). Raw numbers: `results/n100k.csv`, `results/n1m.csv`. Reproduce: `./gen.sh 100000 $HOME/.cache/gm-spikes/repos/nav100k checkout`, `./gen.sh 1000000 $HOME/.cache/gm-spikes/repos/nav1m` (index only), then `RUNS=5 python3 -I bench.py <repo> n100k worktree` (1M: `RUNS=2`, no `worktree`).

## Environment and limits

- Synthetic repositories from `gen.sh`: 100 000 and 1 000 000 files, paths of 3 to 6 components, about 40 bytes each, source-like content of 2 to 5 lines. Known token frequencies: `common_token` in 1 file in 10, `rare_token` in 1 in 20 000, `needle_unique` in one file.
- **1M has no checkout** (it does not fit the disk allowance of the session), so there is no 1M worktree grep: only `ls-files` from the index, `ls-tree`, and `git grep HEAD` over blobs.
- Warm runs only (no root for `drop_caches`). 5 runs at 100k, 2 at 1M, median. One virtual machine, not a laptop SSD; **the Mac numbers are still to be taken**.
- Content is tiny. Real files are bigger, so real grep times are higher; paths and counts are the part that scales like the real thing.

## Numbers (median, seconds)

| Measurement | 100k | 1M |
|---|---|---|
| `git ls-files -z`, all paths | 0.048 | 0.488 |
| `git ls-tree -r -z HEAD` | 0.148 | 0.537 |
| `ls-files -z`, first record | 0.044 | 0.563 |
| `ls-files -z`, peak RSS of git | 42 MiB | 267 MiB |
| output of `ls-files -z` | 3.7 MiB | 36.6 MiB |
| `grep HEAD`, common token, all (10 000 / 100 000 lines) | 2.40 | 8.90 |
| `grep HEAD`, common token, first result | 0.012 | 0.116 |
| `grep HEAD`, rare token, all | 2.32 | 8.85 |
| `grep HEAD`, rare token, first result | 1.03 | 3.95 |
| `grep HEAD`, unique token, first result | 1.39 | 5.44 |
| `grep HEAD -iE` regex, all | 2.34 | 9.13 |
| `grep` worktree, common, all / first | 1.53 / 0.035 | not measured |
| `grep` worktree, rare, all / first | 1.46 / 0.71 | not measured |
| `grep` worktree, unique, first result | 0.97 | not measured |
| `grep` worktree, rare, `--threads=1`, all | **0.67** | not measured |

## Reading

1. **The file list is cheap.** A million paths: half a second, 37 MB of output. `ls-files` prints nothing until it has read the whole index, so streaming gives no early records (first record at 1M: 0.56 s, no earlier than the 0.49 s total, within noise). It is a background read followed by one swap, not a stream. D46 stands: no "indexing…" wait on 100k, a half second on 1M.
2. **`git ls-files` holds 267 MiB at 1M files in its own process.** It is gone when the process ends; the app keeps the 37 MB buffer. Not a problem for the app.
3. **`git grep` is fast when the pattern has early hits and scans everything when it does not.** First result for a common token: 12 to 35 ms at 100k, 116 ms at 1M. For a rare or unique token the first result costs a full scan: about 1 s at 100k, 4 to 5 s at 1M over blobs. The target in `09` ("first grep result under 1 s at 100k files") holds for patterns with early hits and for the worktree at 100k (0.7 to 0.97 s), and only just.
4. **Searching a revision is slower than the worktree** (2.3 s against 1.5 s at 100k), as expected from reading and inflating blobs.
5. **Regex costs nothing over fixed strings here** (2.34 against 2.40 s): time is dominated by reading, not matching.
6. **`--threads=1` was twice as fast as the default on the worktree** (0.67 against 1.46 s). The default uses all cores and loses to contention on this VM. This is one machine and a virtual disk, so it is a doubt to repeat on the Mac, not a rule.

## Verdict

- **D44 (`git grep` as the grep engine) holds for 100k files**, with cancellation, a result cap and a pathspec field from the first version. At 1M files a search with no early hit takes seconds (about 9 s over blobs), which `09` already expects: the answer is to narrow with a pathspec and to show "searching…" with Cancel, not to build another engine. The in-process alternative (`regex` and `ignore` crates, D51) stays the fallback if the N6 gate fails on real repositories.
- **D46 (file list from `ls-files` in one buffer) holds.**
- N3 should measure `--threads` (default against 1 against N/2) on the Mac before fixing it.

## Open doubts

No cold cache; no 1M worktree grep; synthetic tiny files; one VM; 2 runs at 1M; no sparse checkout, submodule or very long path case; `LC_ALL=C` assumed.

## Correction (2026-10-22, found while building N3)

The file list timings above are for the tracked files only (`git ls-files -z`). The list Quick Open reads is `git ls-files -z -v --cached --others --exclude-standard`, and finding the untracked files makes Git walk the whole worktree, which is most of the cost:

| Command, 100k files, warm | seconds |
|---|---|
| `git ls-files -z --cached` | 0.05 |
| `git ls-files -z --others --exclude-standard` | 0.76 |
| `git ls-files -z --cached --others --exclude-standard` | 0.75 to 0.81 |
| `git status --porcelain -uall` | 0.86 to 1.13 |

So the real list read is about **0.75 s at 100k files**, and the same walk is paid by `git grep --untracked`. It runs in the background and the previous list keeps answering, so nothing blocks, but the first Quick Open of a session in a large repository waits for it, and 1 million files was not measured (that repository has no worktree). The first conclusion above ("no waiting on 100k") holds for the tracked files only.

`git grep` through the app's adapter (`grep_timings` in `crates/gm-core/tests/grep_real_git.rs`, release build, 100k files, one thread, median of 3):

| Scope | common (stops at 1000 lines) | rare | unique | no match |
|---|---|---|---|---|
| tracked only | 0.065 s | 0.93 s | 0.85 s | 0.89 s |
| tracked and new | 1.09 s | 2.06 s | 1.99 s | 2.00 s |

Searching the new files adds about a second at 100k files, even when there are none, and even for a pattern that is found at once. `-i` and `--column` cost nothing measurable. One thread was again faster than the default (0.9 s against 1.5 s with all threads).

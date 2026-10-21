# Spike S-C: path matcher over a million paths (docs/09)

Measured on 2026-10-21 by the agent, in a cloud container (4 vCPU, git 2.43.0). Raw numbers: `results/n100k.csv`, `results/n1m.csv`. Reproduce: `export CARGO_TARGET_DIR=$HOME/.cache/gm-spikes/target; cd spikes && cargo build -p spike-nav-c-match --release --offline`, then `git -C <repo> ls-files -z > paths.bin; $CARGO_TARGET_DIR/release/spike-nav-c-match paths.bin 7` (`SHOW=1` prints the top results of both matchers). The paths come from the repositories of spike S-A.

## What is compared

- **baseline**: `fuzzy_match` from `crates/gm-ui/src/fuzzy.rs`, copied verbatim, on every path, then a sort. This is what the palette runs today.
- **bytes**: a byte matcher for ASCII queries, over one contiguous buffer with `u32` offsets. A forward pass finds the end of the subsequence, a backward pass tightens its start, and the score has bonuses for a segment start (after `/`, `_`, `-`, `.`, camelCase), for adjacent letters, for the last path component, for the file name matched whole, and a small penalty for length. A top-K heap (K = 50) keeps the best, with ties broken by path order, never by arrival.
- **bytes over N threads**: the buffer is cut in N ranges, each with its own heap, merged at the end. The spike asserts that the top 50 is identical for 1, 4 and 7 threads and for the machine's thread count.

## Numbers (median of 7 runs, ms; baseline 1 run at 1M)

| Query | 100k baseline | 100k bytes, 1 thread | 100k bytes, 4 threads | 1M baseline | 1M bytes, 1 thread | 1M bytes, 4 threads |
|---|---|---|---|---|---|---|
| `file_0500` | 122 | 5.6 | 3.9 | 1349 | 51 | 25 |
| `corsrc` | 114 | 4.5 | 3.0 | 1057 | 47 | 23 |
| `docsmd` | 119 | 4.9 | 2.5 | 1174 | 48 | 18 |
| `auth` | 94 | 4.4 | 2.8 | 1022 | 43 | 21 |
| `rs` (61 % of paths match) | 100 | 5.1 | 3.2 | 974 | 42 | 25 |
| `mod12` | 128 | 6.3 | 3.4 | 1207 | 56 | 16 |
| `crates/ui/src/file` | 139 | 5.3 | 2.6 | 1317 | 43 | 21 |
| `zzzzqq` (no match) | 129 | 4.4 | 2.8 | 1080 | 62 | 16 |

Memory for 1M paths: 37.5 MiB of bytes plus 3.9 MiB of offsets. Loading the buffer from a file: 81 ms.

## Verdict

1. **D46 holds: a separate path matcher is required, and it is easily fast enough.** The current matcher takes about 1 s at a million paths because it allocates a `Vec<char>` per call. The byte matcher takes 16 to 25 ms on 4 threads and 40 to 60 ms on one, so the target "a key press answers in under 50 ms with the list ready" holds at 1M paths even without threads on this machine, and with a wide margin at 100k (3 to 6 ms).
2. **The result does not depend on the thread count.** Ties break by path order, which is the order of the buffer.
3. **The old matcher is also a poor fit for paths**, as `SHOW=1` shows: it ranks by where the run starts and knows nothing of directories or file names (`auth` puts `docs/auth0/bench/mod0/…` before `services/auth0/…`, and `ui/sr` puts a `.py` file four levels down first). The weights of the byte matcher are a first draft that N1 must tune with tests; the spike only proves cost and determinism.

## Open doubts

ASCII queries only (N1 falls back to the character matcher for a non-ASCII query, which is slow but rare); candidate ASCII-folding of non-ASCII paths not measured; one VM, not the Mac; synthetic paths of uniform shape; the ranking is a draft and untested against real repositories; highlight ranges (positions are computed but not returned) are N1's work.

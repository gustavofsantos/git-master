# Spike C: in-memory restack (libgit2 vs `git merge-tree`)

Question (Q6, docs/02 "Restack"): can GitMaster restack a branch stack entirely in memory, without touching the working tree or index, and surface conflicts?

Run: `cargo test -p spike-c-rebase` (13 tests, all pass) and `cargo run --release -p spike-c-rebase` (timing). git 2.53.0, git2 0.20.4 (libgit2 bundled). Scratch repos under `$GM_SPIKE_TMP` (`~/.cache/gm-spikes/tmp`).

## Method

- Each scenario builds two identical repos from the same setup. Engine repo: libgit2 and merge-tree run on it. Reference repo: `git rebase --onto <new-parent> <oldtip-parent> <branch>` per edge, in pre-order. Conflicts in the reference are resolved with the same resolver, then `rebase --continue`.
- Engine repo: asserts `.git/index` bytes and `.git/HEAD` unchanged, no `rebase-merge`/`rebase-apply` state, `git status --porcelain` empty.
- Match means: every branch tip has the same tree as the reference and the same `rev-list --count`. Commit ids are not compared.
- Conflicts: match means the same conflicted paths and the same base/ours/theirs blob contents as `git rebase` stage entries.

## Coverage table

| Scenario | libgit2 in-memory | merge-tree per commit |
|---|---|---|
| Clean 3-branch stack (main <- A <- B <- C) | MATCH | MATCH |
| Conflict in middle branch, stop | CONFLICT, same paths and 3 sides as git (f.txt) | CONFLICT, same paths and 3 sides as git (f.txt) |
| Conflict in middle branch, resolve programmatically, continue to C | MATCH (resolved=1) | MATCH (resolved=1) |
| Rename in base branch (f.txt -> g.txt, A edits f.txt) | MATCH | MATCH |
| Commit becomes empty (already in main) | ERROR without workaround (`Applied`); MATCH with skip-on-`Applied` | MATCH (dropped) |
| Merge commit inside a branch | MATCH (merges flattened, same counts as git) | MATCH |
| Binary file change, clean | MATCH | MATCH |
| Binary file conflict, stop | CONFLICT, same sides as git (bin.dat) | CONFLICT, same sides as git |
| Binary file conflict, resolve, continue | MATCH (resolved=1) | MATCH (resolved=1) |
| 3000 files, 2 branches | MATCH | MATCH |
| File mode change (644 -> 755) | MATCH | MATCH |
| Parent amended after child cut (stackParentBase = old tip) | MATCH | MATCH |
| 200-commit stack (10 x 20) | MATCH | MATCH |

Untouched-repo assertions (index bytes, HEAD, state dirs, clean status) passed in every row for both engines, including the conflict-stop rows.

## Timings (200 commits, 10 branches x 20 commits, one release run)

| Approach | Time |
|---|---|
| libgit2 in-memory (`Repository::rebase`, inmemory) | 135 ms |
| `git merge-tree --write-tree --merge-base` + `commit-tree` per commit | 2377 ms |
| `git rebase --onto` per branch (reference, subprocess) | 549 ms |

Single run, not statistically averaged. merge-tree costs about one subprocess pair per commit, so it scales with commit count.

## Gaps and notes

- **libgit2 empty commits:** `Repository::rebase` returns `Applied` ("this patch has already been applied") from `commit()` when a commit becomes empty. The spike skips such commits on `Applied` (`SKIP_APPLIED`), which then matches git. Without the workaround the scenario errors. Whether this is the intended libgit2 API flow was not verified against libgit2 docs.
- **libgit2 conflicts:** `next()` reports the conflict either as an error with `Unmerged` or as `Ok` with `inmemory_index().has_conflicts()`. The spike handles both. Conflict entries give stage 1/2/3 blobs, so base/ours/theirs are available. Resolution: `conflict_remove` plus `Index::add` with a blob written by `repo.blob()`. `Index::add_frombuffer` fails on the in-memory index ("Index is not backed up by an existing repository"). `ours` is the new parent side, `theirs` is the replayed commit, same as git.
- **merge-tree version:** correct per-commit cherry-pick needs `--merge-base=<parent>`, which exists only in git >= 2.40. The brief said 2.38; on 2.38/2.39 the merge base would be computed wrongly. Require 2.40+ or drop this engine. Not tested on older git.
- **merge-tree conflicts:** the conflicted-file-info section gives stage blob ids, so sides are available. Output filenames are not `-z`-quoted in this spike; paths with special characters are untested.
- **merge-tree objects:** it writes tree/blob/commit objects to the object database and creates no refs. Not the working tree or index.
- **Not tested (no scenario, no claim):** rerere, rebase hooks (`pre-restack`/`post-restack`), commit signing, submodules, LFS, `--rebase-merges` semantics (both engines flatten merges here), Windows, and very large repos (performance beyond 3000 files).
- **Deleted parent branch:** not tested as a separate case. Both engines take oids/revs captured at plan time, so the result should not depend on the branch still existing; this is an inference.
- **gitoxide (from docs, not tested):** the crate-status page fetched via WebFetch lists `gix` merge, rebase and cherry-pick "workflow orchestration", `gix-rebase` (drive/status), `gix-cherry-pick`, `gix-sequencer` and `gix-rerere` as items, but the fetched text did not show their done/incomplete state, and it lists "merge workflow" and "rebase workflow" orchestration as still-to-do plumbing. Treat gitoxide as not ready for rebase-in-memory on this evidence. No gix code was run.

## Verdict on Q6

- **Restack backend: libgit2 in-memory.** It covers every tested scenario (with the `Applied` skip), returns correct conflict sides and resumes after programmatic resolution, and is about 18x faster than per-commit merge-tree on 200 commits. D26 (hybrid, libgit2 for merge/rebase in memory) holds for this question.
- **merge-tree:** keep as an option or cross-check (it is the same algorithm as git's ort merge). It is slower and needs git >= 2.40.
- **gitoxide:** do not switch now. Evaluate it only if libgit2 fails on rerere, signing or hooks, which were not tested here. Those should be spiked before D26 is closed.
- Q6 in docs/01 is not updated by this spike; the author should record the outcome there.

## Open doubts

- Empty-commit handling via `Applied` depends on libgit2 semantics not checked against upstream docs.
- The reference is `git rebase`, not `git merge-tree`, so a match shows agreement with git's behaviour, not with the merge-tree backend's own correctness.
- Timings are a single release run on one machine.

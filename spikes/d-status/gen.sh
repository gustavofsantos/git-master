#!/usr/bin/env bash
# Deterministic synthetic repositories for spike D (status scale).
#
#   gen.sh clean <files> <repo-dir>   build a repo with <files> tracked files in a
#                                     4-level-deep tree (50 files per leaf dir),
#                                     committed with git fast-import, worktree materialized.
#   gen.sh dirty <files> <repo-dir>   on a clean repo: rewrite 1000 tracked files and
#                                     add 10000 untracked files inside existing dirs.
#
# Same arguments always produce the same commit ids and file contents
# (fixed author/committer dates and content derived from the index).
set -euo pipefail

mode=${1:?mode: clean|dirty}
files=${2:?file count}
repo=${3:?repo dir}

if [[ $mode == clean ]]; then
  rm -rf "$repo"
  mkdir -p "$repo"
  git -C "$repo" init -q -b main
  python3 -I - "$files" <<'PY' | git -C "$repo" fast-import --quiet
import sys
n = int(sys.argv[1])
out = sys.stdout.buffer
msg = b"synthetic spike repo\n"
out.write(b"commit refs/heads/main\n")
out.write(b"committer Spike <spike@example.invalid> 1700000000 +0000\n")
out.write(b"data %d\n" % len(msg) + msg)
for i in range(n):
    k = i // 50
    d = [(k // 20**j) % 20 for j in (3, 2, 1, 0)]
    path = "l%d/l%d/l%d/l%d/k%d/f%07d.txt" % (d[0], d[1], d[2], d[3], k, i)
    body = b"file %07d\n" % i
    out.write(b"M 100644 inline %s\n" % path.encode())
    out.write(b"data %d\n" % len(body) + body)
out.write(b"\n")
PY
  git -C "$repo" reset -q --hard
  echo "clean: $(git -C "$repo" ls-files | wc -l) tracked files, $(git -C "$repo" rev-parse --short HEAD)"

elif [[ $mode == dirty ]]; then
  python3 -I - "$files" "$repo" <<'PY'
import os, sys
n, repo = int(sys.argv[1]), sys.argv[2]
ndirs = (n + 49) // 50
def leaf(k):
    d = [(k // 20**j) % 20 for j in (3, 2, 1, 0)]
    return os.path.join(repo, "l%d/l%d/l%d/l%d/k%d" % (d[0], d[1], d[2], d[3], k))
# 1000 modified tracked files, spread evenly over the index.
step = max(1, n // 1000)
for i in range(0, n, step)[:1000]:
    k = i // 50
    p = os.path.join(leaf(k), "f%07d.txt" % i)
    with open(p, "wb") as f:
        f.write(b"modified %07d\n" % i)
# 10000 untracked files inside existing leaf dirs (so they are listed, not collapsed).
for j in range(10000):
    k = (j * 37) % ndirs
    p = os.path.join(leaf(k), "new%05d.txt" % j)
    with open(p, "wb") as f:
        f.write(b"untracked %05d\n" % j)
print("dirty: 1000 modified, 10000 untracked")
PY
else
  echo "unknown mode: $mode" >&2
  exit 2
fi

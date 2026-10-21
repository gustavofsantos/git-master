#!/usr/bin/env bash
# Deterministic synthetic repository for spike S-A (docs/09: lists and grep at scale).
#
#   gen.sh <files> <repo-dir> [checkout]
#
# Builds <files> tracked files with git fast-import: paths of depth 3 to 6 under a
# few top-level dirs, mixed extensions, source-like content. Tokens in the content
# have known frequencies (see FINDINGS.md): `common_token` in 1 file out of 10,
# `rare_token` in 1 out of 20000, `needle_unique` in exactly one file.
# With `checkout` the worktree is materialized (skipped by default: at 1M files it
# does not fit the disk allowance of the cloud session).
# Same arguments always produce the same commit.
set -euo pipefail

files=${1:?file count}
repo=${2:?repo dir}
checkout=${3:-}

rm -rf "$repo"
mkdir -p "$repo"
git -C "$repo" init -q -b main
python3 -I - "$files" <<'PY' | git -C "$repo" fast-import --quiet
import sys
n = int(sys.argv[1])
out = sys.stdout.buffer
tops = ["crates", "packages", "services", "tools", "docs", "vendor"]
mids = ["core", "ui", "api", "store", "net", "auth", "cache", "index", "render", "sync"]
subs = ["src", "tests", "bench", "include", "lib"]
exts = [".rs", ".ts", ".py", ".go", ".md", ".json", ".c", ".h"]
msg = b"synthetic spike repo\n"
out.write(b"commit refs/heads/main\n")
out.write(b"committer Spike <spike@example.invalid> 1700000000 +0000\n")
out.write(b"data %d\n" % len(msg) + msg)
for i in range(n):
    t = tops[i % 6]
    m = mids[(i // 6) % 10]
    s = subs[(i // 60) % 5]
    depth = 3 + (i % 4)                       # 3..6 components
    parts = [t, "%s%d" % (m, (i // 300) % 40)]
    if depth >= 4:
        parts.append(s)
    for d in range(depth - 4):
        parts.append("mod%d" % ((i // (7 * (d + 1))) % 25))
    parts.append("file_%07d%s" % (i, exts[i % 8]))
    path = "/".join(parts[:depth] if depth < 4 else parts)
    lines = ["// file %07d" % i, "pub fn item_%d() -> usize { %d }" % (i, i % 97)]
    if i % 10 == 0:
        lines.append("let x = common_token(%d);" % i)
    if i % 20000 == 7:
        lines.append("let y = rare_token(%d);" % i)
    if i == n // 2:
        lines.append("let z = needle_unique();")
    lines.append("struct Holder%d { value: u32 }" % (i % 1000))
    body = ("\n".join(lines) + "\n").encode()
    out.write(b"M 100644 inline %s\n" % path.encode())
    out.write(b"data %d\n" % len(body) + body)
out.write(b"\n")
PY
if [[ $checkout == checkout ]]; then
  git -C "$repo" reset -q --hard
else
  git -C "$repo" read-tree HEAD          # index only: enough for ls-files
fi
echo "files=$(git -C "$repo" ls-tree -r --name-only HEAD | wc -l) head=$(git -C "$repo" rev-parse --short HEAD) pack=$(du -sh "$repo/.git" | cut -f1)"

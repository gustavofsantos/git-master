#!/usr/bin/env python3
"""Spike S-A timings (docs/09). Usage: bench.py <repo> <label> [worktree]

Every measurement is the median of RUNS warm runs (no root for drop_caches).
`first` is the time until the first output line, then the process is killed,
which is what a cancellable streaming search sees.
"""
import os, statistics, subprocess, sys, time

repo, label = sys.argv[1], sys.argv[2]
worktree = len(sys.argv) > 3 and sys.argv[3] == "worktree"
RUNS = int(os.environ.get("RUNS", "5"))
NUL, NL = b"\0", b"\n"
ENV = dict(os.environ, LC_ALL="C", GIT_OPTIONAL_LOCKS="0")


def git(*args):
    return ["git", "-C", repo, "--no-optional-locks", *args]


def full(cmd):
    t = time.perf_counter()
    out = subprocess.run(cmd, env=ENV, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL).stdout
    return time.perf_counter() - t, out


def first(cmd):
    t = time.perf_counter()
    p = subprocess.Popen(cmd, env=ENV, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    line = p.stdout.readline()
    dt = time.perf_counter() - t
    p.kill()
    p.wait()
    return dt, line


def median(fn, cmd):
    times = []
    last = b""
    for _ in range(RUNS):
        dt, last = fn(cmd)
        times.append(dt)
    return statistics.median(times), last


rows = []


def row(name, secs, extra=""):
    rows.append((name, secs, extra))
    print(f"{label},{name},{secs:.3f},{extra}", flush=True)


# --- file lists
secs, out = median(full, git("ls-files", "-z"))
row("ls-files -z (all)", secs, f"{out.count(NUL)} paths, {len(out) // 1024} KiB")
secs, out = median(full, git("ls-tree", "-r", "-z", "--name-only", "HEAD"))
row("ls-tree -r -z HEAD (all)", secs, f"{out.count(NUL)} paths, {len(out) // 1024} KiB")
secs, _ = median(first, git("ls-files", "-z"))
row("ls-files -z (first record)", secs)

# --- grep in a revision (blobs), works without a checkout
base = ["grep", "-I", "-n", "--column", "-z", "--no-color", "-F"]
for name, token in [("common", "common_token"), ("rare", "rare_token"), ("unique", "needle_unique")]:
    secs, out = median(full, git(*base, "-e", token, "HEAD"))
    row(f"grep HEAD {name} (all)", secs, f"{out.count(NL)} lines")
    secs, _ = median(first, git(*base, "-e", token, "HEAD"))
    row(f"grep HEAD {name} (first result)", secs)

# regex, case-insensitive
secs, out = median(full, git("grep", "-I", "-n", "-z", "--no-color", "-i", "-E", "-e", r"holder4[0-9]{2} \{", "HEAD"))
row("grep HEAD -iE (all)", secs, f"{out.count(NL)} lines")

# --- grep in the worktree (needs a checkout)
if worktree:
    for name, token in [("common", "common_token"), ("rare", "rare_token"), ("unique", "needle_unique")]:
        secs, out = median(full, git(*base, "-e", token))
        row(f"grep worktree {name} (all)", secs, f"{out.count(NL)} lines")
        secs, _ = median(first, git(*base, "-e", token))
        row(f"grep worktree {name} (first result)", secs)
    secs, out = median(full, git(*base, "--threads=1", "-e", "rare_token"))
    row("grep worktree rare --threads=1 (all)", secs)

# --- peak memory of ls-files -z, measured in a fresh interpreter (ru_maxrss of its child)
code = ("import resource,subprocess,sys;"
        "subprocess.run(['git','-C',sys.argv[1],'ls-files','-z'],stdout=subprocess.DEVNULL);"
        "print(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)")
r = subprocess.run([sys.executable, "-I", "-c", code, repo], env=ENV, stdout=subprocess.PIPE)
row("ls-files -z peak RSS KiB", float(r.stdout.decode().strip()))

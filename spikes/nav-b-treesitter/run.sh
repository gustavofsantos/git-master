#!/usr/bin/env bash
# Spike S-B measurements (design/code-navigation.md). Writes results/*.txt.
#
# Needs a release build with every grammar and the Rust truth tool:
#   export CARGO_TARGET_DIR=$HOME/.cache/gm-spikes/target-b
#   (cd spikes && cargo build --release -p spike-nav-b-treesitter --features all \
#                 -p spike-nav-b-truth-rust)
# and the corpora (see FINDINGS.md): a cargo registry, the Go standard library,
# the Python standard library, shallow clones of the repositories listed in
# fetch-corpora.sh, and the `typescript` npm module for the ground truth.
set -u
here=$(cd "$(dirname "$0")" && pwd)
out=$here/results; mkdir -p "$out"
T=${CARGO_TARGET_DIR:-$HOME/.cache/gm-spikes/target-b}/release
B=$T/spike-nav-b-treesitter; R=$T/spike-nav-b-truth-rust
C=${CORPORA:-$HOME/.cache/gm-spikes/corpora}
RUST=${RUST_SRC:-$HOME/.cargo/registry/src}
GO=${GO_SRC:-/usr/local/go/src}
PY=${PY_SRC:-/usr/lib/python3.13}
JSMOD=${JS_SRC:-/opt/node22/lib/node_modules}
export TS_MODULE=${TS_MODULE:-/opt/node-tools/node_modules/typescript}
tmp=${TMPDIR:-/tmp}/s-b; mkdir -p "$tmp"
Q=$here/queries; X=$here/truth
step() { echo "== $1"; }

step "rust"
$B parse rust "$RUST" > "$out/rust-parse.txt" 2>&1
$B tags rust "$RUST" --dump "$tmp/rust.tags" > "$out/rust-tags.txt" 2>&1
$R "$RUST" > "$tmp/rust.truth"
python3 -I "$X/compare.py" "$tmp/rust.truth" "$tmp/rust.tags" --show 12 > "$out/rust-compare.txt"

step "python"
for name in stdlib:$PY django:$C/django; do
  n=${name%%:*}; root=${name#*:}
  $B parse python "$root" > "$out/python-$n-parse.txt" 2>&1
  $B tags python "$root" --dump "$tmp/py-$n.tags" > "$out/python-$n-tags.txt" 2>&1
  python3 -I "$X/py_truth.py" "$root" > "$tmp/py-$n.truth"
  python3 -I "$X/compare.py" "$tmp/py-$n.truth" "$tmp/py-$n.tags" --show 12 > "$out/python-$n-compare.txt"
done

step "go"
$B parse go "$GO" > "$out/go-parse.txt" 2>&1
$B tags go "$GO" --dump "$tmp/go.tags" > "$out/go-tags.txt" 2>&1
(cd "$X/go_truth" && /usr/local/go/bin/go run . "$GO") > "$tmp/go.truth"
python3 -I "$X/compare.py" "$tmp/go.truth" "$tmp/go.tags" --show 12 > "$out/go-compare.txt"

step "typescript (shipped / combined with JS / combined plus overlay)"
$B parse typescript "$C/TypeScript" > "$out/typescript-parse.txt" 2>&1
node "$X/ts_truth.js" "$C/TypeScript" ts,tsx > "$tmp/ts.truth"
for variant in shipped combined overlay-all; do
  f=$Q/typescript-$variant.scm
  $B tags typescript "$C/TypeScript" --tags-file "$f" --dump "$tmp/ts-$variant.tags" > "$out/typescript-$variant-tags.txt" 2>&1
  python3 -I "$X/compare.py" "$tmp/ts.truth" "$tmp/ts-$variant.tags" --show 12 > "$out/typescript-$variant-compare.txt"
done

step "javascript"
$B parse javascript "$JSMOD" > "$out/javascript-parse.txt" 2>&1
$B tags javascript "$JSMOD" --dump "$tmp/js.tags" > "$out/javascript-tags.txt" 2>&1
node "$X/ts_truth.js" "$JSMOD" js,mjs,cjs > "$tmp/js.truth"
python3 -I "$X/compare.py" "$tmp/js.truth" "$tmp/js.tags" --show 12 > "$out/javascript-compare.txt"

step "second wave (cost and sanity only: no ground truth)"
for pair in c:git cpp:json ruby:rails java:guava csharp:Newtonsoft.Json; do
  l=${pair%%:*}; d=${pair#*:}
  $B tags $l "$C/$d" > "$out/wave2-$l-tags.txt" 2>&1
done

step "threads"
for t in 1 2 4; do $B parse rust "$RUST" --threads $t > "$out/rust-parse-threads$t.txt" 2>&1; done

step "highlight"
: > "$out/highlight.txt"
for pair in rust:$(find "$RUST" -name tables.rs -path '*unicode-width*' | head -1) \
            go:$GO/syscall/zerrors_linux_amd64.go \
            python:$PY/typing.py \
            typescript:$(find "$C/TypeScript" -name "*.ts" -size +300k -size -900k | head -1) \
            javascript:$(find "$JSMOD" -name "*.js" -size +300k -size -3000k | head -1); do
  l=${pair%%:*}; f=${pair#*:}
  [ -f "$f" ] && $B highlight $l "$f" >> "$out/highlight.txt" 2>&1 && echo >> "$out/highlight.txt"
done
echo done

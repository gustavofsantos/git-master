#!/usr/bin/env bash
# The real repositories the S-B spike measures, as shallow clones.
set -eu
C=${CORPORA:-$HOME/.cache/gm-spikes/corpora}; mkdir -p "$C"; cd "$C"
for r in microsoft/TypeScript django/django vuejs/core git/git rails/rails google/guava nlohmann/json JamesNK/Newtonsoft.Json; do
  d=$(basename "$r"); [ -d "$d" ] || git clone --depth 1 -q "https://github.com/$r" "$d"
done

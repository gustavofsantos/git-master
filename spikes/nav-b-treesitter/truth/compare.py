"""Compares the tags run of tree-sitter with a ground truth.
usage: compare.py <truth.tsv> <tags.tsv> [--show N]
Rows are path<TAB>line<TAB>class<TAB>name. The tags dump has the kind of the
query (definition.function ...), which is mapped to fn or type here; kinds that
are neither (module, macro, constant, ...) are left out of both sides."""
import collections, sys

FN = {"definition.function", "definition.method"}
TYPE = {"definition.class", "definition.interface", "definition.type"}

def load(path, mapped):
    rows = collections.Counter()
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.rstrip("\n")
        if not line:
            continue
        p = line.split("\t")
        if len(p) != 4:
            continue
        rel, ln, kind, name = p
        if mapped:
            kind = "fn" if kind in FN else "type" if kind in TYPE else None
        if kind is None:
            continue
        rows[(rel, int(ln), kind, name)] += 1
    return rows

def main():
    truth, tags = load(sys.argv[1], False), load(sys.argv[2], True)
    show = int(sys.argv[sys.argv.index("--show") + 1]) if "--show" in sys.argv else 8
    # a definition found twice (a Rust method also matches the function pattern) counts once
    tags = collections.Counter({k: 1 for k in tags})
    truth = collections.Counter({k: 1 for k in truth})
    for cls in ("fn", "type", "all"):
        t = {k for k in truth if cls == "all" or k[2] == cls}
        g = {k for k in tags if cls == "all" or k[2] == cls}
        hit = t & g
        print(f"{cls:<5} truth={len(t):>8} tree-sitter={len(g):>8} both={len(hit):>8} "
              f"recall={100*len(hit)/max(len(t),1):6.2f}%  precision={100*len(hit)/max(len(g),1):6.2f}%")
    # in the tree-sitter rows but not in the truth, and the other way round
    for label, a, b in (("missed by tree-sitter (in the truth only)", truth, tags), ("extra in tree-sitter (not in the truth)", tags, truth)):
        diff = sorted(k for k in a if k not in b)
        by_kind = collections.Counter(k[2] for k in diff)
        print(f"\n{label}: {len(diff)}  {dict(by_kind)}")
        for k in diff[:show]:
            print("   ", k)

main()

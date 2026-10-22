"""Ground truth for Python: the definitions the `ast` module finds.
usage: py_truth.py <root> > truth.tsv      rows: path<TAB>line<TAB>class<TAB>name"""
import ast, os, sys
root = sys.argv[1]
out = []
for d, dirs, files in os.walk(root):
    dirs[:] = [x for x in dirs if x != ".git"]
    for f in files:
        if not f.endswith(".py"):
            continue
        p = os.path.join(d, f)
        try:
            if os.path.getsize(p) > 2 * 1024 * 1024:
                continue
            with open(p, "rb") as fh:
                tree = ast.parse(fh.read(), p)
        except Exception:
            continue
        rel = os.path.relpath(p, root)
        for n in ast.walk(tree):
            if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
                out.append(f"{rel}\t{n.lineno}\tfn\t{n.name}")
            elif isinstance(n, ast.ClassDef):
                out.append(f"{rel}\t{n.lineno}\ttype\t{n.name}")
out.sort()
print("\n".join(out))

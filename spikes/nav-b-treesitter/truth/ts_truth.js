// Ground truth for TypeScript and JavaScript: the declarations the TypeScript
// compiler's parser finds. usage: node ts_truth.js <root> <ext,ext,...> > truth.tsv
const ts = require(process.env.TS_MODULE || "typescript");
const fs = require("fs");
const path = require("path");
const root = process.argv[2];
const exts = new Set((process.argv[3] || "ts,tsx").split(",").map((e) => "." + e));
const rows = [];
function walk(dir) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    if (e.isSymbolicLink()) continue;
    const p = path.join(dir, e.name);
    if (e.isDirectory()) { if (e.name !== ".git") walk(p); }
    else if (exts.has(path.extname(e.name))) visitFile(p);
  }
}
function visitFile(p) {
  let text;
  try { if (fs.statSync(p).size > 2 * 1024 * 1024) return; text = fs.readFileSync(p, "utf8"); } catch { return; }
  const kind = p.endsWith(".tsx") || p.endsWith(".jsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
  const sf = ts.createSourceFile(p, text, ts.ScriptTarget.Latest, true, kind);
  const rel = path.relative(root, p);
  const add = (cls, nameNode) => {
    if (!nameNode || !nameNode.getText) return;
    const line = sf.getLineAndCharacterOfPosition(nameNode.getStart(sf)).line + 1;
    rows.push(`${rel}\t${line}\t${cls}\t${nameNode.getText(sf)}`);
  };
  const isFn = (n) => n && (ts.isArrowFunction(n) || ts.isFunctionExpression(n));
  // An explicit stack: the compiler's own test files nest expressions thousands deep.
  const stack = [sf];
  while (stack.length) {
    const n = stack.pop();
    if (ts.isFunctionDeclaration(n) && n.name) add("fn", n.name);
    else if (ts.isMethodDeclaration(n) || ts.isMethodSignature(n)) { if (ts.isIdentifier(n.name)) add("fn", n.name); }
    else if (ts.isClassDeclaration(n) && n.name) add("type", n.name);
    else if (ts.isInterfaceDeclaration(n) || ts.isTypeAliasDeclaration(n) || ts.isEnumDeclaration(n)) add("type", n.name);
    else if (ts.isVariableDeclaration(n) && ts.isIdentifier(n.name) && isFn(n.initializer)) add("fn", n.name);
    else if (ts.isPropertyDeclaration(n) && ts.isIdentifier(n.name) && isFn(n.initializer)) add("fn", n.name);
    ts.forEachChild(n, (c) => { stack.push(c); });
  }
}
walk(root);
rows.sort();
console.log(rows.join("\n"));

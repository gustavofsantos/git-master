# Highlight queries

The tree-sitter queries that color the file viewer (N4, D111). They are the product's own:
edit them here, and keep the tests of `src/syntax.rs` green.

Each file started as the `highlights` query of a grammar crate, all under the MIT license,
and was then corrected for GitMaster:

| Folder | Seeded from | Copyright |
|---|---|---|
| `rust/` | `tree-sitter-rust` 0.24.2 | 2017 Maxim Sokolov |
| `python/` | `tree-sitter-python` 0.25.0 | 2016 Max Brunsfeld |
| `go/` | `tree-sitter-go` 0.25.0 | 2014 Max Brunsfeld |
| `javascript/` | `tree-sitter-javascript` 0.25.0 (`highlights.scm`, and `highlights-jsx.scm` as `jsx.scm`) | 2014 Max Brunsfeld |
| `typescript/` | `tree-sitter-typescript` 0.23.2 | MIT, as declared in the crate manifest |

TypeScript's query only adds to JavaScript's, so `syntax.rs` layers them: JavaScript then
TypeScript, and for TSX JavaScript, JSX, then TypeScript.

Rules for a query:

- A capture name maps to a role by its dotted name (`role_for_capture`). A name with no role
  paints nothing; a test lists the ones that are silent on purpose.
- Where captures overlap, the smaller range wins, and of equal ranges the later pattern.

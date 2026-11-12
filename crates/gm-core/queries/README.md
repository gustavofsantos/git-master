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
| `java/` | `tree-sitter-java` 0.23.5 | MIT, as declared in the crate manifest |
| `c/` | `tree-sitter-c` 0.24.2 | MIT, as declared in the crate manifest |
| `cpp/` | `tree-sitter-cpp` 0.23.4 | MIT, as declared in the crate manifest |
| `ruby/` | `tree-sitter-ruby` 0.23.1 | MIT, as declared in the crate manifest |
| `csharp/` | `tree-sitter-c-sharp` 0.23.5 | MIT, as declared in the crate manifest |
| `json/` | `tree-sitter-json` 0.24.8 | MIT, as declared in the crate manifest |
| `toml/` | `tree-sitter-toml-ng` 0.7.0 | MIT, as declared in the crate manifest |
| `yaml/` | `tree-sitter-yaml` 0.7.2 | MIT, as declared in the crate manifest |
| `bash/` | `tree-sitter-bash` 0.25.1 | MIT, as declared in the crate manifest |
| `html/` | `tree-sitter-html` 0.23.2 | MIT, as declared in the crate manifest |
| `css/` | `tree-sitter-css` 0.25.0 | MIT, as declared in the crate manifest |
| `clojure/` | `tree-sitter-clojure-orchard` 0.2.8 (its literals, comments and quote operators; the rest is written for GitMaster) | CC0 1.0, public domain |
| `lua/` | Written for GitMaster for `tree-sitter-lua` 0.4.1 (D151, D233); not seeded from a grammar crate | GitMaster |

TypeScript's query only adds to JavaScript's, and C++'s to C's, so `syntax.rs` layers them:
JavaScript then TypeScript (for TSX: JavaScript, JSX, then TypeScript), and C then C++.

Rules for a query:

- A capture named `_x` is a helper for a predicate (`#match?`) of its pattern and paints nothing.
- A capture name maps to a role by its dotted name (`role_for_capture`). A name with no role
  paints nothing; a test lists the ones that are silent on purpose.
- Where captures overlap, the smaller range wins, and of equal ranges the later pattern.

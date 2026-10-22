# Spike S-B: tree-sitter for symbols (N5) and syntax highlight (N4) (docs/09)

Measured on 2026-10-22 by the agent, in a cloud container: Linux x86_64, 4 vCPU of an Intel Xeon at 2.8 GHz, 15 GB, rustc 1.99.1, git 2.43, warm caches. **Nothing was measured on macOS.** Raw output is in `results/`; `queries/` holds the query files the numbers depend on; `truth/` holds the ground-truth tools and the comparison script.

## What was tested

- `tree-sitter` 0.27.1 and the grammar crates `rust` 0.24.2, `python` 0.25.0, `go` 0.25.0, `typescript` 0.23.2 (TypeScript and TSX), `javascript` 0.25.0 (first wave), and `java` 0.23.5, `c` 0.24.2, `cpp` 0.23.4, `ruby` 0.23.1, `c-sharp` 0.23.5 (second wave). Each grammar crate ships `tags.scm` and `highlights.scm`; all are compiled into the binary as C (D51), nothing is installed.
- Real code, not synthetic: the 22,604 `.rs` files of a cargo registry (534 MB, 12.1 M lines), the Go standard library (7,117 files), Python's standard library and Django, the TypeScript repository (13,410 `.ts`/`.tsx` files, which includes the compiler's test baselines), Node's bundled JavaScript, and shallow clones of git (C), nlohmann/json (C++), rails (Ruby), guava (Java) and Newtonsoft.Json (C#). `fetch-corpora.sh` clones the repositories.
- A **mixed 100k-file corpus**: hard links of all of the above, with the Rust registry twice: 104,758 parseable files, 1.39 GB, 34.9 M lines.
- Accuracy against a **ground truth from each language's own parser** (`syn` for Rust, `ast` for Python, `go/parser` for Go, the TypeScript compiler API for TypeScript and JavaScript), compared by `(file, line, kind, name)`. Kinds are folded into `fn` (functions and methods) and `type` (struct, enum, trait, class, interface, alias).

## Verdict in one page

1. **Tree-sitter is viable for D45 and N4: fast enough, small, accurate where it is configured well.** A million lines parse in about 9 s of one core. Compiling ten grammars costs about 15 CPU-seconds and 16.6 MB of stripped binary (the five first-wave ones about 5 MB).
2. **Do not use the grammar crates' queries as shipped.** The TypeScript `tags.scm` alone has **35% recall** (it has no class, function, method, enum or type-alias pattern, because it is meant to be layered on the JavaScript one); layered it reaches 87%; with a 3-pattern overlay, **99.1%**. The TypeScript `highlights.scm` is also thin and must be layered on the JavaScript one (5.5 times more captures). The Rust `tags.scm` lists associated types as type definitions (17% of its type rows are `type Item = …;` noise), and the Go one misses `type X = Y` aliases. The product must own its query files (`queries/` has working seeds).
3. **For definitions, a plain tree walk beats the query engine 2.6 to 3 times** with identical accuracy (Rust 18.9 s of CPU against 49.6 s; Go 2.5 s against 7.7 s), and the query engine's cost does not shrink when reference patterns are removed. Parse is the larger part either way.
4. **Indexing 100 thousand mixed files takes 109 s on 4 threads** (about 400 CPU-seconds), and parallelism is linear when files are handed out one at a time. As a low-priority background job on one or two threads that is roughly 5 to 8 minutes, once per blob, with a peak of 42 to 66 MB of memory.
5. **SQLite is enough for the symbol store**: 1.19 M symbols insert in 1.8 s into a 113 MB file; exact and prefix lookups take under 0.1 ms.
6. **Highlighting only the visible lines is cheap** (0.1 to 0.6 ms for 60 lines) and an incremental reparse after an edit is 0.2 to 45 ms. A cold parse of a 1.5 MB file is 0.4 s, so it belongs on a worker thread, with the viewer's 2 MiB cap.
7. **Fluidity (D45's condition) holds on this machine** if the work stays off the UI thread and is throttled. It was not tested against a live UI, and macOS is unmeasured.

## 1. Cost to build and to ship

Clean release builds, one grammar at a time, in their own target directories (`measure.sh`; `results/compile-Linux.txt`). The "none" row is tree-sitter itself plus its Rust dependencies; the rest of each row is the grammar.

| Grammars | wall (s) | user CPU (s) | stripped binary (KB) | binary added (KB) |
|---|---|---|---|---|
| none | 14.9 | 47.1 | 2,407 | |
| rust | 14.9 | 48.3 | 3,525 | 1,118 |
| python | 14.6 | 48.3 | 2,886 | 479 |
| go | 14.6 | 47.3 | 2,642 | 235 |
| typescript (+ TSX) | 16.8 | 50.5 | 5,231 | 2,824 |
| javascript | 14.4 | 47.9 | 2,837 | 430 |
| java | 14.0 | 46.9 | 2,837 | 430 |
| c | 14.4 | 48.0 | 3,024 | 617 |
| cpp | 14.6 | 49.6 | 5,763 | 3,356 |
| ruby | 14.2 | 48.1 | 4,476 | 2,069 |
| c# | 15.0 | 50.4 | 7,617 | 5,210 |
| **first wave** (rust, python, go, typescript, javascript) | 18.2 | 54.4 | 7,432 | 5,025 |
| **all ten** | 18.3 | 62.6 | 19,044 | 16,637 |

Reading: the parsers are mostly data tables, so they compile fast (the 29 MB `parser.c` of C# adds 3 CPU-seconds). The cost that matters is **binary size**, dominated by C# (5.2 MB), C++ (3.4 MB), TypeScript (2.8 MB) and Ruby (2.1 MB). Putting languages behind Cargo features, as the spike does, is easy. The highlights query of a language compiles in 5 to 41 ms (Rust 32 to 41, Go 5, Python 9, JS 25, TS+JS 35); compile it on first use of the language, off the UI thread.

**To do on the Mac:** `cd spikes && cargo fetch && nav-b-treesitter/measure.sh | tee nav-b-treesitter/results/compile-$(uname -s).txt`. The script is portable (bash `time`, `strip`, `wc`). The grammars are plain C built by the `cc` crate, so Command Line Tools should be enough (the GPUI `xcrun metal` problem in `Cargo.toml` is unrelated), but that is unverified.

## 2. Parse and index speed

Single thread, release build, files up to 2 MiB (the viewer's cap). "Errors" is the share of files in which the parser put at least one error node.

| Corpus | Files | Size | Parse | MB/s | lines/s | files/s | Errors |
|---|---|---|---|---|---|---|---|
| Rust (cargo registry) | 22,604 | 534.5 MB | 102 to 113 s | 4.7 to 5.2 | 107k to 119k | 199 to 222 | 7.0% |
| Go standard library | 7,117 | 75.5 MB | 18.5 s | 4.1 | 135k | 384 | 0.7% |
| Python stdlib | 589 | 10.3 MB | 1.7 s | 5.9 | 169k | 338 | 0% |
| Python (Django) | 2,937 | 18.6 MB | 3.4 s | 5.5 | 157k | 869 | 0.03% |
| TypeScript repo (ts, tsx) | 13,410 | 20.8 MB | 4.1 s | 5.0 | 158k | 3,259 | 8.6% |
| JavaScript (Node's modules) | 1,224 | 6.6 MB | 1.6 s | 4.2 | 126k | 788 | 0.1% |
| C (git) | 986 | 11.7 MB | 3.4 s | 3.5 | 132k | 293 | **40%** |
| C++ (nlohmann/json) | 591 | 8.1 MB | 2.7 s | 3.0 | 80k | 220 | **38%** |
| Ruby (rails) | 3,483 | 19.0 MB | 6.2 s | 3.1 | 95k | 566 | 0% |
| Java (guava) | 3,275 | 28.0 MB | 5.9 s | 4.8 | 136k | 557 | 0.4% |
| C# (Newtonsoft.Json) | 951 | 7.0 MB | 2.3 s | 3.0 | 86k | 414 | 6.9% |

These times include running the tags query for the second wave and are therefore slightly higher than parse alone.

**Syntax errors do not mean lost symbols.** Rust has 7% of files with an error node, yet recall against `syn` is 100% for functions and types. The Rust errors are rare constructs the grammar lacks (`safe fn` in extern blocks, `extern type`, trait aliases, `$` in macro matchers, `str![[…]]`); tree-sitter recovers locally. The 40% for C and C++ is a different matter: macro-heavy code is genuinely hard for these grammars, and **symbol quality for C and C++ was not verified** (there is no ground truth here). Do not put C or C++ in the first version of symbols without that check.

**Parallelism.** My first threaded run split the files into static chunks and scaled badly (1.6 times on 4 threads) because the Rust registry puts the huge generated `windows-*` crates together. With files handed out one at a time from a shared counter: 107.3 s, 54.8 s, 27.6 s on 1, 2, 4 threads (**1.96 and 3.9 times**). Keep that in the real indexer.

**The 100k-file index job** (`results/farm100k-*.txt`, 4 threads, definitions and references captured by the shipped queries):

| Step | Wall | CPU |
|---|---|---|
| parse only | 75.6 s | 260 s |
| parse and the shipped tags query | **109 s** (959 files/s, 12.8 MB/s) | read 9 s, parse 261 s, query 128 s |

- 104,709 files, 1.39 GB, 34.9 M lines; 3.47 M definitions (33 per file) and 6.9 M references. 18% of these files have an error node (the mix includes test data, and C and C++).
- Peak memory: **42 MB with one thread, 66 MB with four** (measured on the 13k JavaScript files of the TypeScript repository; the largest file allowed is 2 MiB).
- One thread at low priority needs about 6.6 CPU-minutes for 100 thousand files with the shipped queries; with the plain walk (below) about 5.5. This happens once per blob, so a repository that is indexed once and then changes by a few files costs nothing.
- The query engine is **a third of the CPU**: 54% on top of parse for Rust, 53% for Go, 45% for Python. **Removing the `reference.*` patterns did not reduce it** (Rust: 49.6 s against 50.2 s; farm: 121 s against 128 s, also with `disable_pattern`), so the cost is the engine walking the tree, not the number of patterns.

## 3. The plain tree walk

`src/walk.rs` finds definitions with a depth-first `TreeCursor` walk and a `match` on node kinds (Rust, Python, Go). Same accuracy as the queries, much cheaper (4 threads, CPU seconds of the definitions step):

| | query | walk | |
|---|---|---|---|
| Rust (22.6k files) | 49.6 s | 18.9 s | 2.6 times |
| Go (7.1k files) | 7.7 s | 2.5 s | 3.0 times |
| Python (Django) | 1.4 s | 0.5 s | 2.8 times |

The walk reproduces the query's results exactly (`results/walk-compare.txt`), including the context rules the Rust fix needs (an alias counts only at the top of a file or of a module). A walk is more code per language than a query; TypeScript and JavaScript need patterns with context (an arrow function assigned to a variable, a property that holds a function), so for those a query is the practical choice. A mix is reasonable: a walk for languages whose definitions are plain declarations, a query where they are not.

## 4. Quality of the symbols

Recall and precision against the language's own parser, over every file the ground-truth parser accepts (`results/*-compare.txt`). **Precision is a lower bound**: the ground truth counts declarations only, so symbols the query also finds, correctly, show as "extra".

| Language, query | fn recall | fn precision | type recall | type precision | Notes |
|---|---|---|---|---|---|
| Rust, shipped `tags.scm` | 100.00% | 99.99% | 100.00% | 83.2% | 67.9k "extra" types are **associated types** (`type Item = …;`, `type Output;`) |
| Rust, `queries/rust-defs.scm` or the walk | 100.00% | 99.99% | 97.9% | 100.0% | associated types excluded; misses 7.0k aliases declared inside function bodies (typenum's generated tests) |
| Python (stdlib 16.9k, Django 44k) | 100.00% | 100.00% | 100.00% | 99.99% | exact |
| Go, shipped | 99.99% | 99.99% | 97.7% | 99.99% | misses `type X = Y` aliases (their own node type) and 11 functions in invalid test data |
| Go, `queries/go-overlay.scm` or the walk | 99.99% | 99.99% | **99.95%** | 99.99% | the query and the walk give identical results (`results/go-overlay-compare.txt`) |
| TypeScript, **shipped `tags.scm` alone** | 34.9% | 95.9% | 35.1% | 99.9% | no pattern for classes, functions, methods, enums, aliases |
| TypeScript, JavaScript + TypeScript tags | 98.2% | 87.9% | 71.2% | 99.2% | still no enums or type aliases |
| TypeScript, `queries/typescript-overlay-all.scm` | **99.0%** | 87.9% | **99.1%** | 99.3% | adds enum, type alias, function-valued class fields |
| JavaScript (Node's modules), shipped | 99.9% | 75.0% | 100.0% | 91.3% | 2.3k "extra" are assignments and object-literal functions the truth does not count |

What the TypeScript "extra" definitions are (5,883, classified from their source line): methods that are accessors (`get client()`, 2,517) or constructors (399), assignments of functions (`this.toPath = () => …`, about 1,830), named function expressions (`(function walk(node) …)`, about 600), plus class expressions and type literals. They are real names a developer would search for, so the lower precision is the truth being narrower, not errors. The 665 misses are nearly all in the compiler's test baselines and testdata (567) and in `.d.ts` files (98).

**Second wave**: not verified. Java and Ruby parse cleanly (0.4% and 0% error files); C, C++ and C# do not (40%, 38%, 6.9%).

## 5. Highlighting (N4)

`results/highlight.txt`, one thread, `QueryCursor::set_byte_range` over the visible lines only:

| File | Cold parse | Reparse after a 1-char edit | 60 visible lines | Whole file |
|---|---|---|---|---|
| Rust, 1.56 MB (`tables.rs`) | 424 to 620 ms | 40 to 43 ms | 0.36 to 0.60 ms | 229 ms |
| Rust, 1.29 MB (`windows-sys`) | 275 ms | 33 ms | 1.3 ms | |
| Rust, 93 KB | 23 ms | 1.6 ms | | |
| Go, 59 KB | 7.7 ms | 0.6 ms | 0.1 ms | 3 ms |
| Python, 134 KB | 17.8 ms | 1.6 ms | 0.2 to 0.3 ms | 10.5 ms |
| TypeScript, 367 KB | 58 ms | 0.2 ms | 0.16 to 0.20 ms (shipped) | 26 ms |
| TypeScript, layered JS + TS highlights | | | 0.31 ms, **325 captures** (shipped: 59) | 46 ms, 56k captures |
| JavaScript, **11.5 MB minified** (7,560 lines) | **3.6 s** | 49 ms | **13 to 183 ms**, 266k captures at the top | 2.9 s |

- The viewer's 2 MiB cap (N2) bounds the cold parse to well under a second on this machine, and highlighting must run off the UI thread anyway. Pathological lines (minified bundles) make even 60 lines cost 183 ms: skip or truncate highlight for lines past a length (the viewer already clips them) and set a parse timeout (`Parser::set_timeout_micros` or the progress callback).
- Highlight capture names in the five `highlights.scm` files (26 distinct, dotted hierarchies): `attribute, comment, comment.documentation, constant, constant.builtin, constructor, embedded, escape, function, function.builtin, function.macro, function.method, keyword, label, number, operator, property, punctuation.bracket, punctuation.delimiter, punctuation.special, string, string.special, type, type.builtin, variable, variable.builtin, variable.parameter`. A theme would map these to about ten roles by their first segment.
- The quality of the highlight (does it look right) was **not judged**: there is no window here.
- `queries/typescript-highlights-combined.scm` is JS then TS highlights; it compiles on the TypeScript grammar.

## 6. Where to keep the index

`src/store.rs`, 1.19 M symbols from 39,969 blobs (the real dumps), SQLite with WAL, one table of blobs and one of symbols (`results/store.txt`):

| | |
|---|---|
| insert, one transaction | 1.82 s (652k rows/s) |
| build two indexes after the insert | 1.57 s |
| file size | **113 MB** (100 bytes per symbol, name stored twice and two indexes) |
| exact name (indexed) | 0.040 ms |
| name prefix of 3 letters (indexed range) | 0.073 ms |
| contains (`LIKE %x%`, no index, 1.19 M rows) | 9.6 ms |
| is this blob indexed (by id) | 0.006 ms |
| outline of one file | 0.021 ms |
| fuzzy scan over 336k distinct names, naive | 25 to 34 ms |

A normalized schema (a table of distinct names, 28% of the rows here) would roughly halve the file; at the farm's 3.47 M definitions that is on the order of 170 to 350 MB. Fuzzy search over names should use the byte matcher of N1 (S-C: 16 to 25 ms for a million on four threads), not SQLite.

Where: both places the docs allowed respect principle 5. **Recommendation: `.git/gitmaster/symbols.db`** (the Git common directory, so every worktree of a repository shares it), next to the `state.db` that workflows already keep there, keyed by `(blob oid, language, query version)`. The index is derived and may be deleted; it goes away with the repository; a worktree that is not writable falls back to memory only. The alternative, one file in the app data directory shared by all repositories, would reuse blobs across clones but needs its own garbage collection, and a path-based key for modified files.

## 7. Things the spike did not settle

- **macOS.** Compile time, binary size, parse speed and the rest are Linux-only. Run `measure.sh` and `run.sh` there before relying on the numbers.
- **Fluidity with a live UI.** The claim that background indexing leaves the app smooth rests on thread counts and CPU time, not on a measured frame rate. N5's gate should include "scroll and type while indexing".
- **C, C++, C#, and the rest of the second wave**: parse speed and cost are measured; the symbols are not verified.
- **Highlight look.** No window. Capture names are listed; the mapping to the theme is design work for N4.
- **Cold caches**, very large single files beyond 2 MiB, files with BOMs or odd encodings, Windows line endings, and Unicode identifiers were not tested separately.
- The Rust ground truth skips files `syn` cannot parse; the Go truth skips files `go/parser` rejects. Recall over invalid files is therefore not measured.
- `tree-sitter` 0.27 and the grammar versions will move; the ABI range here is 13 to 15 and the TypeScript and C++ grammars are at ABI 14.

## Reproduce

```bash
export PATH=$HOME/.cargo/bin:$PATH          # rustc 1.99 or newer (rustup update stable)
cd spikes && cargo fetch
nav-b-treesitter/fetch-corpora.sh           # clones into ~/.cache/gm-spikes/corpora
export CARGO_TARGET_DIR=$HOME/.cache/gm-spikes/target-b
cargo build --release -p spike-nav-b-treesitter --features all,store -p spike-nav-b-truth-rust

nav-b-treesitter/measure.sh | tee nav-b-treesitter/results/compile-$(uname -s).txt   # section 1
nav-b-treesitter/run.sh                      # sections 2, 4, 5 (about 25 minutes)
```

Commands behind the other tables (set `B=$CARGO_TARGET_DIR/release/spike-nav-b-treesitter`):

```bash
# thread scaling (section 2)
for t in 1 2 4; do $B parse rust ~/.cargo/registry/src --threads $t; done
# the 100k-file mixed corpus: hard links of every corpus, the Rust registry twice
F=$HOME/.cache/gm-spikes/farm; mkdir -p $F
cp -al ~/.cargo/registry/src $F/rust1; cp -al ~/.cargo/registry/src $F/rust2
cp -al /usr/local/go/src $F/go; cp -al /usr/lib/python3.13 $F/pystd
for d in TypeScript django rails guava git json Newtonsoft.Json core; do cp -al ~/.cache/gm-spikes/corpora/$d $F/$d; done
cp -al /opt/node22/lib/node_modules $F/node
$B tags all $F --threads 4                   # section 2, the index job
# the plain walk (section 3): SPIKE_WALK=1; a hand-written definitions query: --tags-file
SPIKE_WALK=1 $B tags rust ~/.cargo/registry/src --threads 4 --dump /tmp/rust-walk.tags
python3 -I truth/compare.py /tmp/rust.truth /tmp/rust-walk.tags
# layered highlights (section 5)
SPIKE_HIGHLIGHTS=queries/typescript-highlights-combined.scm $B highlight typescript some.ts
# the symbol store (section 6)
$B store /tmp/symbols.db /tmp/rust-walk.tags /tmp/go-walk.tags /tmp/python-walk.tags /tmp/ts-overlay-all.tags /tmp/js.tags
```

Ground-truth tools: `$CARGO_TARGET_DIR/release/spike-nav-b-truth-rust <root>`, `python3 -I truth/py_truth.py <root>`, `(cd truth/go_truth && go run . <root>)`, `TS_MODULE=<path to the typescript npm module> node truth/ts_truth.js <root> ts,tsx`. All write `path<TAB>line<TAB>class<TAB>name`.

Environment variables the harness reads: `SPIKE_WALK` (find definitions by a tree walk), `SPIKE_DEFS_ONLY` (turn reference patterns off), `SPIKE_ERRORS` (print where the first syntax error of each file is), `SPIKE_HIGHLIGHTS` (a file that replaces the highlights query).

//! Spike S-B (design/code-navigation.md): tree-sitter for the symbols (N5) and the highlight (N4).
//!
//!   spike-nav-b-treesitter info
//!   spike-nav-b-treesitter parse <lang> <root> [--threads N] [--max-bytes N]
//!   spike-nav-b-treesitter tags  <lang> <root> [--threads N] [--dump FILE]
//!   (SPIKE_WALK=1 finds definitions by a plain tree walk instead of a query: rust, python, go)
//!   (SPIKE_DEFS_ONLY=1 turns the reference patterns of a tags query off)
//!   spike-nav-b-treesitter highlight <lang> <file> [--lines N]
//!   spike-nav-b-treesitter store <db> <tags.tsv>...        (feature `store`)
//!
//! `<lang>` is `all` to take every enabled grammar and pick by extension.

mod langs;
#[cfg(feature = "store")]
mod store;
mod walk;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

use langs::Lang;

/// Files larger than this are skipped, as the viewer would cut them (N2).
const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let threads: usize = flag("--threads").map_or(1, |t| t.parse().unwrap());
    let max_bytes: u64 = flag("--max-bytes").map_or(DEFAULT_MAX_BYTES, |t| t.parse().unwrap());
    match args.first().map(String::as_str) {
        Some("info") => info(),
        Some("parse") => scan(
            &args[1],
            Path::new(&args[2]),
            threads,
            max_bytes,
            false,
            None,
            None,
        ),
        Some("tags") => scan(
            &args[1],
            Path::new(&args[2]),
            threads,
            max_bytes,
            true,
            flag("--dump"),
            flag("--tags-file"),
        ),
        #[cfg(feature = "store")]
        Some("store") => store::run(&args[1..]),
        Some("highlight") => highlight(
            &args[1],
            Path::new(&args[2]),
            flag("--lines").map_or(60, |l| l.parse().unwrap()),
        ),
        _ => eprintln!("usage: see the header of src/main.rs"),
    }
}

fn info() {
    println!(
        "tree-sitter ABI {}..={}",
        tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION,
        tree_sitter::LANGUAGE_VERSION
    );
    for l in langs::all() {
        println!(
            "{:<11} abi={} node_kinds={} exts={:?}",
            l.name,
            l.language.abi_version(),
            l.language.node_kind_count(),
            l.exts
        );
    }
}

fn walk(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut names: Vec<_> = entries.flatten().collect();
    names.sort_by_key(|e| e.file_name());
    for e in names {
        let Ok(t) = e.file_type() else { continue };
        let p = e.path();
        if t.is_symlink() {
            continue;
        }
        if t.is_dir() {
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            walk(&p, out);
        } else if t.is_file() {
            out.push(p);
        }
    }
}

fn pick<'a>(langs: &'a [Lang], want: &str, path: &Path) -> Option<&'a Lang> {
    let ext = path.extension()?.to_str()?;
    langs.iter().find(|l| {
        (want == "all" || l.name == want || (want == "typescript" && l.name == "tsx"))
            && l.exts.contains(&ext)
    })
}

#[derive(Default, Clone)]
struct Stats {
    files: usize,
    bytes: u64,
    lines: u64,
    errors: usize,
    skipped_big: usize,
    read: Duration,
    parse: Duration,
    query: Duration,
    defs: BTreeMap<String, usize>,
    refs: usize,
    rows: Vec<String>,
}

impl Stats {
    fn merge(&mut self, o: Stats) {
        self.files += o.files;
        self.bytes += o.bytes;
        self.lines += o.lines;
        self.errors += o.errors;
        self.skipped_big += o.skipped_big;
        self.read += o.read;
        self.parse += o.parse;
        self.query += o.query;
        self.refs += o.refs;
        for (k, v) in o.defs {
            *self.defs.entry(k).or_default() += v;
        }
        self.rows.extend(o.rows);
    }
}

fn work(
    files: &[&PathBuf],
    next: &std::sync::atomic::AtomicUsize,
    want: &str,
    with_tags: bool,
    dump: bool,
    max_bytes: u64,
    root: &Path,
    tags_override: Option<&str>,
) -> Stats {
    let langs = langs::all();
    let mut parsers: BTreeMap<&str, (Parser, Option<Query>)> = BTreeMap::new();
    let mut s = Stats::default();
    let mut cursor = QueryCursor::new();
    // Files are taken one at a time from a shared counter, so a thread that meets a
    // huge file does not hold the others back (a static split by directory did).
    loop {
        let at = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some(path) = files.get(at) else { break };
        let Some(lang) = pick(&langs, want, path) else {
            continue;
        };
        let entry = parsers.entry(lang.name).or_insert_with(|| {
            let mut p = Parser::new();
            p.set_language(&lang.language).unwrap();
            let q = with_tags.then(|| {
                let text = tags_override.unwrap_or(lang.tags);
                let mut q = Query::new(&lang.language, text).expect("tags query");
                if std::env::var_os("SPIKE_DEFS_ONLY").is_some() {
                    // The symbol index needs the definitions; references are found with
                    // `git grep -w` (D45). Patterns that capture a reference are turned off.
                    for i in 0..q.pattern_count() {
                        let (a, b) = (q.start_byte_for_pattern(i), q.end_byte_for_pattern(i));
                        if text[a..b].contains("@reference.") {
                            q.disable_pattern(i);
                        }
                    }
                }
                q
            });
            (p, q)
        });
        let t = Instant::now();
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        if meta.len() > max_bytes {
            s.skipped_big += 1;
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        s.read += t.elapsed();
        let t = Instant::now();
        let Some(tree) = entry.0.parse(&bytes, None) else {
            continue;
        };
        s.parse += t.elapsed();
        s.files += 1;
        s.bytes += bytes.len() as u64;
        s.lines += bytes.iter().filter(|&&b| b == b'\n').count() as u64;
        if tree.root_node().has_error() {
            s.errors += 1;
            if std::env::var_os("SPIKE_ERRORS").is_some() {
                // The first node Tree-sitter could not place: where and what it saw.
                let mut cursor = tree.walk();
                let mut found = None;
                'search: loop {
                    let n = cursor.node();
                    if n.is_error() || n.is_missing() {
                        found = Some(n);
                        break 'search;
                    }
                    if n.has_error() && cursor.goto_first_child() {
                        continue;
                    }
                    while !cursor.goto_next_sibling() {
                        if !cursor.goto_parent() {
                            break 'search;
                        }
                    }
                }
                if let Some(n) = found {
                    let row = n.start_position().row;
                    let line = String::from_utf8_lossy(
                        bytes.split(|&b| b == b'\n').nth(row).unwrap_or_default(),
                    );
                    let rel = path.strip_prefix(root).unwrap_or(path);
                    eprintln!(
                        "ERR {}:{}: {} | {}",
                        rel.display(),
                        row + 1,
                        if n.is_missing() { "missing" } else { "error" },
                        line.trim()
                    );
                }
            }
        }
        if with_tags && std::env::var_os("SPIKE_WALK").is_some() {
            let t = Instant::now();
            walk::definitions(lang.name, tree.root_node(), |kind, name| {
                *s.defs.entry(kind.to_owned()).or_default() += 1;
                if dump {
                    let rel = path.strip_prefix(root).unwrap_or(path);
                    let text = String::from_utf8_lossy(&bytes[name.byte_range()]);
                    s.rows.push(format!(
                        "{}\t{}\t{}\t{}",
                        rel.display(),
                        name.start_position().row + 1,
                        kind,
                        text
                    ));
                }
            });
            s.query += t.elapsed();
        } else if let Some(query) = &entry.1 {
            let t = Instant::now();
            let names = query.capture_names();
            let mut matches = cursor.matches(query, tree.root_node(), bytes.as_slice());
            while let Some(m) = matches.next() {
                let mut kind: Option<&str> = None;
                let mut name: Option<(usize, &[u8])> = None;
                for c in m.captures() {
                    let n = names[c.index as usize];
                    if n == "name" {
                        name = Some((c.node.start_position().row + 1, &bytes[c.node.byte_range()]));
                    } else if n.starts_with("definition.") || n.starts_with("reference.") {
                        kind = Some(n);
                    }
                }
                let Some(kind) = kind else { continue };
                if kind.starts_with("reference.") {
                    s.refs += 1;
                    continue;
                }
                *s.defs.entry(kind.to_owned()).or_default() += 1;
                if dump {
                    if let Some((row, text)) = name {
                        let rel = path.strip_prefix(root).unwrap_or(path);
                        s.rows.push(format!(
                            "{}\t{}\t{}\t{}",
                            rel.display(),
                            row,
                            kind,
                            String::from_utf8_lossy(text)
                        ));
                    }
                }
            }
            s.query += t.elapsed();
        }
    }
    s
}

fn scan(
    want: &str,
    root: &Path,
    threads: usize,
    max_bytes: u64,
    with_tags: bool,
    dump: Option<String>,
    tags_file: Option<String>,
) {
    let tags_text = tags_file.map(|f| std::fs::read_to_string(f).expect("tags file"));
    let tags_override = tags_text.as_deref();
    let mut all = Vec::new();
    let t = Instant::now();
    walk(root, &mut all);
    let walked = t.elapsed();
    let langs = langs::all();
    let files: Vec<&PathBuf> = all
        .iter()
        .filter(|p| pick(&langs, want, p).is_some())
        .collect();
    let started = Instant::now();
    let want_rows = dump.is_some();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let total = if threads <= 1 {
        work(
            &files,
            &next,
            want,
            with_tags,
            want_rows,
            max_bytes,
            root,
            tags_override,
        )
    } else {
        let mut total = Stats::default();
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    scope.spawn(|| {
                        work(
                            &files,
                            &next,
                            want,
                            with_tags,
                            want_rows,
                            max_bytes,
                            root,
                            tags_override,
                        )
                    })
                })
                .collect();
            for h in handles {
                total.merge(h.join().unwrap());
            }
        });
        total
    };
    let wall = started.elapsed();
    let mb = total.bytes as f64 / 1_048_576.0;
    println!("lang={want} root={} threads={threads}", root.display());
    println!(
        "walk: {} entries, {} matching files in {:.2}s",
        all.len(),
        files.len(),
        walked.as_secs_f64()
    );
    println!(
        "parsed {} files ({:.1} MB, {} lines), {} with syntax errors ({:.2}%), {} skipped as too big",
        total.files,
        mb,
        total.lines,
        total.errors,
        100.0 * total.errors as f64 / total.files.max(1) as f64,
        total.skipped_big
    );
    println!(
        "wall {:.2}s  =>  {:.0} files/s, {:.1} MB/s, {:.0} lines/s  (cpu: read {:.2}s, parse {:.2}s, query {:.2}s)",
        wall.as_secs_f64(),
        total.files as f64 / wall.as_secs_f64(),
        mb / wall.as_secs_f64(),
        total.lines as f64 / wall.as_secs_f64(),
        total.read.as_secs_f64(),
        total.parse.as_secs_f64(),
        total.query.as_secs_f64()
    );
    if with_tags {
        let defs: usize = total.defs.values().sum();
        println!(
            "definitions {} ({:.1} per file), references {}",
            defs,
            defs as f64 / total.files.max(1) as f64,
            total.refs
        );
        for (k, v) in &total.defs {
            println!("  {k:<28} {v}");
        }
    }
    if let Some(file) = dump {
        let mut rows = total.rows;
        rows.sort();
        std::fs::write(&file, rows.join("\n") + "\n").unwrap();
        println!("dumped {} rows to {file}", rows.len());
    }
}

fn highlight(want: &str, file: &Path, visible_lines: usize) {
    let langs = langs::all();
    let Some(lang) = pick(&langs, want, file) else {
        eprintln!("no grammar for {}", file.display());
        return;
    };
    let bytes = std::fs::read(file).unwrap();
    let mut parser = Parser::new();
    parser.set_language(&lang.language).unwrap();
    let q_started = Instant::now();
    let highlights_text = std::env::var("SPIKE_HIGHLIGHTS")
        .map(|f| std::fs::read_to_string(f).expect("highlights file"))
        .unwrap_or_else(|_| lang.highlights.to_owned());
    let query = Query::new(&lang.language, &highlights_text).expect("highlights query");
    let compile = q_started.elapsed();
    let lines = bytes.iter().filter(|&&b| b == b'\n').count();
    // Cold parse, then a reparse after a one-character edit in the middle with the old tree (incremental).
    let t = Instant::now();
    let mut tree = parser.parse(&bytes, None).unwrap();
    let full = t.elapsed();
    let mid = bytes.len() / 2;
    let mut edited = bytes.clone();
    edited.insert(mid, b' ');
    let at = tree_sitter::Point {
        row: bytes[..mid].iter().filter(|&&b| b == b'\n').count(),
        column: 0,
    };
    tree.edit(&tree_sitter::InputEdit {
        start_byte: mid,
        old_end_byte: mid,
        new_end_byte: mid + 1,
        start_position: at,
        old_end_position: at,
        new_end_position: tree_sitter::Point {
            row: at.row,
            column: at.column + 1,
        },
    });
    let t = Instant::now();
    let tree2 = parser.parse(&edited, Some(&tree)).unwrap();
    let incremental = t.elapsed();
    // The highlight of the visible lines only, at the top, the middle and the end.
    let line_start = |n: usize| -> usize {
        let mut seen = 0;
        for (i, b) in bytes.iter().enumerate() {
            if seen == n {
                return i;
            }
            if *b == b'\n' {
                seen += 1;
            }
        }
        bytes.len()
    };
    println!(
        "file {} ({} bytes, {} lines) with {}",
        file.display(),
        bytes.len(),
        lines,
        lang.name
    );
    println!(
        "highlights query compiled in {:.2} ms",
        compile.as_secs_f64() * 1000.0
    );
    println!(
        "cold parse {:.2} ms; reparse after a one-character edit {:.2} ms",
        full.as_secs_f64() * 1000.0,
        incremental.as_secs_f64() * 1000.0
    );
    let mut cursor = QueryCursor::new();
    for (label, first) in [
        ("top", 0),
        ("middle", lines / 2),
        ("end", lines.saturating_sub(visible_lines)),
    ] {
        let a = line_start(first);
        let b = line_start(first + visible_lines);
        cursor.set_byte_range(a..b);
        let t = Instant::now();
        let mut n = 0usize;
        let mut caps = cursor.captures(&query, tree2.root_node(), edited.as_slice());
        while let Some((m, _)) = caps.next() {
            n += m.captures().len();
        }
        println!(
            "highlight {visible_lines} lines at the {label}: {:.3} ms, {n} captures",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
    cursor.set_byte_range(0..edited.len());
    let t = Instant::now();
    let mut n = 0usize;
    let mut caps = cursor.captures(&query, tree2.root_node(), edited.as_slice());
    while let Some((m, _)) = caps.next() {
        n += m.captures().len();
    }
    println!(
        "highlight the whole file: {:.2} ms, {n} captures",
        t.elapsed().as_secs_f64() * 1000.0
    );
}

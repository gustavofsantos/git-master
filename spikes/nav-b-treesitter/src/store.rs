//! The symbol store benchmark (spike S-B, docs/09 "where to keep the index").
//!
//! Reads the tags dumps (`path<TAB>row<TAB>kind<TAB>name`) and stores them in
//! SQLite the way N5 would: one row per symbol, keyed by the blob it came from,
//! plus a table of the blobs already indexed. The blob id here is a hash of the
//! path, standing in for a Git object id.

use std::collections::HashMap;
use std::time::Instant;

use rusqlite::{Connection, params};

pub fn run(args: &[String]) {
    let db = &args[0];
    let _ = std::fs::remove_file(db);
    let _ = std::fs::remove_file(format!("{db}-wal"));
    let _ = std::fs::remove_file(format!("{db}-shm"));
    let mut rows: Vec<(String, u32, String, String)> = Vec::new();
    for file in &args[1..] {
        for line in std::fs::read_to_string(file).unwrap().lines() {
            let p: Vec<&str> = line.split('\t').collect();
            if let [path, row, kind, name] = p[..] {
                rows.push((
                    path.to_owned(),
                    row.parse().unwrap_or(0),
                    kind.to_owned(),
                    name.to_owned(),
                ));
            }
        }
    }
    let mut blobs: HashMap<String, i64> = HashMap::new();
    for (path, ..) in &rows {
        let next = blobs.len() as i64 + 1;
        blobs.entry(path.clone()).or_insert(next);
    }
    println!("{} symbols from {} blobs", rows.len(), blobs.len());

    let conn = Connection::open(db).unwrap();
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
        .unwrap();
    conn.execute_batch(
        "CREATE TABLE blob(id INTEGER PRIMARY KEY, oid TEXT NOT NULL UNIQUE, lang TEXT NOT NULL, query_version INTEGER NOT NULL, symbols INTEGER NOT NULL);
         CREATE TABLE symbol(blob INTEGER NOT NULL, name TEXT NOT NULL, lower TEXT NOT NULL, kind TEXT NOT NULL, row INTEGER NOT NULL);",
    )
    .unwrap();

    // Insert in one transaction, as an indexer thread would do per batch of blobs.
    let t = Instant::now();
    {
        let tx = conn.unchecked_transaction().unwrap();
        {
            let mut ins_blob = tx.prepare("INSERT INTO blob(id, oid, lang, query_version, symbols) VALUES (?1, ?2, ?3, 1, 0)").unwrap();
            for (path, id) in &blobs {
                ins_blob
                    .execute(params![id, format!("{:040x}", fnv(path)), "x"])
                    .unwrap();
            }
            let mut ins = tx
                .prepare(
                    "INSERT INTO symbol(blob, name, lower, kind, row) VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .unwrap();
            for (path, row, kind, name) in &rows {
                ins.execute(params![blobs[path], name, name.to_lowercase(), kind, row])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
    }
    let insert = t.elapsed();
    let t = Instant::now();
    conn.execute_batch(
        "CREATE INDEX symbol_lower ON symbol(lower); CREATE INDEX symbol_blob ON symbol(blob);",
    )
    .unwrap();
    let index = t.elapsed();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    let size = std::fs::metadata(db).unwrap().len();
    println!(
        "insert {:.2}s ({:.0} rows/s), build indexes {:.2}s, file {:.1} MB ({:.0} bytes per symbol)",
        insert.as_secs_f64(),
        rows.len() as f64 / insert.as_secs_f64(),
        index.as_secs_f64(),
        size as f64 / 1_048_576.0,
        size as f64 / rows.len() as f64
    );

    // Lookups, as the search box would make them.
    let names: Vec<&str> = rows
        .iter()
        .step_by(rows.len() / 200 + 1)
        .map(|r| r.3.as_str())
        .collect();
    let time = |label: &str, f: &mut dyn FnMut() -> usize| {
        let t = Instant::now();
        let n = f();
        println!(
            "{label:<44} {:>8.3} ms  ({n} rows)",
            t.elapsed().as_secs_f64() * 1000.0 / names.len() as f64
        );
    };
    time("exact name (indexed, per query)", &mut || {
        let mut q = conn
            .prepare_cached("SELECT blob, row FROM symbol WHERE lower = ?1 LIMIT 50")
            .unwrap();
        names
            .iter()
            .map(|n| {
                q.query_map([n.to_lowercase()], |r| r.get::<_, i64>(0))
                    .unwrap()
                    .count()
            })
            .sum()
    });
    time(
        "prefix of 3 letters (indexed range, per query)",
        &mut || {
            let mut q = conn
                .prepare_cached(
                    "SELECT blob, row FROM symbol WHERE lower >= ?1 AND lower < ?2 LIMIT 50",
                )
                .unwrap();
            names
                .iter()
                .map(|n| {
                    let lo: String = n.to_lowercase().chars().take(3).collect();
                    let hi = format!("{lo}\u{10ffff}");
                    q.query_map([lo, hi], |r| r.get::<_, i64>(0))
                        .unwrap()
                        .count()
                })
                .sum()
        },
    );
    time("contains, no index (LIKE %x%, per query)", &mut || {
        let mut q = conn
            .prepare_cached("SELECT blob, row FROM symbol WHERE lower LIKE ?1 LIMIT 50")
            .unwrap();
        names
            .iter()
            .take(20)
            .map(|n| {
                q.query_map([format!("%{}%", n.to_lowercase())], |r| r.get::<_, i64>(0))
                    .unwrap()
                    .count()
            })
            .sum()
    });
    time("is this blob indexed? (by oid, per lookup)", &mut || {
        let mut q = conn
            .prepare_cached("SELECT id FROM blob WHERE oid = ?1")
            .unwrap();
        blobs
            .keys()
            .take(names.len())
            .map(|p| {
                q.query_map([format!("{:040x}", fnv(p))], |r| r.get::<_, i64>(0))
                    .unwrap()
                    .count()
            })
            .sum()
    });
    time("outline of one file (by blob, per query)", &mut || {
        let mut q = conn
            .prepare_cached("SELECT name, row FROM symbol WHERE blob = ?1 ORDER BY row")
            .unwrap();
        blobs
            .values()
            .take(names.len())
            .map(|b| q.query_map([b], |r| r.get::<_, String>(0)).unwrap().count())
            .sum()
    });
    // The fuzzy search over every distinct name, in memory, as the path matcher of N1 does.
    let t = Instant::now();
    let mut distinct: Vec<String> = rows.iter().map(|r| r.3.to_lowercase()).collect();
    distinct.sort_unstable();
    distinct.dedup();
    println!(
        "distinct names: {} (collected and sorted in {:.2}s)",
        distinct.len(),
        t.elapsed().as_secs_f64()
    );
    for query in ["newf", "hdl", "parseexpr", "x"] {
        let q = query.as_bytes();
        let t = Instant::now();
        let hits = distinct
            .iter()
            .filter(|name| {
                let mut i = 0;
                for &b in name.as_bytes() {
                    if i < q.len() && b == q[i] {
                        i += 1;
                    }
                }
                i == q.len()
            })
            .count();
        println!(
            "fuzzy scan over all names for {query:?}: {:.2} ms ({hits} hits)",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

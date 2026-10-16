//! Spike C: restack a branch stack entirely in memory.
//!
//! Engines under comparison:
//!   * `lg`  - libgit2 `Repository::rebase` with `RebaseOptions::inmemory(true)`.
//!   * `mt`  - `git merge-tree --write-tree --merge-base=<parent>` per commit,
//!             then `git commit-tree` (subprocess, git >= 2.40 for --merge-base).
//!   * `ref` - `git rebase --onto` on a second, identical repository. This is
//!             the ground truth the engines are compared against.
//!
//! Scratch data lives under $GM_SPIKE_TMP (default ~/.cache/gm-spikes/tmp).
#![allow(dead_code)]

use git2::{ErrorCode, Index, IndexEntry, IndexTime, Oid, RebaseOptions, Repository, Signature};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

const SKIP_APPLIED: bool = true;
const ID_NAME: &str = "Spike";
const ID_MAIL: &str = "spike@example.invalid";

// ---------------------------------------------------------------- data model

#[derive(Clone, Debug)]
pub struct Edge {
    /// Branch to replay.
    pub branch: String,
    /// Rev of the old parent tip (the recorded stackParentBase).
    pub upstream: String,
    /// Branch name (already restacked) or `main` that becomes the new base.
    pub onto: String,
}

fn e(branch: &str, upstream: &str, onto: &str) -> Edge {
    Edge { branch: branch.into(), upstream: upstream.into(), onto: onto.into() }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Side {
    pub path: String,
    pub base: Option<Vec<u8>>,
    pub ours: Option<Vec<u8>>,
    pub theirs: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    pub branch: String,
    pub at: String,
    pub sides: Vec<Side>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// branch -> resulting commit sha (in the engine's repository).
    Done { tips: BTreeMap<String, String>, resolved: usize },
    Stopped(Conflict),
    Error(String),
}

/// Given a conflict, return path -> resolved bytes.
pub type Resolver = fn(&Conflict) -> Vec<(String, Vec<u8>)>;

// ---------------------------------------------------------------- git helpers

fn spike_tmp() -> PathBuf {
    let p = match std::env::var("GM_SPIKE_TMP") {
        Ok(v) => PathBuf::from(v),
        Err(_) => PathBuf::from(std::env::var("HOME").expect("HOME")).join(".cache/gm-spikes/tmp"),
    };
    fs::create_dir_all(&p).expect("create GM_SPIKE_TMP");
    p
}

fn new_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("c-rebase-")
        .tempdir_in(spike_tmp())
        .expect("tempdir")
}

fn git_out(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .env("GIT_AUTHOR_NAME", ID_NAME)
        .env("GIT_AUTHOR_EMAIL", ID_MAIL)
        .env("GIT_COMMITTER_NAME", ID_NAME)
        .env("GIT_COMMITTER_EMAIL", ID_MAIL);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().expect("spawn git")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = git_out(dir, args, &[]);
    assert!(o.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim_end().to_string()
}

fn git_bytes(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let o = git_out(dir, args, &[]);
    o.status.success().then_some(o.stdout)
}

fn rev(dir: &Path, r: &str) -> String {
    git(dir, &["rev-parse", "--verify", &format!("{r}^{{commit}}")])
}

fn tree_of(dir: &Path, sha: &str) -> String {
    git(dir, &["rev-parse", &format!("{sha}^{{tree}}")])
}

fn count_of(dir: &Path, sha: &str) -> usize {
    git(dir, &["rev-list", "--count", sha]).parse().unwrap()
}

// ---------------------------------------------------------------- repo setup

fn init(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "core.hooksPath", "/dev/null"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn put(dir: &Path, path: &str, bytes: impl AsRef<[u8]>) {
    let p = dir.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn commit(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--no-verify", "-m", msg]);
}

fn branch_from(dir: &Path, name: &str, from: &str) {
    git(dir, &["checkout", "-q", "-b", name, from]);
}

fn co(dir: &Path, name: &str) {
    git(dir, &["checkout", "-q", name]);
}

fn tag_old(dir: &Path, branch: &str) {
    git(dir, &["tag", &format!("oldtip-{branch}"), branch]);
}

/// Deterministic binary payload.
fn bin_bytes(seed: u8) -> Vec<u8> {
    (0..1024u32).map(|i| (i as u8).wrapping_mul(seed).wrapping_add(seed)).collect()
}

// ---------------------------------------------------------------- engine: libgit2 in-memory

fn lg_conflict(repo: &Repository, idx: &Index, branch: &str) -> Conflict {
    let mut sides = Vec::new();
    for c in idx.conflicts().unwrap() {
        let c = c.unwrap();
        let entry = c
            .our
            .as_ref()
            .or(c.their.as_ref())
            .or(c.ancestor.as_ref())
            .unwrap();
        let path = String::from_utf8_lossy(&entry.path).into_owned();
        let read = |e: &Option<IndexEntry>| {
            e.as_ref().map(|e| repo.find_blob(e.id).unwrap().content().to_vec())
        };
        sides.push(Side { path, base: read(&c.ancestor), ours: read(&c.our), theirs: read(&c.their) });
    }
    sides.sort_by(|a, b| a.path.cmp(&b.path));
    Conflict { branch: branch.into(), at: "(libgit2 in-progress op)".into(), sides }
}

fn lg_engine(dir: &Path, edges: &[Edge], resolver: Option<Resolver>) -> Outcome {
    let repo = Repository::open(dir).unwrap();
    let sig = Signature::now(ID_NAME, ID_MAIL).unwrap();
    let mut tips: BTreeMap<String, String> = BTreeMap::new();
    let mut resolved = 0usize;

    for ed in edges {
        let br = Oid::from_str(&rev(dir, &ed.branch)).unwrap();
        let up = Oid::from_str(&rev(dir, &ed.upstream)).unwrap();
        let onto = match tips.get(&ed.onto) {
            Some(s) => Oid::from_str(s).unwrap(),
            None => Oid::from_str(&rev(dir, &ed.onto)).unwrap(),
        };
        let a_br = repo.find_annotated_commit(br).unwrap();
        let a_up = repo.find_annotated_commit(up).unwrap();
        let a_onto = repo.find_annotated_commit(onto).unwrap();
        let mut opts = RebaseOptions::new();
        opts.inmemory(true);
        let mut rb = match repo.rebase(Some(&a_br), Some(&a_up), Some(&a_onto), Some(&mut opts)) {
            Ok(r) => r,
            Err(err) => return Outcome::Error(format!("{}: init: {err}", ed.branch)),
        };

        let mut last = onto;
        loop {
            let step = rb.next().map(|r| r.map(|op| op.id()));
            let needs_fix = match &step {
                None => break,
                Some(Err(err)) if err.code() == ErrorCode::Unmerged => true,
                Some(Err(err)) => {
                    return Outcome::Error(format!("{}: next: {err} ({:?})", ed.branch, err.code()))
                }
                Some(Ok(_)) => rb.inmemory_index().unwrap().has_conflicts(),
            };
            if needs_fix {
                let conflict = {
                    let idx = rb.inmemory_index().unwrap();
                    lg_conflict(&repo, &idx, &ed.branch)
                };
                match resolver {
                    None => return Outcome::Stopped(conflict),
                    Some(r) => {
                        let mut idx = rb.inmemory_index().unwrap();
                        for (path, bytes) in r(&conflict) {
                            idx.conflict_remove(Path::new(&path)).unwrap();
                            let id = repo.blob(&bytes).unwrap();
                            let entry = IndexEntry {
                                ctime: IndexTime::new(0, 0),
                                mtime: IndexTime::new(0, 0),
                                dev: 0,
                                ino: 0,
                                mode: 0o100644,
                                uid: 0,
                                gid: 0,
                                file_size: bytes.len() as u32,
                                id,
                                flags: 0,
                                flags_extended: 0,
                                path: path.into_bytes(),
                            };
                            idx.add(&entry).unwrap();
                        }
                        resolved += 1;
                    }
                }
            }
            match rb.commit(None, &sig, None) {
                Ok(oid) => last = oid,
                Err(err) if err.code() == ErrorCode::Applied && SKIP_APPLIED => {
                    println!("NOTE {}: libgit2 Applied on commit, skipping", ed.branch);
                }
                Err(err) => return Outcome::Error(format!("{}: commit: {err}", ed.branch)),
            }
        }
        if let Err(err) = rb.finish(None) {
            return Outcome::Error(format!("{}: finish: {err}", ed.branch));
        }
        tips.insert(ed.branch.clone(), last.to_string());
    }
    Outcome::Done { tips, resolved }
}

// ---------------------------------------------------------------- engine: merge-tree

fn mt_blob(dir: &Path, oid: &str) -> Vec<u8> {
    git_bytes(dir, &["cat-file", "blob", oid]).expect("cat-file blob")
}

/// Parse the conflicted-file-info section of `merge-tree --write-tree` output.
fn mt_sides(dir: &Path, stdout: &str) -> Vec<Side> {
    let mut map: BTreeMap<String, [Option<String>; 3]> = BTreeMap::new();
    for line in stdout.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        let Some((info, path)) = line.split_once('\t') else { continue };
        let mut it = info.split_whitespace();
        let _mode = it.next().unwrap();
        let oid = it.next().unwrap();
        let slot = match it.next().unwrap() {
            "1" => 0,
            "2" => 1,
            "3" => 2,
            _ => continue,
        };
        map.entry(path.to_string()).or_insert([None, None, None])[slot] = Some(oid.to_string());
    }
    map.into_iter()
        .map(|(path, [b, o, t])| Side {
            path,
            base: b.map(|x| mt_blob(dir, &x)),
            ours: o.map(|x| mt_blob(dir, &x)),
            theirs: t.map(|x| mt_blob(dir, &x)),
        })
        .collect()
}

/// Write `files` over `tree` using a scratch index (GIT_INDEX_FILE outside .git).
fn mt_apply_resolution(dir: &Path, tree: &str, files: &[(String, Vec<u8>)]) -> String {
    let scratch = tempfile::Builder::new().prefix("idx-").tempdir_in(spike_tmp()).unwrap();
    let idx = scratch.path().join("index");
    let idx_s = idx.to_str().unwrap();
    let env = [("GIT_INDEX_FILE", idx_s)];
    let o = git_out(dir, &["read-tree", tree], &env);
    assert!(o.status.success(), "read-tree");
    for (path, bytes) in files {
        let f = scratch.path().join("blob");
        fs::write(&f, bytes).unwrap();
        let blob = String::from_utf8(git_out(dir, &["hash-object", "-w", f.to_str().unwrap()], &[]).stdout)
            .unwrap()
            .trim()
            .to_string();
        let spec = format!("100644,{blob},{path}");
        let o = git_out(dir, &["update-index", "--add", "--cacheinfo", &spec], &env);
        assert!(o.status.success(), "update-index");
    }
    String::from_utf8(git_out(dir, &["write-tree"], &env).stdout).unwrap().trim().to_string()
}

fn mt_commit_tree(dir: &Path, tree: &str, parent: &str, src: &str) -> String {
    let meta = git(dir, &["log", "-1", "--format=%an%x1f%ae%x1f%aI", src]);
    let parts: Vec<&str> = meta.split('\x1f').collect();
    let msg = git(dir, &["log", "-1", "--format=%B", src]);
    let o = git_out(
        dir,
        &["commit-tree", tree, "-p", parent, "-m", &msg],
        &[
            ("GIT_AUTHOR_NAME", parts[0]),
            ("GIT_AUTHOR_EMAIL", parts[1]),
            ("GIT_AUTHOR_DATE", parts[2]),
        ],
    );
    assert!(o.status.success(), "commit-tree: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

fn mt_engine(dir: &Path, edges: &[Edge], resolver: Option<Resolver>) -> Outcome {
    let mut tips: BTreeMap<String, String> = BTreeMap::new();
    let mut resolved = 0usize;
    for ed in edges {
        let br = rev(dir, &ed.branch);
        let up = rev(dir, &ed.upstream);
        let onto = match tips.get(&ed.onto) {
            Some(s) => s.clone(),
            None => rev(dir, &ed.onto),
        };
        let list = git(dir, &["rev-list", "--reverse", "--topo-order", "--no-merges", &format!("{up}..{br}")]);
        let mut cur = onto;
        for c in list.lines().filter(|l| !l.is_empty()) {
            let parent = git(dir, &["rev-parse", &format!("{c}^")]);
            let o = git_out(
                dir,
                &["merge-tree", "--write-tree", &format!("--merge-base={parent}"), &cur, c],
                &[],
            );
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let mut tree = stdout.lines().next().unwrap_or("").to_string();
            match o.status.code() {
                Some(0) => {}
                Some(1) => {
                    let conflict = Conflict {
                        branch: ed.branch.clone(),
                        at: c.to_string(),
                        sides: mt_sides(dir, &stdout),
                    };
                    match resolver {
                        None => return Outcome::Stopped(conflict),
                        Some(r) => {
                            tree = mt_apply_resolution(dir, &tree, &r(&conflict));
                            resolved += 1;
                        }
                    }
                }
                _ => {
                    return Outcome::Error(format!(
                        "{}: merge-tree exit {:?}: {}",
                        ed.branch,
                        o.status.code(),
                        String::from_utf8_lossy(&o.stderr)
                    ))
                }
            }
            // Mirror git rebase's default of dropping commits that become empty.
            if tree == tree_of(dir, &cur) {
                continue;
            }
            cur = mt_commit_tree(dir, &tree, &cur, c);
        }
        tips.insert(ed.branch.clone(), cur);
        let _ = br;
    }
    Outcome::Done { tips, resolved }
}

// ---------------------------------------------------------------- reference: git rebase --onto

fn ref_engine(dir: &Path, edges: &[Edge], resolver: Option<Resolver>) -> Outcome {
    let mut resolved = 0usize;
    let mut tips = BTreeMap::new();
    for ed in edges {
        let o = git_out(dir, &["rebase", "--onto", &ed.onto, &ed.upstream, &ed.branch], &[("GIT_EDITOR", "true")]);
        if !o.status.success() {
            loop {
                let unmerged = git(dir, &["diff", "--name-only", "--diff-filter=U"]);
                let paths: Vec<String> = unmerged.lines().map(String::from).collect();
                if paths.is_empty() {
                    return Outcome::Error(format!(
                        "{}: rebase failed: {}",
                        ed.branch,
                        String::from_utf8_lossy(&o.stderr)
                    ));
                }
                let sides = paths
                    .iter()
                    .map(|p| Side {
                        path: p.clone(),
                        base: git_bytes(dir, &["show", &format!(":1:{p}")]),
                        ours: git_bytes(dir, &["show", &format!(":2:{p}")]),
                        theirs: git_bytes(dir, &["show", &format!(":3:{p}")]),
                    })
                    .collect();
                let conflict = Conflict {
                    branch: ed.branch.clone(),
                    at: git(dir, &["rev-parse", "REBASE_HEAD"]),
                    sides,
                };
                match resolver {
                    None => {
                        git_out(dir, &["rebase", "--abort"], &[]);
                        return Outcome::Stopped(conflict);
                    }
                    Some(r) => {
                        for (p, bytes) in r(&conflict) {
                            fs::write(dir.join(&p), &bytes).unwrap();
                            git(dir, &["add", &p]);
                        }
                        resolved += 1;
                        let c = git_out(dir, &["rebase", "--continue"], &[("GIT_EDITOR", "true")]);
                        if c.status.success() {
                            break;
                        }
                    }
                }
            }
        }
        tips.insert(ed.branch.clone(), rev(dir, &ed.branch));
    }
    Outcome::Done { tips, resolved }
}

// ---------------------------------------------------------------- comparison

fn paths(c: &Conflict) -> String {
    c.sides.iter().map(|s| s.path.as_str()).collect::<Vec<_>>().join(",")
}

fn verdict(dir_e: &Path, dir_r: &Path, eng: &Outcome, rf: &Outcome) -> String {
    match (eng, rf) {
        (Outcome::Error(m), _) => format!("ERROR: {m}"),
        (_, Outcome::Error(m)) => format!("REF ERROR: {m}"),
        (Outcome::Done { tips: a, resolved }, Outcome::Done { tips: b, .. }) => {
            let mut diffs = Vec::new();
            for (br, sha) in a {
                let rsha = &b[br];
                if tree_of(dir_e, sha) != tree_of(dir_r, rsha) {
                    diffs.push(format!("{br}: tree differs"));
                } else if count_of(dir_e, sha) != count_of(dir_r, rsha) {
                    diffs.push(format!(
                        "{br}: commits {} vs git {}",
                        count_of(dir_e, sha),
                        count_of(dir_r, rsha)
                    ));
                }
            }
            if diffs.is_empty() {
                format!("MATCH git rebase --onto (tree+commit count; resolved={resolved})")
            } else {
                format!("MISMATCH {}", diffs.join("; "))
            }
        }
        (Outcome::Stopped(c1), Outcome::Stopped(c2)) => {
            if c1.sides == c2.sides {
                format!("CONFLICT, same paths and sides as git: [{}]", paths(c1))
            } else {
                format!("CONFLICT differs: engine=[{}] git=[{}]", paths(c1), paths(c2))
            }
        }
        (Outcome::Done { .. }, Outcome::Stopped(c)) => {
            format!("MISMATCH: engine completed, git stopped on [{}]", paths(c))
        }
        (Outcome::Stopped(c), Outcome::Done { .. }) => {
            format!("MISMATCH: engine stopped on [{}], git completed", paths(c))
        }
    }
}

pub struct Scenario {
    pub name: &'static str,
    pub setup: fn(&Path),
    pub edges: Vec<Edge>,
    pub resolver: Option<Resolver>,
}

pub struct Report {
    pub name: &'static str,
    pub lg: String,
    pub mt: String,
}

/// Runs both engines on repo 1 (asserting the working tree and index are
/// untouched) and git rebase on repo 2, returning one verdict per engine.
pub fn run(s: &Scenario) -> Report {
    let d1 = new_dir();
    let d2 = new_dir();
    (s.setup)(d1.path());
    (s.setup)(d2.path());

    let index_path = d1.path().join(".git/index");
    let index_before = fs::read(&index_path).unwrap();
    let head_before = fs::read(d1.path().join(".git/HEAD")).unwrap();

    let lg = lg_engine(d1.path(), &s.edges, s.resolver);
    let mt = mt_engine(d1.path(), &s.edges, s.resolver);

    assert_eq!(index_before, fs::read(&index_path).unwrap(), "{}: .git/index bytes changed", s.name);
    assert_eq!(head_before, fs::read(d1.path().join(".git/HEAD")).unwrap(), "{}: HEAD changed", s.name);
    for st in ["rebase-merge", "rebase-apply"] {
        assert!(!d1.path().join(".git").join(st).exists(), "{}: {st} state left behind", s.name);
    }
    let status = git(d1.path(), &["status", "--porcelain"]);
    assert!(status.is_empty(), "{}: working tree dirty:\n{status}", s.name);

    let rf = ref_engine(d2.path(), &s.edges, s.resolver);
    let lg_v = verdict(d1.path(), d2.path(), &lg, &rf);
    let mt_v = verdict(d1.path(), d2.path(), &mt, &rf);
    println!("ROW {} | libgit2 | {}", s.name, lg_v);
    println!("ROW {} | merge-tree | {}", s.name, mt_v);
    if let Outcome::Stopped(c) = &lg {
        println!("DETAIL {} | libgit2 conflict sides: {}", s.name, describe(c));
    }
    if let Outcome::Stopped(c) = &mt {
        println!("DETAIL {} | merge-tree conflict sides: {}", s.name, describe(c));
    }
    Report { name: s.name, lg: lg_v, mt: mt_v }
}

fn describe(c: &Conflict) -> String {
    c.sides
        .iter()
        .map(|s| {
            let f = |b: &Option<Vec<u8>>| match b {
                Some(v) => format!("{}B", v.len()),
                None => "-".into(),
            };
            format!("{} (base {}, ours {}, theirs {})", s.path, f(&s.base), f(&s.ours), f(&s.theirs))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

// ---------------------------------------------------------------- scenarios

fn setup_clean(dir: &Path) {
    init(dir);
    put(dir, "base.txt", "base\n");
    commit(dir, "base");
    tag_old(dir, "main");
    branch_from(dir, "A", "main");
    put(dir, "a.txt", "a1\n");
    commit(dir, "a1");
    put(dir, "a2.txt", "a2\n");
    commit(dir, "a2");
    tag_old(dir, "A");
    branch_from(dir, "B", "A");
    put(dir, "b.txt", "b\n");
    commit(dir, "b1");
    tag_old(dir, "B");
    branch_from(dir, "C", "B");
    put(dir, "c.txt", "c\n");
    commit(dir, "c1");
    co(dir, "main");
    put(dir, "m.txt", "m1\n");
    commit(dir, "main moves");
}

fn setup_conflict_middle(dir: &Path) {
    init(dir);
    put(dir, "f.txt", "l1\nl2\nl3\n");
    commit(dir, "base");
    tag_old(dir, "main");
    branch_from(dir, "A", "main");
    put(dir, "f.txt", "A1\nl2\nl3\n");
    commit(dir, "A changes line 1");
    tag_old(dir, "A");
    branch_from(dir, "B", "A");
    put(dir, "f.txt", "A1\nl2\nB3\n");
    commit(dir, "B changes line 3");
    tag_old(dir, "B");
    branch_from(dir, "C", "B");
    put(dir, "c.txt", "c\n");
    commit(dir, "c1");
    co(dir, "main");
    put(dir, "f.txt", "l1\nl2\nM3\n");
    commit(dir, "main changes line 3");
}

fn resolve_f(_: &Conflict) -> Vec<(String, Vec<u8>)> {
    vec![("f.txt".into(), b"A1\nl2\nRESOLVED\n".to_vec())]
}

fn setup_rename(dir: &Path) {
    init(dir);
    let body: String = (1..=8).map(|i| format!("{i}\n")).collect();
    put(dir, "f.txt", body);
    commit(dir, "base");
    tag_old(dir, "main");
    branch_from(dir, "A", "main");
    let edited: String = (1..=8).map(|i| if i == 2 { "two\n".to_string() } else { format!("{i}\n") }).collect();
    put(dir, "f.txt", edited);
    commit(dir, "A edits f.txt");
    tag_old(dir, "A");
    branch_from(dir, "B", "A");
    put(dir, "b.txt", "b\n");
    commit(dir, "b1");
    co(dir, "main");
    git(dir, &["mv", "f.txt", "g.txt"]);
    commit(dir, "main renames f.txt to g.txt");
}

fn setup_empty(dir: &Path) {
    init(dir);
    put(dir, "base.txt", "base\n");
    commit(dir, "base");
    tag_old(dir, "main");
    branch_from(dir, "A", "main");
    put(dir, "x.txt", "X\n");
    commit(dir, "A1 adds x");
    put(dir, "y.txt", "Y\n");
    commit(dir, "A2 adds y");
    tag_old(dir, "A");
    branch_from(dir, "B", "A");
    put(dir, "b.txt", "b\n");
    commit(dir, "b1");
    co(dir, "main");
    put(dir, "x.txt", "X\n");
    commit(dir, "main adds the same x upstream");
}

fn setup_merge(dir: &Path) {
    init(dir);
    put(dir, "base.txt", "base\n");
    commit(dir, "base");
    tag_old(dir, "main");
    branch_from(dir, "A", "main");
    put(dir, "a.txt", "a1\n");
    commit(dir, "a1");
    branch_from(dir, "S", "A");
    put(dir, "s.txt", "s1\n");
    commit(dir, "s1");
    co(dir, "A");
    put(dir, "a2.txt", "a2\n");
    commit(dir, "a2");
    git(dir, &["merge", "-q", "--no-ff", "-m", "merge S into A", "S"]);
    tag_old(dir, "A");
    branch_from(dir, "B", "A");
    put(dir, "b.txt", "b\n");
    commit(dir, "b1");
    co(dir, "main");
    put(dir, "m.txt", "m1\n");
    commit(dir, "main moves");
}

fn setup_binary_clean(dir: &Path) {
    init(dir);
    put(dir, "base.txt", "base\n");
    put(dir, "bin.dat", bin_bytes(3));
    commit(dir, "base");
    tag_old(dir, "main");
    branch_from(dir, "A", "main");
    put(dir, "bin.dat", bin_bytes(7));
    commit(dir, "A changes binary");
    tag_old(dir, "A");
    branch_from(dir, "B", "A");
    put(dir, "b.txt", "b\n");
    commit(dir, "b1");
    co(dir, "main");
    put(dir, "m.txt", "m1\n");
    commit(dir, "main moves");
}

fn setup_binary_conflict(dir: &Path) {
    init(dir);
    put(dir, "base.txt", "base\n");
    put(dir, "bin.dat", bin_bytes(3));
    commit(dir, "base");
    tag_old(dir, "main");
    branch_from(dir, "A", "main");
    put(dir, "bin.dat", bin_bytes(7));
    commit(dir, "A changes binary");
    tag_old(dir, "A");
    branch_from(dir, "B", "A");
    put(dir, "b.txt", "b\n");
    commit(dir, "b1");
    co(dir, "main");
    put(dir, "bin.dat", bin_bytes(11));
    commit(dir, "main changes binary too");
}

fn resolve_bin(_: &Conflict) -> Vec<(String, Vec<u8>)> {
    vec![("bin.dat".into(), b"resolved binary\0\x01".to_vec())]
}

fn setup_bulk(dir: &Path) {
    init(dir);
    for i in 0..3000 {
        put(dir, &format!("bulk/{i:04}.txt"), format!("{i}\n"));
    }
    commit(dir, "base with 3000 files");
    tag_old(dir, "main");
    branch_from(dir, "A", "main");
    put(dir, "bulk/0001.txt", "A\n");
    put(dir, "a.txt", "a\n");
    commit(dir, "A1");
    tag_old(dir, "A");
    branch_from(dir, "B", "A");
    put(dir, "bulk/2999.txt", "B\n");
    commit(dir, "B1");
    co(dir, "main");
    put(dir, "bulk/0500.txt", "M\n");
    put(dir, "m.txt", "m\n");
    commit(dir, "main moves");
}

fn setup_mode(dir: &Path) {
    init(dir);
    put(dir, "base.txt", "base\n");
    put(dir, "run.sh", "echo hi\n");
    commit(dir, "base");
    tag_old(dir, "main");
    branch_from(dir, "A", "main");
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    commit(dir, "A makes run.sh executable");
    tag_old(dir, "A");
    branch_from(dir, "B", "A");
    put(dir, "b.txt", "b\n");
    commit(dir, "b1");
    co(dir, "main");
    put(dir, "m.txt", "m1\n");
    commit(dir, "main moves");
}

/// A was amended after B was created on the old A tip. B's recorded
/// stackParentBase is the old A tip (tag oldtip-A), not the new A tip.
fn setup_amended_parent(dir: &Path) {
    init(dir);
    put(dir, "base.txt", "base\n");
    commit(dir, "base");
    tag_old(dir, "main");
    branch_from(dir, "A", "main");
    put(dir, "a.txt", "a1\n");
    commit(dir, "a1");
    tag_old(dir, "A");
    branch_from(dir, "B", "A");
    put(dir, "b.txt", "b\n");
    commit(dir, "b1");
    co(dir, "A");
    put(dir, "a.txt", "a1 amended\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--amend", "--no-verify", "-m", "a1 amended"]);
    co(dir, "main");
    put(dir, "m.txt", "m1\n");
    commit(dir, "main moves");
}

fn stack_edges_clean() -> Vec<Edge> {
    vec![e("A", "oldtip-main", "main"), e("B", "oldtip-A", "A"), e("C", "oldtip-B", "B")]
}

pub fn scenarios() -> Vec<Scenario> {
    let two = || vec![e("A", "oldtip-main", "main"), e("B", "oldtip-A", "A")];
    vec![
        Scenario { name: "clean_3_branch_stack", setup: setup_clean, edges: stack_edges_clean(), resolver: None },
        Scenario { name: "conflict_middle_stop", setup: setup_conflict_middle, edges: stack_edges_clean(), resolver: None },
        Scenario { name: "conflict_middle_resolve", setup: setup_conflict_middle, edges: stack_edges_clean(), resolver: Some(resolve_f) },
        Scenario { name: "rename_in_base", setup: setup_rename, edges: two(), resolver: None },
        Scenario { name: "commit_becomes_empty", setup: setup_empty, edges: two(), resolver: None },
        Scenario { name: "merge_commit_in_stack", setup: setup_merge, edges: two(), resolver: None },
        Scenario { name: "binary_clean", setup: setup_binary_clean, edges: two(), resolver: None },
        Scenario { name: "binary_conflict_stop", setup: setup_binary_conflict, edges: two(), resolver: None },
        Scenario { name: "binary_conflict_resolve", setup: setup_binary_conflict, edges: two(), resolver: Some(resolve_bin) },
        Scenario { name: "bulk_3000_files", setup: setup_bulk, edges: two(), resolver: None },
        Scenario { name: "file_mode_change", setup: setup_mode, edges: two(), resolver: None },
        Scenario { name: "amended_parent_base", setup: setup_amended_parent, edges: two(), resolver: None },
    ]
}

// ---------------------------------------------------------------- harness main: 200-commit timing

fn setup_bench(dir: &Path) {
    init(dir);
    put(dir, "base/f0.txt", "0\n");
    commit(dir, "base");
    tag_old(dir, "main");
    let mut prev = "main".to_string();
    for i in 1..=10 {
        let b = format!("s{i}");
        branch_from(dir, &b, &prev);
        for j in 0..20 {
            put(dir, &format!("s{i}/c{j}.txt"), format!("{i}-{j}\n"));
            commit(dir, &format!("s{i} c{j}"));
        }
        tag_old(dir, &b);
        prev = b;
    }
    co(dir, "main");
    put(dir, "m.txt", "main moves\n");
    commit(dir, "main moves");
}

fn bench_edges() -> Vec<Edge> {
    (1..=10)
        .map(|i| {
            let up = if i == 1 { "oldtip-main".to_string() } else { format!("oldtip-s{}", i - 1) };
            let onto = if i == 1 { "main".to_string() } else { format!("s{}", i - 1) };
            Edge { branch: format!("s{i}"), upstream: up, onto }
        })
        .collect()
}

fn main() {
    let v = git_out(Path::new("."), &["--version"], &[]);
    println!("git: {}", String::from_utf8_lossy(&v.stdout).trim());

    let d1 = new_dir();
    let d2 = new_dir();
    setup_bench(d1.path());
    setup_bench(d2.path());
    let edges = bench_edges();
    println!("bench: 10 branches x 20 commits = 200 commits to replay");

    let t = Instant::now();
    let lg = lg_engine(d1.path(), &edges, None);
    println!("libgit2 in-memory: {:?}", t.elapsed());

    let t = Instant::now();
    let mt = mt_engine(d1.path(), &edges, None);
    println!("merge-tree per commit: {:?}", t.elapsed());

    let t = Instant::now();
    let rf = ref_engine(d2.path(), &edges, None);
    println!("git rebase --onto (reference, subprocess): {:?}", t.elapsed());

    println!("libgit2 vs git: {}", verdict(d1.path(), d2.path(), &lg, &rf));
    println!("merge-tree vs git: {}", verdict(d1.path(), d2.path(), &mt, &rf));
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario(name: &str) -> Scenario {
        scenarios().into_iter().find(|s| s.name == name).unwrap()
    }

    fn check_match(name: &str) {
        let r = run(&scenario(name));
        assert!(r.lg.starts_with("MATCH"), "{name} libgit2: {}", r.lg);
        assert!(r.mt.starts_with("MATCH"), "{name} merge-tree: {}", r.mt);
    }

    fn check_conflict(name: &str) {
        let r = run(&scenario(name));
        assert!(r.lg.starts_with("CONFLICT, same"), "{name} libgit2: {}", r.lg);
        assert!(r.mt.starts_with("CONFLICT, same"), "{name} merge-tree: {}", r.mt);
    }

    #[test]
    fn clean_3_branch_stack() { check_match("clean_3_branch_stack"); }

    #[test]
    fn conflict_middle_stop_reports_sides() { check_conflict("conflict_middle_stop"); }

    #[test]
    fn conflict_middle_resolve_then_continue() { check_match("conflict_middle_resolve"); }

    #[test]
    fn rename_in_base() { check_match("rename_in_base"); }

    /// libgit2 without the Applied skip fails here; see SKIP_APPLIED.
    #[test]
    fn commit_becomes_empty() {
        let r = run(&scenario("commit_becomes_empty"));
        assert!(r.lg.starts_with("MATCH"), "libgit2 (with skip): {}", r.lg);
        assert!(r.mt.starts_with("MATCH"), "merge-tree: {}", r.mt);
    }

    #[test]
    fn merge_commit_in_stack() { check_match("merge_commit_in_stack"); }

    #[test]
    fn binary_clean() { check_match("binary_clean"); }

    #[test]
    fn binary_conflict_stop_reports_sides() { check_conflict("binary_conflict_stop"); }

    #[test]
    fn binary_conflict_resolve_then_continue() { check_match("binary_conflict_resolve"); }

    #[test]
    fn bulk_3000_files() { check_match("bulk_3000_files"); }

    #[test]
    fn file_mode_change() { check_match("file_mode_change"); }

    #[test]
    fn amended_parent_base() { check_match("amended_parent_base"); }

    #[test]
    fn bench_200_commits_matches_git() {
        let d1 = new_dir();
        let d2 = new_dir();
        setup_bench(d1.path());
        setup_bench(d2.path());
        let edges = bench_edges();
        let lg = lg_engine(d1.path(), &edges, None);
        let mt = mt_engine(d1.path(), &edges, None);
        let rf = ref_engine(d2.path(), &edges, None);
        assert!(verdict(d1.path(), d2.path(), &lg, &rf).starts_with("MATCH"));
        assert!(verdict(d1.path(), d2.path(), &mt, &rf).starts_with("MATCH"));
    }
}

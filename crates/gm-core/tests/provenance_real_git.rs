//! Provenance (D144) against real repositories in temporary directories: `git blame`, the blob
//! of each commit and a scripted `threads.db` written through `ThreadStore`. Each test removes
//! its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::threads::store::{Batch, NewEdit, NewEntry, ThreadStore};
use gm_schema::threads::{
    AttachKind, ProvenanceCommit, ProvenanceResult, ProvenanceThread, RelationDirection,
    ThreadRelKind,
};
use gm_schema::{
    Actor, Command, CommandKind, CutBound, EditChange, EntryKind, Event, Evidence, LineRun,
    LinkKind, LinkOrigin, OperationId, OperationState, ThreadLink, Unverifiable,
};

/// 2017-07-14: before every commit below, so an edit made then can explain its lines.
const EDITED_AT: i64 = 1_500_000_000_000;
/// 2023-11-14: after every commit below, so an edit made then cannot.
const LATER: i64 = 1_700_000_000_000;
const FIRST: &str = "2020-01-01T00:00:00Z";
const SECOND: &str = "2020-02-01T00:00:00Z";
const THIRD: &str = "2020-03-01T00:00:00Z";
const BASE: &str = "2019-06-01T00:00:00Z";

const BODY: [&str; 5] = [
    "fn compute(x: i32) -> i32 {",
    "    let doubled = x * 2;",
    "    let shifted = doubled + 7;",
    "    shifted",
    "}",
];

fn git_with(dir: &Path, args: &[&str], date: Option<&str>) -> std::process::Output {
    let mut command = Process::new("git");
    command
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    if let Some(date) = date {
        command
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
    }
    command.output().unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_with(dir, args, None);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Commits `content` to `file` with the author date `date`, and returns the new commit's id.
fn commit_file(root: &Path, file: &str, content: &str, date: &str, message: &str) -> String {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, content).unwrap();
    git(root, &["add", "--", file]);
    let output = git_with(root, &["commit", "-q", "-m", message], Some(date));
    assert!(output.status.success());
    git(root, &["rev-parse", "HEAD"])
}

struct Repo {
    _base: tempfile::TempDir,
    root: PathBuf,
}

/// A repository on `main` with one commit of `other.txt`, dated `BASE`.
fn repo() -> Repo {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    commit_file(&root, "other.txt", "base\n", BASE, "base");
    Repo { _base: base, root }
}

fn body(lines: &[&str]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The unified diff a Write of `lines` makes: every line added, one hunk.
fn added(lines: &[&str]) -> String {
    let mut patch = format!("@@ -0,0 +1,{} @@\n", lines.len());
    for line in lines {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    patch
}

/// The blob id of `text`, as Git names it.
fn blob(text: &str) -> String {
    git2::Oid::hash_object(git2::ObjectType::Blob, text.as_bytes())
        .unwrap()
        .to_string()
}

fn entry(source: &str, kind: EntryKind, text: &str, at: i64) -> NewEntry {
    NewEntry {
        source_id: source.into(),
        kind,
        parent_id: None,
        tool: None,
        text: text.into(),
        args_json: None,
        is_error: false,
        truncated: false,
        orig_bytes: None,
        redactions: 0,
        hits: Vec::new(),
        at,
        edit: None,
    }
}

/// A tool call that edited `path` with `patch`; `post` is the blob the file had afterwards.
fn edit_entry(
    source: &str,
    path: &str,
    diff: &str,
    post: Option<String>,
    from_input: bool,
    at: i64,
) -> NewEntry {
    let mut found = entry(source, EntryKind::ToolCall, "Edit", at);
    found.tool = Some("Edit".into());
    found.edit = Some(NewEdit {
        path: path.into(),
        old_path: None,
        change: EditChange::Modify,
        patch: diff.into(),
        added: 0,
        removed: 0,
        pre_oid: None,
        post_oid: post,
        from_input,
        hits: Vec::new(),
    });
    found
}

/// A thread with a prompt and the given edit, stored in `store` under `session`.
fn thread(store: &mut ThreadStore, root: &Path, session: &str, edit: NewEntry) -> String {
    let entries = vec![
        entry(
            "p1",
            EntryKind::User,
            "change the compute function",
            EDITED_AT,
        ),
        edit,
        entry("a1", EntryKind::Assistant, "done", EDITED_AT),
    ];
    let batch = Batch {
        harness: "claude-code".into(),
        session_id: session.into(),
        worktree: root.to_string_lossy().into_owned(),
        branch: Some("main".into()),
        head: None,
        model: Some("claude".into()),
        harness_version: None,
        transcript: None,
        cursor: None,
        started_at: EDITED_AT,
        at: EDITED_AT + 10,
        ended: None,
        entries,
        gaps: Vec::new(),
    };
    store.apply(&batch).expect("apply").uid
}

fn store_of(root: &Path) -> ThreadStore {
    ThreadStore::open(&root.join(".git")).expect("open the store")
}

fn engine_on(root: &Path) -> Engine {
    let git = SubprocessGit {
        log: Some(CommandLog::default()),
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let events = engine.handle(
        OperationId(1),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::OpenRepo {
                path: root.to_string_lossy().into_owned(),
            },
        },
    );
    assert!(done(&events), "open failed: {events:?}");
    engine
}

fn done(events: &[Event]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            Event::OperationChanged {
                state: OperationState::Done,
                ..
            }
        )
    })
}

/// Asks why lines `from` to `to` of `path` are as they are, as an agent (reads are open to it).
/// The blame runs as a job, as the runner runs it: the answer comes when the job reports back.
fn explain(engine: &mut Engine, path: &str, from: u32, to: u32) -> ProvenanceResult {
    let started = engine.handle(
        OperationId(9),
        &Command {
            actor: Actor::Agent {
                name: "test".into(),
            },
            kind: CommandKind::ReadProvenance {
                path: path.into(),
                from_line: from,
                to_line: to,
            },
        },
    );
    assert!(!done(&started), "the read answers later, not in the call");
    let job = engine.take_provenance_jobs().pop().expect("a blame job");
    let events = engine.provenance_report(job.run());
    assert!(done(&events), "read failed: {events:?}");
    events
        .iter()
        .find_map(|event| match event {
            Event::ThreadProvenance { provenance, .. } => Some(provenance.clone()),
            _ => None,
        })
        .expect("a provenance answer")
}

fn commit_of<'a>(result: &'a ProvenanceResult, id: &str) -> &'a ProvenanceCommit {
    result
        .commits
        .iter()
        .find(|commit| commit.id == id)
        .unwrap_or_else(|| panic!("no commit {id} in {result:?}"))
}

fn thread_of<'a>(commit: &'a ProvenanceCommit, uid: &str) -> &'a ProvenanceThread {
    commit
        .threads
        .iter()
        .find(|thread| thread.uid == uid)
        .unwrap_or_else(|| panic!("no thread {uid} in {commit:?}"))
}

#[test]
fn a_blob_match_is_the_strongest_evidence() {
    let repo = repo();
    let content = body(&BODY);
    let commit = commit_file(&repo.root, "src/lib.rs", &content, FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let uid = thread(
        &mut store,
        &repo.root,
        "blob",
        edit_entry(
            "e1",
            "src/lib.rs",
            &added(&BODY),
            Some(blob(&content)),
            false,
            EDITED_AT,
        ),
    );
    drop(store);

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);

    let owner = commit_of(&result, &commit);
    assert_eq!(owner.path, "src/lib.rs");
    assert_eq!(owner.lines, vec![LineRun { from: 1, to: 5 }]);
    assert_eq!(owner.subject, "add compute");
    let found = thread_of(owner, &uid);
    assert_eq!(
        found.evidence,
        vec![
            Evidence::BlobMatch,
            Evidence::LineMatch,
            Evidence::HunkOverlap
        ]
    );
    assert!(!found.ambiguous);
    let edit = &found.edits[0];
    assert_eq!(edit.seq, 1, "entries are numbered from 0");
    let detail = store_of(&repo.root)
        .read_thread(&uid, 0, 10)
        .expect("read the thread");
    let carrier = detail
        .entries
        .iter()
        .find(|entry| entry.seq == edit.seq)
        .expect("the entry that carried the edit");
    assert_eq!(
        edit.entry_id, carrier.id,
        "the edit names its entry's row id"
    );
    assert_eq!(edit.evidence[0], Evidence::BlobMatch);
    assert_eq!(edit.unverifiable, None);
    assert!(
        edit.entries
            .iter()
            .any(|entry| entry.kind == EntryKind::User)
    );
    assert!(result.cuts.is_empty());
}

#[test]
fn a_line_match_holds_with_an_absolute_path_and_no_blob() {
    let repo = repo();
    let commit = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let absolute = format!("{}/src/lib.rs", repo.root.display());
    let uid = thread(
        &mut store,
        &repo.root,
        "line",
        edit_entry("e1", &absolute, &added(&BODY), None, false, EDITED_AT),
    );
    drop(store);

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    let found = thread_of(commit_of(&result, &commit), &uid);
    assert_eq!(
        found.evidence,
        vec![Evidence::LineMatch, Evidence::HunkOverlap]
    );
    assert_eq!(found.edits[0].path, absolute);
    assert!(!found.ambiguous);
}

#[test]
fn several_matching_edits_are_all_returned_and_marked_ambiguous() {
    let repo = repo();
    let commit = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let first = thread(
        &mut store,
        &repo.root,
        "first",
        edit_entry("e1", "src/lib.rs", &added(&BODY), None, false, EDITED_AT),
    );
    let second = thread(
        &mut store,
        &repo.root,
        "second",
        edit_entry("e2", "src/lib.rs", &added(&BODY), None, false, EDITED_AT),
    );
    let unrelated = thread(
        &mut store,
        &repo.root,
        "unrelated",
        edit_entry(
            "e3",
            "src/lib.rs",
            &added(&["let other = 1;"]),
            None,
            false,
            EDITED_AT,
        ),
    );
    let before = store.read_thread(&unrelated, 0, 100).unwrap().links.len();
    drop(store);

    let mut engine = engine_on(&repo.root);
    let logged = engine.log().len();
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    let owner = commit_of(&result, &commit);

    assert_eq!(
        owner.threads.len(),
        2,
        "the unrelated thread is not an answer"
    );
    for uid in [&first, &second] {
        let found = thread_of(owner, uid);
        assert!(found.ambiguous, "both matches are marked ambiguous");
        assert_eq!(found.edits.len(), 1);
        assert_eq!(found.evidence[0], Evidence::LineMatch);
    }
    assert!(owner.threads.iter().all(|found| found.uid != unrelated));

    // A line match never creates a stored link, and a read leaves the log alone.
    let store = store_of(&repo.root);
    assert!(store.read_thread(&first, 0, 100).unwrap().links.is_empty());
    assert_eq!(
        store.read_thread(&unrelated, 0, 100).unwrap().links.len(),
        before
    );
    assert_eq!(
        engine.log().len(),
        logged,
        "a read is not in the command log"
    );
}

#[test]
fn a_formatter_run_after_the_edit_breaks_the_match() {
    let repo = repo();
    let spaced = [
        "fn  compute(x: i32) -> i32 {",
        "    let  doubled = x * 2;",
        "    let  shifted = doubled + 7;",
        "    shifted",
        "}",
    ];
    let mut store = store_of(&repo.root);
    let uid = thread(
        &mut store,
        &repo.root,
        "fmt",
        edit_entry("e1", "src/lib.rs", &added(&spaced), None, false, EDITED_AT),
    );
    drop(store);
    commit_file(
        &repo.root,
        "src/lib.rs",
        &body(&spaced),
        FIRST,
        "unformatted",
    );
    let formatted = commit_file(&repo.root, "src/lib.rs", &body(&BODY), SECOND, "rustfmt");

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    let owner = commit_of(&result, &formatted);
    assert!(
        owner.threads.is_empty(),
        "the formatter owns lines 1 to 3: {owner:?}"
    );
    let older = result
        .commits
        .iter()
        .find(|commit| commit.id != formatted)
        .expect("the unformatted commit owns the rest");
    let found = thread_of(older, &uid);
    assert!(!found.evidence.contains(&Evidence::LineMatch));
    assert!(!found.evidence.contains(&Evidence::BlobMatch));
}

#[test]
fn a_rebase_keeps_the_match() {
    let repo = repo();
    git(&repo.root, &["checkout", "-q", "-b", "feature"]);
    let original = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let uid = thread(
        &mut store,
        &repo.root,
        "rebase",
        edit_entry("e1", "src/lib.rs", &added(&BODY), None, false, EDITED_AT),
    );
    drop(store);
    git(&repo.root, &["checkout", "-q", "main"]);
    commit_file(&repo.root, "elsewhere.txt", "x\n", THIRD, "main moves");
    git(&repo.root, &["checkout", "-q", "feature"]);
    git(&repo.root, &["rebase", "-q", "main"]);

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    let owner = result.commits.first().expect("the rebased commit");
    assert_ne!(owner.id, original, "the commit was rewritten");
    assert_eq!(
        owner.author_at, 1_577_836_800_000,
        "the author date survives"
    );
    assert_eq!(
        thread_of(owner, &uid).evidence,
        vec![Evidence::LineMatch, Evidence::HunkOverlap]
    );
}

#[test]
fn a_squash_keeps_the_match() {
    let repo = repo();
    let base = git(&repo.root, &["rev-parse", "HEAD"]);
    let edited = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let uid = thread(
        &mut store,
        &repo.root,
        "squash",
        edit_entry("e1", "src/lib.rs", &added(&BODY), None, false, EDITED_AT),
    );
    drop(store);
    commit_file(&repo.root, "other.txt", "more\n", SECOND, "second");
    git(&repo.root, &["reset", "-q", "--soft", &base]);
    let squashed = git_with(&repo.root, &["commit", "-q", "-m", "squashed"], Some(FIRST));
    assert!(squashed.status.success());
    let squashed = git(&repo.root, &["rev-parse", "HEAD"]);
    assert_ne!(squashed, edited);

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    let owner = commit_of(&result, &squashed);
    assert_eq!(
        thread_of(owner, &uid).evidence,
        vec![Evidence::LineMatch, Evidence::HunkOverlap]
    );
}

#[test]
fn a_reorder_keeps_the_match() {
    let repo = repo();
    let base = git(&repo.root, &["rev-parse", "HEAD"]);
    let edited = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let uid = thread(
        &mut store,
        &repo.root,
        "reorder",
        edit_entry("e1", "src/lib.rs", &added(&BODY), None, false, EDITED_AT),
    );
    drop(store);
    let other = commit_file(&repo.root, "other.txt", "more\n", SECOND, "second");
    git(&repo.root, &["reset", "-q", "--hard", &base]);
    git(&repo.root, &["cherry-pick", &other]);
    git(&repo.root, &["cherry-pick", &edited]);
    let moved = git(&repo.root, &["rev-parse", "HEAD"]);
    assert_ne!(moved, edited, "the commit was rewritten");

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    let owner = commit_of(&result, &moved);
    assert_eq!(
        thread_of(owner, &uid).evidence,
        vec![Evidence::LineMatch, Evidence::HunkOverlap]
    );
}

#[test]
fn a_redacted_edit_is_unverifiable_not_no_match() {
    let repo = repo();
    let patch = added(&[
        "fn compute(x: i32) -> i32 {",
        "    let key = \"[REDACTED:api-key]\";",
        "    let shifted = doubled + 7;",
        "    shifted",
    ]);
    let commit = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let uid = thread(
        &mut store,
        &repo.root,
        "redacted",
        edit_entry("e1", "src/lib.rs", &patch, None, false, EDITED_AT),
    );
    drop(store);

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    let found = thread_of(commit_of(&result, &commit), &uid);
    assert!(found.evidence.is_empty());
    assert_eq!(found.edits[0].unverifiable, Some(Unverifiable::Redacted));
    assert!(found.edits[0].evidence.is_empty());
}

#[test]
fn a_truncated_edit_and_an_edit_from_input_are_unverifiable() {
    let repo = repo();
    let truncated = "@@ -0,0 +1,3 @@\n+fn compute(x: i32) -> i32 {\n[... truncated 900 bytes ...]\n+    shifted\n";
    let commit = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let cut = thread(
        &mut store,
        &repo.root,
        "cut",
        edit_entry("e1", "src/lib.rs", truncated, None, false, EDITED_AT),
    );
    let known = thread(
        &mut store,
        &repo.root,
        "known",
        edit_entry("e2", "src/lib.rs", &added(&BODY), None, true, EDITED_AT),
    );
    drop(store);

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    let owner = commit_of(&result, &commit);
    assert_eq!(
        thread_of(owner, &cut).edits[0].unverifiable,
        Some(Unverifiable::Truncated)
    );
    assert_eq!(
        thread_of(owner, &known).edits[0].unverifiable,
        Some(Unverifiable::FromInput)
    );
    assert!(thread_of(owner, &known).evidence.is_empty());
}

#[test]
fn an_edit_made_after_the_commit_cannot_explain_it() {
    let repo = repo();
    let commit = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    thread(
        &mut store,
        &repo.root,
        "later",
        edit_entry("e1", "src/lib.rs", &added(&BODY), None, false, LATER),
    );
    drop(store);

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    assert!(commit_of(&result, &commit).threads.is_empty());
}

#[test]
fn at_most_ten_commits_are_shown_and_the_cut_is_said() {
    let repo = repo();
    let mut lines = Vec::new();
    let mut ids = Vec::new();
    for index in 1..=12 {
        lines.push(format!("line {index}"));
        let date = format!("2020-01-{index:02}T00:00:00Z");
        let text = format!("{}\n", lines.join("\n"));
        ids.push(commit_file(
            &repo.root,
            "src/many.txt",
            &text,
            &date,
            "one line",
        ));
    }

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/many.txt", 1, 12);
    assert_eq!(result.commits.len(), 10);
    let shown: Vec<&str> = result
        .commits
        .iter()
        .map(|commit| commit.id.as_str())
        .collect();
    assert_eq!(
        shown,
        ids[..10].iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(result.cuts.len(), 1);
    assert_eq!(result.cuts[0].bound, CutBound::Commits);
    assert_eq!(result.cuts[0].limit, 10);
}

#[test]
fn at_most_five_threads_are_shown_for_a_commit_and_the_cut_is_said() {
    let repo = repo();
    let commit = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let mut uids = Vec::new();
    for index in 0..7 {
        let uid = thread(
            &mut store,
            &repo.root,
            &format!("linked-{index}"),
            entry(
                &format!("u{index}"),
                EntryKind::User,
                "unrelated",
                EDITED_AT,
            ),
        );
        store
            .link(
                &uid,
                &ThreadLink {
                    kind: LinkKind::Commit,
                    reference: commit.clone(),
                    origin: LinkOrigin::Observed,
                    resolved: None,
                    state: None,
                },
            )
            .unwrap();
        uids.push(uid);
    }
    drop(store);

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    let owner = commit_of(&result, &commit);
    assert_eq!(owner.threads.len(), 5);
    assert!(
        owner
            .threads
            .iter()
            .all(|found| found.evidence == vec![Evidence::ObservedCommit])
    );
    assert!(result.cuts.iter().any(|cut| cut.bound == CutBound::Threads
        && cut.limit == 5
        && cut.commit.as_deref() == Some(commit.as_str())));
}

#[test]
fn a_manual_link_is_shown_as_linked_by_you() {
    let repo = repo();
    let commit = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let uid = thread(
        &mut store,
        &repo.root,
        "manual",
        entry("u1", EntryKind::User, "unrelated", EDITED_AT),
    );
    store
        .link(
            &uid,
            &ThreadLink {
                kind: LinkKind::Commit,
                reference: commit.clone(),
                origin: LinkOrigin::Human,
                resolved: None,
                state: None,
            },
        )
        .unwrap();
    drop(store);

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    assert_eq!(
        thread_of(commit_of(&result, &commit), &uid).evidence,
        vec![Evidence::Manual]
    );
}

#[test]
fn bad_paths_and_ranges_are_refused() {
    let repo = repo();
    commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut engine = engine_on(&repo.root);
    for (path, from, to) in [
        ("../outside.rs", 1, 2),
        ("src/lib.rs", 0, 2),
        ("src/lib.rs", 4, 2),
        ("src/lib.rs", 1, 2000),
    ] {
        let events = engine.handle(
            OperationId(3),
            &Command {
                actor: Actor::Human,
                kind: CommandKind::ReadProvenance {
                    path: path.into(),
                    from_line: from,
                    to_line: to,
                },
            },
        );
        assert!(!done(&events), "{path} {from} {to} must be refused");
    }
}

fn ask(path: &str, from: u32, to: u32) -> Command {
    Command {
        actor: Actor::Human,
        kind: CommandKind::ReadProvenance {
            path: path.into(),
            from_line: from,
            to_line: to,
        },
    }
}

fn is_state(events: &[Event], id: u64, wanted: OperationState) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            Event::OperationChanged { id: got, state, .. }
                if *got == OperationId(id) && *state == wanted
        )
    })
}

#[test]
fn a_newer_read_cancels_the_blame_in_flight_and_its_answer_is_dropped() {
    let repo = repo();
    commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut engine = engine_on(&repo.root);
    engine.handle(OperationId(3), &ask("src/lib.rs", 1, 5));

    let superseding = engine.handle(OperationId(4), &ask("src/lib.rs", 1, 3));

    assert!(is_state(&superseding, 3, OperationState::Cancelled));
    let mut jobs = engine.take_provenance_jobs().into_iter();
    let stale = jobs.next().expect("the stale blame");
    assert!(
        stale.stop.load(std::sync::atomic::Ordering::SeqCst),
        "the stale blame must be stopped"
    );
    assert!(
        engine.provenance_report(stale.run()).is_empty(),
        "a cancelled read answers nothing"
    );
    let current = jobs.next().expect("the current blame");
    let events = engine.provenance_report(current.run());
    assert!(is_state(&events, 4, OperationState::Done));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::ThreadProvenance { id: OperationId(4), provenance } if provenance.to_line == 3
    )));
}

#[test]
fn leaving_the_repository_cancels_the_blame_in_flight() {
    let repo = repo();
    commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut engine = engine_on(&repo.root);
    engine.handle(OperationId(3), &ask("src/lib.rs", 1, 5));
    let job = engine.take_provenance_jobs().pop().expect("a blame job");

    let reopened = engine.handle(
        OperationId(5),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::OpenRepo {
                path: repo.root.to_string_lossy().into_owned(),
            },
        },
    );

    assert!(is_state(&reopened, 3, OperationState::Cancelled));
    assert!(engine.provenance_report(job.run()).is_empty());
}

#[test]
fn a_thread_shows_its_labels_and_linked_threads_at_most_ten_each_and_says_the_cut() {
    let repo = repo();
    let commit = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let hub = thread(
        &mut store,
        &repo.root,
        "hub",
        entry("u1", EntryKind::User, "unrelated", EDITED_AT),
    );
    store
        .link(
            &hub,
            &ThreadLink {
                kind: LinkKind::Commit,
                reference: commit.clone(),
                origin: LinkOrigin::Human,
                resolved: None,
                state: None,
            },
        )
        .unwrap();
    // Twelve labels and twelve threads linked out of the hub: each list shows ten.
    for index in 0..12 {
        let name = format!("label-{index:02}");
        store
            .attach_label(&name, AttachKind::Thread, &hub, None, "human", EDITED_AT)
            .unwrap();
        let spoke = thread(
            &mut store,
            &repo.root,
            &format!("spoke-{index:02}"),
            entry(&format!("s{index}"), EntryKind::User, "spoke", EDITED_AT),
        );
        store
            .link_threads(
                &hub,
                &spoke,
                ThreadRelKind::Relates,
                "agent:claude-code",
                EDITED_AT,
            )
            .unwrap();
    }
    drop(store);

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    let shown = thread_of(commit_of(&result, &commit), &hub);
    assert_eq!(shown.labels.len(), 10);
    assert_eq!(shown.labels[0].name, "label-00");
    assert_eq!(shown.labels[0].origin, "human");
    assert_eq!(shown.links.len(), 10);
    assert!(shown.links.iter().all(|link| {
        link.direction == RelationDirection::Out
            && link.kind == ThreadRelKind::Relates
            && link.origin == "agent:claude-code"
            && link.title == "change the compute function"
    }));
    for bound in [CutBound::Labels, CutBound::Links] {
        assert!(
            result.cuts.iter().any(|cut| cut.bound == bound
                && cut.limit == 10
                && cut.commit.as_deref() == Some(commit.as_str())),
            "no {bound:?} cut in {:?}",
            result.cuts
        );
    }
}

#[test]
fn a_thread_inside_the_bound_has_no_cut_and_shows_inbound_links_as_in() {
    let repo = repo();
    let commit = commit_file(&repo.root, "src/lib.rs", &body(&BODY), FIRST, "add compute");
    let mut store = store_of(&repo.root);
    let hub = thread(
        &mut store,
        &repo.root,
        "quiet",
        entry("u1", EntryKind::User, "unrelated", EDITED_AT),
    );
    let older = thread(
        &mut store,
        &repo.root,
        "older",
        entry("o1", EntryKind::User, "older", EDITED_AT),
    );
    store
        .link(
            &hub,
            &ThreadLink {
                kind: LinkKind::Commit,
                reference: commit.clone(),
                origin: LinkOrigin::Human,
                resolved: None,
                state: None,
            },
        )
        .unwrap();
    store
        .link_threads(&older, &hub, ThreadRelKind::Continues, "human", EDITED_AT)
        .unwrap();
    drop(store);

    let mut engine = engine_on(&repo.root);
    let result = explain(&mut engine, "src/lib.rs", 1, 5);
    let shown = thread_of(commit_of(&result, &commit), &hub);
    assert!(shown.labels.is_empty());
    assert_eq!(shown.links.len(), 1);
    assert_eq!(shown.links[0].uid, older);
    assert_eq!(shown.links[0].direction, RelationDirection::In);
    assert_eq!(shown.links[0].kind, ThreadRelKind::Continues);
    assert!(
        result
            .cuts
            .iter()
            .all(|cut| cut.bound != CutBound::Labels && cut.bound != CutBound::Links)
    );
}

//! Labels and thread links (D143, D145, D146) against real repositories in temporary directories.
//! The engine runs with the global and system Git configuration kept out. Each test removes its
//! directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::threads::store::{Batch, ThreadStore};
use gm_schema::threads::{
    AttachKind, Label, LabelDetail, SearchFacets, SearchGroup, SearchOrder, ThreadDetail,
    ThreadFilter, ThreadHit, ThreadRelKind,
};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

/// 2020-01-01: the committer date of the commits below, in milliseconds.
const COMMITTED_AT: i64 = 1_577_836_800_000;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
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
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct Repo {
    _base: tempfile::TempDir,
    root: PathBuf,
}

/// A repository on `main` with one commit, whose subject is `fix the parser`.
fn repo() -> Repo {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("parser.rs"), "fn parse() {}\n").unwrap();
    git(&root, &["add", "--", "parser.rs"]);
    git(&root, &["commit", "-q", "-m", "fix the parser"]);
    Repo { _base: base, root }
}

fn engine_on(root: &Path) -> Engine {
    let git = SubprocessGit {
        log: Some(CommandLog::default()),
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let events = send(
        &mut engine,
        &Actor::Human,
        CommandKind::OpenRepo {
            path: root.to_string_lossy().into_owned(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Done, "open: {events:?}");
    engine
}

fn send(engine: &mut Engine, actor: &Actor, kind: CommandKind) -> Vec<Event> {
    engine.handle(
        OperationId(1),
        &Command {
            actor: actor.clone(),
            kind,
        },
    )
}

fn agent() -> Actor {
    Actor::Agent {
        name: "claude-code".into(),
    }
}

fn state_of(events: &[Event]) -> OperationState {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::OperationChanged { state, .. } => Some(*state),
            _ => None,
        })
        .expect("an operation state")
}

fn failure(events: &[Event]) -> Option<String> {
    events.iter().rev().find_map(|event| match event {
        Event::OperationChanged { detail, .. } => detail.clone(),
        _ => None,
    })
}

fn label_of(events: &[Event]) -> Option<Label> {
    events.iter().find_map(|event| match event {
        Event::LabelsChanged { label, .. } => label.clone(),
        _ => None,
    })
}

fn detail_of(events: &[Event]) -> LabelDetail {
    events
        .iter()
        .find_map(|event| match event {
            Event::LabelRead { detail, .. } => Some(detail.clone()),
            _ => None,
        })
        .expect("a label read")
}

/// Stores one thread of the repository's store and returns its uid.
fn thread(root: &Path, session: &str) -> String {
    let mut store = ThreadStore::open(&root.join(".git")).unwrap();
    let batch = Batch {
        harness: "claude-code".into(),
        session_id: session.into(),
        worktree: root.to_string_lossy().into_owned(),
        branch: Some("main".into()),
        head: None,
        model: None,
        harness_version: None,
        transcript: None,
        cursor: None,
        started_at: 1_000,
        at: 2_000,
        ended: None,
        entries: Vec::new(),
        gaps: Vec::new(),
    };
    store.apply(&batch).unwrap().uid
}

fn list_threads(engine: &mut Engine) -> Vec<gm_schema::threads::ThreadSummary> {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ListThreads {
            filter: gm_schema::threads::ThreadFilter::default(),
            before_at: None,
            before_uid: None,
            limit: 50,
        },
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::ThreadsListed { threads, .. } => Some(threads.clone()),
            _ => None,
        })
        .expect("threads listed")
}

#[test]
fn an_agent_creates_and_attaches_and_the_row_shows_the_agent() {
    let repo = repo();
    let uid = thread(&repo.root, "s1");
    let mut engine = engine_on(&repo.root);

    let created = send(
        &mut engine,
        &agent(),
        CommandKind::CreateLabel {
            name: "flaky".into(),
            description: "fails sometimes".into(),
            color: Some("#FF8800".into()),
        },
    );
    assert_eq!(state_of(&created), OperationState::Done);
    let label = label_of(&created).expect("the label");
    assert_eq!(label.origin, "agent:claude-code");
    assert_eq!(label.color.as_deref(), Some("#ff8800"));

    let attached = send(
        &mut engine,
        &agent(),
        CommandKind::AttachLabel {
            label: "flaky".into(),
            kind: AttachKind::Thread,
            reference: uid.clone(),
        },
    );
    assert_eq!(state_of(&attached), OperationState::Done);
    assert!(
        attached
            .iter()
            .any(|event| matches!(event, Event::ThreadsChanged))
    );

    let detail = detail_of(&send(
        &mut engine,
        &agent(),
        CommandKind::ReadLabel { id: label.id },
    ));
    assert_eq!(detail.attachments.len(), 1);
    assert_eq!(detail.attachments[0].origin, "agent:claude-code");
    assert_eq!(detail.attachments[0].reference, uid);

    let listed = list_threads(&mut engine);
    assert_eq!(listed[0].tags, ["flaky"]);
}

#[test]
fn record_commands_are_logged_with_their_actor_and_reads_are_not() {
    let repo = repo();
    let uid = thread(&repo.root, "s1");
    let mut engine = engine_on(&repo.root);
    send(
        &mut engine,
        &agent(),
        CommandKind::AttachLabel {
            label: "bug".into(),
            kind: AttachKind::Thread,
            reference: uid.clone(),
        },
    );
    send(
        &mut engine,
        &Actor::Human,
        CommandKind::CreateLabel {
            name: "api".into(),
            description: String::new(),
            color: None,
        },
    );
    send(
        &mut engine,
        &agent(),
        CommandKind::ListLabels {
            query: String::new(),
            limit: 10,
        },
    );
    send(&mut engine, &agent(), CommandKind::ReadLabel { id: 1 });

    let logged: Vec<(Actor, String)> = engine
        .log()
        .iter()
        .map(|entry| (entry.actor.clone(), format!("{:?}", entry.kind)))
        .filter(|(_, kind)| kind.starts_with("AttachLabel") || kind.starts_with("CreateLabel"))
        .collect();
    assert!(logged.iter().any(|(actor, _)| *actor == agent()));
    assert!(logged.iter().any(|(actor, _)| *actor == Actor::Human));
    assert!(
        engine.log().iter().all(|entry| !matches!(
            entry.kind,
            CommandKind::ListLabels { .. } | CommandKind::ReadLabel { .. }
        )),
        "reads are not in the command log"
    );
}

#[test]
fn human_only_commands_refuse_an_agent_and_change_nothing() {
    let repo = repo();
    let uid = thread(&repo.root, "s1");
    let mut engine = engine_on(&repo.root);
    let label = label_of(&send(
        &mut engine,
        &Actor::Human,
        CommandKind::CreateLabel {
            name: "bug".into(),
            description: String::new(),
            color: None,
        },
    ))
    .expect("created");
    send(
        &mut engine,
        &Actor::Human,
        CommandKind::AttachLabel {
            label: "bug".into(),
            kind: AttachKind::Thread,
            reference: uid.clone(),
        },
    );
    let other = thread(&repo.root, "s2");
    send(
        &mut engine,
        &Actor::Human,
        CommandKind::LinkThreads {
            from: uid.clone(),
            to: other.clone(),
            kind: ThreadRelKind::Relates,
        },
    );

    let refusals = [
        (
            CommandKind::RenameLabel {
                id: label.id,
                name: "renamed".into(),
            },
            "only a human can rename a label",
        ),
        (
            CommandKind::DeleteLabel { id: label.id },
            "only a human can delete a label",
        ),
        (
            CommandKind::DetachLabel {
                label: "bug".into(),
                kind: AttachKind::Thread,
                reference: uid.clone(),
            },
            "only a human can detach a label",
        ),
        (
            CommandKind::RemoveThreadRelation {
                from: uid.clone(),
                to: other.clone(),
                kind: ThreadRelKind::Relates,
            },
            "only a human can remove a thread link",
        ),
    ];
    for (kind, message) in refusals {
        let events = send(&mut engine, &agent(), kind);
        assert_eq!(state_of(&events), OperationState::Failed);
        assert_eq!(failure(&events).as_deref(), Some(message));
    }
    let detail = detail_of(&send(
        &mut engine,
        &Actor::Human,
        CommandKind::ReadLabel { id: label.id },
    ));
    assert_eq!(detail.label.name, "bug", "the agent did not rename it");
    assert_eq!(detail.attachments.len(), 1, "the agent did not detach it");
    assert_eq!(list_threads(&mut engine).len(), 2);
}

#[test]
fn a_human_removes_what_an_agent_wrote_and_deleting_a_label_cascades() {
    let repo = repo();
    let uid = thread(&repo.root, "s1");
    let mut engine = engine_on(&repo.root);
    send(
        &mut engine,
        &agent(),
        CommandKind::AttachLabel {
            label: "flaky".into(),
            kind: AttachKind::Thread,
            reference: uid.clone(),
        },
    );
    let id = detail_of(&send(
        &mut engine,
        &agent(),
        CommandKind::ReadLabel { id: 1 },
    ))
    .label
    .id;

    let detached = send(
        &mut engine,
        &Actor::Human,
        CommandKind::DetachLabel {
            label: "flaky".into(),
            kind: AttachKind::Thread,
            reference: uid.clone(),
        },
    );
    assert_eq!(state_of(&detached), OperationState::Done);
    assert!(list_threads(&mut engine)[0].tags.is_empty());

    send(
        &mut engine,
        &agent(),
        CommandKind::AttachLabel {
            label: "flaky".into(),
            kind: AttachKind::Thread,
            reference: uid.clone(),
        },
    );
    let deleted = send(&mut engine, &Actor::Human, CommandKind::DeleteLabel { id });
    assert_eq!(state_of(&deleted), OperationState::Done);
    assert!(list_threads(&mut engine)[0].tags.is_empty());
    let gone = send(&mut engine, &Actor::Human, CommandKind::ReadLabel { id });
    assert_eq!(failure(&gone).as_deref(), Some("Label not found."));
}

#[test]
fn attaching_twice_gives_one_row_and_names_are_unique() {
    let repo = repo();
    let uid = thread(&repo.root, "s1");
    let mut engine = engine_on(&repo.root);
    for actor in [Actor::Human, agent()] {
        let events = send(
            &mut engine,
            &actor,
            CommandKind::AttachLabel {
                label: "bug".into(),
                kind: AttachKind::Thread,
                reference: uid.clone(),
            },
        );
        assert_eq!(state_of(&events), OperationState::Done);
    }
    let first = send(
        &mut engine,
        &Actor::Human,
        CommandKind::CreateLabel {
            name: "bug".into(),
            description: "again".into(),
            color: None,
        },
    );
    let second = send(
        &mut engine,
        &agent(),
        CommandKind::CreateLabel {
            name: "bug".into(),
            description: String::new(),
            color: None,
        },
    );
    assert_eq!(label_of(&first), label_of(&second));
    let id = label_of(&first).expect("label").id;
    let detail = detail_of(&send(
        &mut engine,
        &Actor::Human,
        CommandKind::ReadLabel { id },
    ));
    assert_eq!(detail.attachments.len(), 1);
    assert_eq!(
        detail.attachments[0].origin, "human",
        "the first origin stays"
    );
}

#[test]
fn invalid_labels_are_refused_with_the_reason() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let events = send(
        &mut engine,
        &agent(),
        CommandKind::CreateLabel {
            name: "c".into(),
            description: String::new(),
            color: Some("red".into()),
        },
    );
    assert_eq!(state_of(&events), OperationState::Failed);
    assert_eq!(
        failure(&events).as_deref(),
        Some("a label colour is #rrggbb")
    );
}

#[test]
fn a_commit_label_keeps_the_snapshot_git_gave() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let head = git(&repo.root, &["rev-parse", "HEAD"]);
    let events = send(
        &mut engine,
        &agent(),
        CommandKind::AttachLabel {
            label: "regression".into(),
            kind: AttachKind::Commit,
            reference: "HEAD".into(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Done);
    let detail = detail_of(&send(
        &mut engine,
        &agent(),
        CommandKind::ReadLabel {
            id: label_of(&events).expect("label").id,
        },
    ));
    let attachment = &detail.attachments[0];
    assert_eq!(attachment.reference, head);
    assert_eq!(attachment.subject.as_deref(), Some("fix the parser"));
    assert_eq!(attachment.author.as_deref(), Some("t"));
    assert_eq!(attachment.committed_at, Some(COMMITTED_AT));
}

#[test]
fn an_unknown_commit_is_refused() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let events = send(
        &mut engine,
        &agent(),
        CommandKind::AttachLabel {
            label: "x".into(),
            kind: AttachKind::Commit,
            reference: "0000000000000000000000000000000000000001".into(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Failed);
}

#[test]
fn a_commit_link_without_a_snapshot_gets_one_when_the_store_opens() {
    let repo = repo();
    let head = git(&repo.root, &["rev-parse", "HEAD"]);
    let uid = thread(&repo.root, "s1");
    {
        let mut store = ThreadStore::open(&repo.root.join(".git")).unwrap();
        store
            .link(
                &uid,
                &gm_schema::threads::ThreadLink {
                    member: None,
                    kind: gm_schema::threads::LinkKind::Commit,
                    reference: head.clone(),
                    origin: gm_schema::threads::LinkOrigin::Observed,
                    resolved: None,
                    state: None,
                },
            )
            .unwrap();
    }
    let mut engine = engine_on(&repo.root);
    // Any thread command opens the store of the repository.
    list_threads(&mut engine);
    let store = ThreadStore::open(&repo.root.join(".git")).unwrap();
    assert!(
        store.commit_refs_without_snapshot().unwrap().is_empty(),
        "the open took the snapshot"
    );
    let subject: String = rusqlite_subject(&repo.root, &head);
    assert_eq!(subject, "fix the parser");
}

/// The batched read names what Git knows and leaves out what it does not.
#[test]
fn the_batched_snapshot_read_skips_names_git_does_not_know() {
    let repo = repo();
    let head = git(&repo.root, &["rev-parse", "HEAD"]);
    let tree = git(&repo.root, &["rev-parse", "HEAD^{tree}"]);
    let adapter = SubprocessGit {
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let names = vec![
        "0000000000000000000000000000000000000001".to_owned(),
        tree,
        "HEAD".to_owned(),
    ];
    let found = gm_core::GitPort::commit_snapshots(&adapter, &repo.root, &names).unwrap();
    assert_eq!(found.len(), 1, "the tree and the unknown id are left out");
    assert_eq!(found[0].0, "HEAD");
    assert_eq!(found[0].1.id, head);
    assert_eq!(found[0].1.subject, "fix the parser");
    assert_eq!(found[0].1.author, "t");
    assert_eq!(found[0].1.committed_at, COMMITTED_AT);
}

fn rusqlite_subject(root: &Path, commit: &str) -> String {
    let conn = rusqlite::Connection::open(root.join(".git/gitmaster/threads.db")).unwrap();
    conn.query_row(
        "SELECT subject FROM thread_link WHERE ref = ?1",
        [commit],
        |row| row.get(0),
    )
    .unwrap()
}

/// Reads thread `uid` from its first entry, as the Threads page does when it opens a thread.
fn read_detail(engine: &mut Engine, uid: &str) -> ThreadDetail {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ReadThread {
            uid: uid.into(),
            from_seq: 0,
            limit: 50,
        },
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::ThreadRead { detail, .. } => Some((**detail).clone()),
            _ => None,
        })
        .expect("a thread read")
}

#[test]
fn a_thread_detail_carries_its_links_with_direction_kind_and_origin() {
    let repo = repo();
    let a = thread(&repo.root, "a");
    let b = thread(&repo.root, "b");
    let c = thread(&repo.root, "c");
    let mut engine = engine_on(&repo.root);
    let linked = send(
        &mut engine,
        &agent(),
        CommandKind::LinkThreads {
            from: a.clone(),
            to: b.clone(),
            kind: ThreadRelKind::Continues,
        },
    );
    assert_eq!(state_of(&linked), OperationState::Done);
    let by_human = send(
        &mut engine,
        &Actor::Human,
        CommandKind::LinkThreads {
            from: c.clone(),
            to: a.clone(),
            kind: ThreadRelKind::Relates,
        },
    );
    assert_eq!(state_of(&by_human), OperationState::Done);

    // The thread that continues the other one: the link points out of it, and the agent made it.
    let from_a = read_detail(&mut engine, &a);
    assert_eq!(from_a.relations.len(), 2, "{:?}", from_a.relations);
    let continues = from_a
        .relations
        .iter()
        .find(|relation| relation.kind == ThreadRelKind::Continues)
        .expect("the continues link");
    assert_eq!(continues.from, a);
    assert_eq!(continues.to, b);
    assert_eq!(continues.origin, "agent:claude-code");
    assert!(continues.at > 0);

    // The same link read from the other end points in: the thread is continued by `a`.
    let from_b = read_detail(&mut engine, &b);
    assert_eq!(from_b.relations.len(), 1, "{:?}", from_b.relations);
    assert_eq!(from_b.relations[0].from, a);
    assert_eq!(from_b.relations[0].to, b);
    assert_eq!(from_b.relations[0].kind, ThreadRelKind::Continues);
    assert_eq!(from_b.relations[0].origin, "agent:claude-code");

    // A link a human made to `a` points in, and is marked as the human's.
    let relates = from_a
        .relations
        .iter()
        .find(|relation| relation.kind == ThreadRelKind::Relates)
        .expect("the relates link");
    assert_eq!(relates.from, c);
    assert_eq!(relates.to, a);
    assert_eq!(relates.origin, "human");

    let removed = send(
        &mut engine,
        &Actor::Human,
        CommandKind::RemoveThreadRelation {
            from: a.clone(),
            to: b.clone(),
            kind: ThreadRelKind::Continues,
        },
    );
    assert_eq!(state_of(&removed), OperationState::Done);
    assert!(read_detail(&mut engine, &b).relations.is_empty());
}

#[test]
fn thread_links_are_recorded_by_an_agent_and_removed_by_a_human() {
    let repo = repo();
    let a = thread(&repo.root, "a");
    let b = thread(&repo.root, "b");
    let mut engine = engine_on(&repo.root);
    let linked = send(
        &mut engine,
        &agent(),
        CommandKind::LinkThreads {
            from: a.clone(),
            to: b.clone(),
            kind: ThreadRelKind::Continues,
        },
    );
    assert_eq!(state_of(&linked), OperationState::Done);
    let relation = linked
        .iter()
        .find_map(|event| match event {
            Event::ThreadRelationChanged {
                relation, removed, ..
            } => {
                assert!(!removed);
                Some(relation.clone())
            }
            _ => None,
        })
        .expect("the relation");
    assert_eq!(relation.origin, "agent:claude-code");
    assert_eq!(relation.from, a);
    let removed = send(
        &mut engine,
        &Actor::Human,
        CommandKind::RemoveThreadRelation {
            from: a.clone(),
            to: b.clone(),
            kind: ThreadRelKind::Continues,
        },
    );
    assert_eq!(state_of(&removed), OperationState::Done);
    assert!(
        removed
            .iter()
            .any(|event| matches!(event, Event::ThreadRelationChanged { removed: true, .. }))
    );
    let again = send(
        &mut engine,
        &Actor::Human,
        CommandKind::RemoveThreadRelation {
            from: a,
            to: b,
            kind: ThreadRelKind::Continues,
        },
    );
    assert_eq!(state_of(&again), OperationState::Failed);
}

#[test]
fn a_thread_cannot_be_linked_to_itself() {
    let repo = repo();
    let a = thread(&repo.root, "a");
    let mut engine = engine_on(&repo.root);
    let events = send(
        &mut engine,
        &agent(),
        CommandKind::LinkThreads {
            from: a.clone(),
            to: a,
            kind: ThreadRelKind::Relates,
        },
    );
    assert_eq!(state_of(&events), OperationState::Failed);
}

#[test]
fn a_file_attachment_shows_the_path_the_file_has_now() {
    let repo = repo();
    std::fs::write(repo.root.join("keep.rs"), "fn keep() {}\n").unwrap();
    git(&repo.root, &["add", "--", "keep.rs"]);
    git(&repo.root, &["commit", "-q", "-m", "keep"]);
    let mut engine = engine_on(&repo.root);
    let attached = send(
        &mut engine,
        &Actor::Human,
        CommandKind::AttachLabel {
            label: "parsing".into(),
            kind: AttachKind::File,
            reference: "parser.rs".into(),
        },
    );
    assert_eq!(state_of(&attached), OperationState::Done, "{attached:?}");
    let label = label_of(&attached).expect("the label");
    let kept = send(
        &mut engine,
        &Actor::Human,
        CommandKind::AttachLabel {
            label: "parsing".into(),
            kind: AttachKind::File,
            reference: "keep.rs".into(),
        },
    );
    assert_eq!(state_of(&kept), OperationState::Done, "{kept:?}");

    std::fs::create_dir(repo.root.join("src")).unwrap();
    git(&repo.root, &["mv", "parser.rs", "src/parser.rs"]);
    git(&repo.root, &["commit", "-q", "-m", "move the parser"]);

    let read = send(
        &mut engine,
        &Actor::Human,
        CommandKind::ReadLabel { id: label.id },
    );
    let detail = detail_of(&read);
    let file = |path: &str| {
        detail
            .attachments
            .iter()
            .find(|attachment| attachment.reference == path)
            .unwrap_or_else(|| panic!("no attachment of {path}"))
    };
    // The stored path is kept; the path now comes beside it.
    assert_eq!(
        file("parser.rs").current_path.as_deref(),
        Some("src/parser.rs")
    );
    assert_eq!(file("keep.rs").current_path.as_deref(), Some("keep.rs"));
}

/// The hits, facets and diagnosis of one search.
type Searched = (
    Vec<ThreadHit>,
    Option<SearchFacets>,
    Option<Vec<(String, u32)>>,
);

fn search(engine: &mut Engine, filter: ThreadFilter, facets: bool) -> Searched {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::SearchThreads {
            query: String::new(),
            filter,
            limit: 50,
            cursor: None,
            group: SearchGroup::Thread,
            order: SearchOrder::Relevance,
            context: 0,
            facets,
        },
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::ThreadSearchResults {
                hits,
                facets,
                diagnosis,
                ..
            } => Some((hits.clone(), facets.clone(), diagnosis.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no search answer: {events:?}"))
}

fn uids_of(hits: &[ThreadHit]) -> Vec<String> {
    let mut uids: Vec<String> = hits.iter().map(|hit| hit.thread_uid.clone()).collect();
    uids.sort();
    uids
}

fn with(edit: impl FnOnce(&mut ThreadFilter)) -> ThreadFilter {
    let mut filter = ThreadFilter::default();
    edit(&mut filter);
    filter
}

#[test]
fn search_filters_by_labels_and_related_threads_and_names_the_filter_that_emptied_it() {
    let repo = repo();
    let one = thread(&repo.root, "one");
    let two = thread(&repo.root, "two");
    let three = thread(&repo.root, "three");
    let mut engine = engine_on(&repo.root);
    for (name, uid) in [("bug", &one), ("bug", &two), ("urgent", &two)] {
        let events = send(
            &mut engine,
            &Actor::Human,
            CommandKind::AttachLabel {
                label: name.into(),
                kind: AttachKind::Thread,
                reference: uid.clone(),
            },
        );
        assert_eq!(state_of(&events), OperationState::Done, "{events:?}");
    }
    for (from, to, kind) in [
        (&one, &three, ThreadRelKind::Relates),
        (&two, &one, ThreadRelKind::Continues),
    ] {
        let events = send(
            &mut engine,
            &Actor::Human,
            CommandKind::LinkThreads {
                from: from.clone(),
                to: to.clone(),
                kind,
            },
        );
        assert_eq!(state_of(&events), OperationState::Done, "{events:?}");
    }

    // Every label named must be on the thread.
    let both = with(|f| f.label = Some(vec!["bug".into(), "urgent".into()]));
    assert_eq!(
        uids_of(&search(&mut engine, both, false).0),
        vec![two.clone()]
    );
    let bugs = with(|f| f.label = Some(vec!["bug".into()]));
    let mut expected = vec![one.clone(), two.clone()];
    expected.sort();
    assert_eq!(uids_of(&search(&mut engine, bugs, false).0), expected);
    // A label name is exact, not a prefix.
    let prefix = with(|f| f.label = Some(vec!["bu".into()]));
    assert!(search(&mut engine, prefix, false).0.is_empty());

    // Linked in either direction, any kind, or only the kind asked for.
    let related = with(|f| f.related_to = Some(one.clone()));
    let mut expected = vec![two.clone(), three.clone()];
    expected.sort();
    assert_eq!(uids_of(&search(&mut engine, related, false).0), expected);
    let continues = with(|f| {
        f.related_to = Some(one.clone());
        f.related_kind = Some(ThreadRelKind::Continues);
    });
    assert_eq!(
        uids_of(&search(&mut engine, continues, false).0),
        vec![two.clone()]
    );

    // Nothing is supersedes-linked to `one`, so the search finds nothing, and the diagnosis
    // names the filter that, removed alone, gives threads back.
    let none = with(|f| {
        f.related_to = Some(one.clone());
        f.related_kind = Some(ThreadRelKind::Supersedes);
    });
    let (hits, _, diagnosis) = search(&mut engine, none, false);
    assert!(hits.is_empty());
    assert_eq!(diagnosis, Some(vec![("related_to".to_owned(), 3)]));

    let (_, facets, _) = search(&mut engine, ThreadFilter::default(), true);
    let facets = facets.expect("facets");
    assert!(facets.label.contains(&("bug".to_owned(), 2)), "{facets:?}");
    assert!(
        facets.label.contains(&("urgent".to_owned(), 1)),
        "{facets:?}"
    );
}

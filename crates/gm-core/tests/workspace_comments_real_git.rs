//! Review comments that a workspace holds for its members, seen from a member's repository
//! (D174, D182, D200), against real repositories in temporary directories. A workspace is a
//! repository whose members are linked worktrees of the user's clones. A comment written in the
//! workspace on `member/path` is in the workspace's `review.db`, and the member's own repository
//! lists it, acts on it by its composite id, and never sees another member's or the workspace's
//! own comments. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_core::workspace_cli::run_with;
use gm_core::workspace_comments::{decode, encode};
use gm_core::worktree_cli::Env;
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, ReviewAuthorFilter,
    ReviewComment, ReviewKind, ReviewSeverity, ReviewStatus,
};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A clone `base/<name>` on `main` with `f.txt` ("one", "two", "three"), and a `feat` branch
/// that changes `f.txt` to "one", "three", left for a member to take.
fn clone(base: &Path, name: &str) -> PathBuf {
    let origin = base.join(format!("{name}.git"));
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare", "-b", "main"]);
    let repo = base.join(name);
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("f.txt"), "one\nthree\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "feat change"]);
    git(&repo, &["checkout", "-q", "main"]);
    repo
}

/// A workspace `pay` with members `api` and `web` (linked worktrees of two clones on `feat`),
/// and the user's configuration in a scratch folder.
struct Fixture {
    _dir: tempfile::TempDir,
    base: PathBuf,
    identity: PathBuf,
    /// The main checkouts of the two clones, on `main`.
    api: PathBuf,
    web: PathBuf,
    /// The workspace `pay`, and the members: the worktrees of the clones, inside it.
    ws: PathBuf,
    member_api: PathBuf,
    member_web: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let identity = base.join("identity.gitconfig");
        std::fs::write(
            &identity,
            "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n",
        )
        .unwrap();
        let api = clone(&base, "api");
        let web = clone(&base, "web");
        let mut fixture = Self {
            _dir: dir,
            base,
            identity,
            api,
            web,
            ws: PathBuf::new(),
            member_api: PathBuf::new(),
            member_web: PathBuf::new(),
        };
        fixture.ws = fixture.create("pay");
        fixture.member_api = fixture.add(&fixture.ws.clone(), &fixture.api.clone(), "feat");
        fixture.member_web = fixture.add(&fixture.ws.clone(), &fixture.web.clone(), "feat");
        fixture
    }

    fn env(&self) -> Env {
        Env {
            cwd: self.base.clone(),
            home: Some(self.base.join("home")),
            worktrees_home: None,
        }
    }

    /// Runs `gitmaster workspace <words>` from the base folder, asserting success.
    fn run(&self, words: &[&str]) -> String {
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        let git = SubprocessGit {
            global_config: Some(self.identity.clone()),
            ..SubprocessGit::default()
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(
            &words,
            &self.env(),
            &git,
            Some(&self.identity),
            &mut "".as_bytes(),
            &mut out,
            &mut err,
        );
        assert_eq!(
            code,
            0,
            "{words:?} failed: {}",
            String::from_utf8_lossy(&err)
        );
        String::from_utf8(out).unwrap()
    }

    /// `create <name>` under the fixture's root.
    fn create(&self, name: &str) -> PathBuf {
        let root = self.base.join("workspaces");
        let out = self.run(&["create", name, "--root", root.to_str().unwrap()]);
        let dir = root.join(name);
        assert_eq!(out, format!("{}\n", dir.display()));
        dir
    }

    /// `add <repo> <branch>` into `ws`; returns the member folder.
    fn add(&self, ws: &Path, repo: &Path, branch: &str) -> PathBuf {
        let out = self.run(&[
            "add",
            repo.to_str().unwrap(),
            branch,
            "--workspace",
            ws.to_str().unwrap(),
        ]);
        PathBuf::from(out.trim_end())
    }
}

/// An engine with `dir` open.
struct Session {
    engine: Engine,
    next: u64,
}

impl Session {
    fn open(dir: &Path) -> Self {
        let engine = Engine::new(Box::new(SubprocessGit::default()));
        let mut session = Self { engine, next: 1 };
        let events = session.human(CommandKind::OpenRepo {
            path: dir.to_string_lossy().into_owned(),
        });
        assert!(done(&events), "open failed: {}", failure(&events));
        session
    }

    fn run(&mut self, actor: Actor, kind: CommandKind) -> Vec<Event> {
        self.next += 1;
        self.engine
            .handle(OperationId(self.next), &Command { actor, kind })
    }

    fn human(&mut self, kind: CommandKind) -> Vec<Event> {
        self.run(Actor::Human, kind)
    }

    fn agent(&mut self, kind: CommandKind) -> Vec<Event> {
        self.run(
            Actor::Agent {
                name: "test-agent".to_owned(),
            },
            kind,
        )
    }

    /// The comments this repository lists for `author`.
    fn list(&mut self, author: ReviewAuthorFilter) -> Vec<ReviewComment> {
        let events = self.human(CommandKind::ListReviewComments { author });
        listed(&events)
    }

    /// The comments the listing holds on `path`, by id.
    fn ids_on(&mut self, author: ReviewAuthorFilter, path: &str) -> Vec<i64> {
        self.list(author)
            .into_iter()
            .filter(|comment| comment.path == path)
            .map(|comment| comment.id)
            .collect()
    }
}

fn done(events: &[Event]) -> bool {
    state_of(events) == Some(OperationState::Done)
}

fn state_of(events: &[Event]) -> Option<OperationState> {
    events.iter().rev().find_map(|event| match event {
        Event::OperationChanged { state, .. } if state.is_terminal() => Some(*state),
        _ => None,
    })
}

fn failure(events: &[Event]) -> String {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::OperationChanged {
                state: OperationState::Failed,
                detail,
                ..
            } => detail.clone(),
            _ => None,
        })
        .unwrap_or_default()
}

fn listed(events: &[Event]) -> Vec<ReviewComment> {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::ReviewCommentsListed { comments, .. } => Some(comments.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no comment listing in {events:?}"))
}

/// The one comment a write answers with on `path`, by id.
fn id_of(events: &[Event], path: &str) -> i64 {
    assert!(done(events), "{}", failure(events));
    listed(events)
        .into_iter()
        .find(|comment| comment.path == path)
        .unwrap_or_else(|| panic!("no comment on {path} in {events:?}"))
        .id
}

fn agent_note(path: &str, anchor: &str, body: &str) -> CommandKind {
    CommandKind::CreateAgentReviewComment {
        path: path.to_owned(),
        anchor_text: anchor.to_owned(),
        kind: Some(ReviewKind::Note),
        body: body.to_owned(),
        severity: None::<ReviewSeverity>,
        label: None,
        decorations: Vec::new(),
    }
}

fn human_text(path: &str, text: &str, body: &str) -> CommandKind {
    CommandKind::AddReviewCommentByText {
        label: None,
        path: path.to_owned(),
        text: text.to_owned(),
        body: body.to_owned(),
    }
}

/// The workspace writes a note by the agent and a comment by the person on `api/f.txt`, and
/// returns their ids in the workspace's own store: (agent, person).
fn seed(fixture: &Fixture) -> (i64, i64) {
    let mut ws = Session::open(&fixture.ws);
    let agent = id_of(
        &ws.agent(agent_note("api/f.txt", "three", "three is read once")),
        "api/f.txt",
    );
    let events = ws.human(human_text("api/f.txt", "one", "is one the first line?"));
    assert!(done(&events), "{}", failure(&events));
    let person = listed(&events)
        .into_iter()
        .find(|comment| comment.author.is_none() && comment.path == "api/f.txt")
        .expect("the person's comment")
        .id;
    (agent, person)
}

/// The workspace slot of a composite id a member sees.
fn slot_of(id: i64) -> u32 {
    decode(id).0
}

#[test]
fn a_member_sees_the_comments_its_workspace_holds_on_its_files_under_composite_ids() {
    let fixture = Fixture::new();
    let (agent, person) = seed(&fixture);
    // The person's comment is on "one": the member file reads "one" and "three", so both are
    // located. The agent's note is on "three".
    let mut member = Session::open(&fixture.member_api);
    let seen = member.list(ReviewAuthorFilter::Any);
    assert_eq!(seen.len(), 2, "{seen:?}");
    for comment in &seen {
        assert_eq!(comment.path, "f.txt");
        assert_eq!(comment.workspace.as_deref(), Some("pay"));
        assert!(!comment.outdated, "{comment:?}");
        assert!(comment.location.is_some(), "{comment:?}");
        assert!(slot_of(comment.id) >= 1, "composite: {}", comment.id);
    }
    let note = seen.iter().find(|c| c.kind.is_some()).expect("the note");
    assert_eq!(note.author.as_deref(), Some("agent:test-agent"));
    let slot = slot_of(seen[0].id);
    assert_eq!(note.id, encode(slot, agent));
    let by_person = seen
        .iter()
        .find(|c| c.kind.is_none())
        .expect("the person's");
    assert_eq!(by_person.id, encode(slot, person));

    // The default listing holds the person's comment, and the agent's note only when the
    // person replies to it (D200).
    assert_eq!(member.ids_on(ReviewAuthorFilter::Human, "f.txt").len(), 1);

    // The clone's main checkout is another repository of the same clone: the same comments,
    // with the same ids and the same workspace.
    let mut clone = Session::open(&fixture.api);
    let from_clone = clone.list(ReviewAuthorFilter::Any);
    assert_eq!(from_clone.len(), 2, "{from_clone:?}");
    let mut ids: Vec<i64> = seen.iter().map(|c| c.id).collect();
    let mut clone_ids: Vec<i64> = from_clone.iter().map(|c| c.id).collect();
    ids.sort_unstable();
    clone_ids.sort_unstable();
    assert_eq!(ids, clone_ids);
    assert!(
        from_clone
            .iter()
            .all(|c| c.path == "f.txt" && c.workspace.as_deref() == Some("pay"))
    );
}

#[test]
fn an_edit_of_the_members_own_checkout_makes_the_comment_outdated_there_only() {
    let fixture = Fixture::new();
    seed(&fixture);
    // "one" and "three" are gone from the member's own checkout, so both comments are outdated
    // there.
    std::fs::write(fixture.member_api.join("f.txt"), "four\nfive\n").unwrap();

    let mut member = Session::open(&fixture.member_api);
    let seen = member.list(ReviewAuthorFilter::Any);
    assert_eq!(seen.len(), 2);
    assert!(
        seen.iter().all(|c| c.outdated && c.location.is_none()),
        "{seen:?}"
    );

    // The clone's main checkout has its own "three": the comments are located there.
    let mut clone = Session::open(&fixture.api);
    let from_clone = clone.list(ReviewAuthorFilter::Any);
    assert!(from_clone.iter().all(|c| !c.outdated), "{from_clone:?}");
}

#[test]
fn another_member_and_the_workspace_own_files_are_not_listed() {
    let fixture = Fixture::new();
    seed(&fixture);
    let mut ws = Session::open(&fixture.ws);
    std::fs::write(fixture.ws.join("README.md"), "hello\n").unwrap();
    let events = ws.human(human_text("web/f.txt", "three", "web is read once"));
    assert!(done(&events), "{}", failure(&events));
    let events = ws.human(human_text("README.md", "hello", "a workspace file"));
    assert!(done(&events), "{}", failure(&events));

    let mut api = Session::open(&fixture.member_api);
    let on_api = api.list(ReviewAuthorFilter::Any);
    assert_eq!(on_api.len(), 2, "{on_api:?}");
    assert!(on_api.iter().all(|c| c.path == "f.txt"));

    let mut web = Session::open(&fixture.member_web);
    let on_web = web.list(ReviewAuthorFilter::Any);
    assert_eq!(on_web.len(), 1, "{on_web:?}");
    assert_eq!(on_web[0].path, "f.txt");
    assert_eq!(on_web[0].body, "web is read once");
}

#[test]
fn the_member_hands_off_the_person_comment_and_the_workspace_sees_it_handled() {
    let fixture = Fixture::new();
    let (_, person) = seed(&fixture);
    let mut member = Session::open(&fixture.member_api);
    let composite = member
        .list(ReviewAuthorFilter::Any)
        .into_iter()
        .find(|c| c.kind.is_none())
        .expect("the person's comment")
        .id;
    assert_ne!(composite, person, "a member sees the composite id");

    let events = member.agent(CommandKind::HandleReviewComment {
        id: composite,
        summary: "reads the first line".to_owned(),
    });
    assert!(done(&events), "{}", failure(&events));
    let after = listed(&events)
        .into_iter()
        .find(|c| c.id == composite)
        .expect("the handed-off comment");
    assert_eq!(after.status, ReviewStatus::Handled);
    assert_eq!(after.replies.last().unwrap().body, "reads the first line");

    // The workspace's own store holds it as handled, with the member's reply.
    let mut ws = Session::open(&fixture.ws);
    let stored = ws
        .list(ReviewAuthorFilter::Any)
        .into_iter()
        .find(|c| c.id == person)
        .expect("the comment in the workspace");
    assert_eq!(stored.status, ReviewStatus::Handled);
    assert_eq!(stored.replies.last().unwrap().author, "agent:test-agent");
    assert!(
        stored.after_snapshot.is_some(),
        "the after snapshot is in the workspace"
    );

    // The member's change of the comment is read from the workspace's snapshots.
    let events = member.run(
        Actor::Human,
        CommandKind::LoadReviewChange { id: composite },
    );
    let change = events.iter().find_map(|event| match event {
        Event::ReviewChange {
            change, comment, ..
        } if *comment == composite => Some(change.clone()),
        _ => None,
    });
    assert!(
        change.expect("a change event").is_some(),
        "the change is observed"
    );

    // The person resolves it from the member's repository, as the person does.
    let events = member.human(CommandKind::ResolveReviewComment { id: composite });
    assert!(done(&events), "{}", failure(&events));
    let mut ws = Session::open(&fixture.ws);
    let stored = ws
        .list(ReviewAuthorFilter::Any)
        .into_iter()
        .find(|c| c.id == person)
        .unwrap();
    assert_eq!(stored.status, ReviewStatus::Resolved);
}

#[test]
fn a_member_reply_lands_in_the_workspace_store_and_is_listed_under_a_composite_id() {
    let fixture = Fixture::new();
    let (agent, _) = seed(&fixture);
    let mut member = Session::open(&fixture.member_api);
    let thread = member
        .list(ReviewAuthorFilter::Any)
        .into_iter()
        .find(|c| c.kind.is_some())
        .expect("the agent's note")
        .id;
    let events = member.agent(CommandKind::ReplyToReviewComment {
        id: thread,
        body: "still read once".to_owned(),
    });
    assert!(done(&events), "{}", failure(&events));
    let reply = listed(&events)
        .into_iter()
        .find(|c| c.id == thread)
        .unwrap()
        .replies
        .last()
        .unwrap()
        .clone();
    assert!(
        slot_of(reply.id) >= 1,
        "the reply id is composite: {}",
        reply.id
    );

    let mut ws = Session::open(&fixture.ws);
    let stored = ws
        .list(ReviewAuthorFilter::Any)
        .into_iter()
        .find(|c| c.id == agent)
        .unwrap();
    assert_eq!(stored.replies.last().unwrap().body, "still read once");
    assert_eq!(
        stored.status,
        ReviewStatus::Open,
        "a reply keeps the status"
    );
}

#[test]
fn a_member_agent_cannot_close_reopen_edit_or_delete_and_cannot_hand_off_its_own_thread() {
    let fixture = Fixture::new();
    let (agent, person) = seed(&fixture);
    let mut member = Session::open(&fixture.member_api);
    let person_id = member
        .list(ReviewAuthorFilter::Any)
        .into_iter()
        .find(|c| c.kind.is_none())
        .unwrap()
        .id;
    let note_id = member
        .list(ReviewAuthorFilter::Any)
        .into_iter()
        .find(|c| c.kind.is_some())
        .unwrap()
        .id;

    let refused = [
        CommandKind::ResolveReviewComment { id: person_id },
        CommandKind::ReopenReviewComment { id: person_id },
        CommandKind::DeleteReviewComment { id: person_id },
        CommandKind::EditReviewComment {
            id: person_id,
            body: "rewritten".to_owned(),
        },
    ];
    for kind in refused {
        let events = member.agent(kind);
        assert_eq!(state_of(&events), Some(OperationState::Failed));
        assert!(failure(&events).contains("only"), "{}", failure(&events));
    }
    let events = member.agent(CommandKind::HandleReviewComment {
        id: note_id,
        summary: "closing my own note".to_owned(),
    });
    assert_eq!(state_of(&events), Some(OperationState::Failed));
    assert!(
        failure(&events).contains("cannot hand off"),
        "{}",
        failure(&events)
    );

    // Nothing changed in the workspace.
    let mut ws = Session::open(&fixture.ws);
    let all = ws.list(ReviewAuthorFilter::Any);
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|c| c.status == ReviewStatus::Open));
    assert!(
        all.iter()
            .any(|c| c.id == person && c.body == "is one the first line?")
    );
    assert!(
        all.iter()
            .any(|c| c.id == agent && c.body == "three is read once")
    );
}

#[test]
fn an_id_of_an_unknown_slot_or_of_another_member_is_refused_as_not_here() {
    let fixture = Fixture::new();
    seed(&fixture);
    let mut ws = Session::open(&fixture.ws);
    let events = ws.human(human_text("web/f.txt", "three", "web comment"));
    assert!(done(&events), "{}", failure(&events));
    // The id the member of web sees for its comment: composite, not the workspace's own.
    let mut web = Session::open(&fixture.member_web);
    let on_web = web.list(ReviewAuthorFilter::Any)[0].id;

    let mut member = Session::open(&fixture.member_api);
    let unknown_slot = (4000_i64 << 40) + 1;
    for id in [unknown_slot, on_web] {
        let events = member.agent(CommandKind::HandleReviewComment {
            id,
            summary: "touching another member".to_owned(),
        });
        assert_eq!(state_of(&events), Some(OperationState::Failed));
        assert_eq!(failure(&events), format!("no review comment {id} here"));
    }
    let mut ws = Session::open(&fixture.ws);
    let web_comment = ws
        .list(ReviewAuthorFilter::Any)
        .into_iter()
        .find(|c| c.path == "web/f.txt")
        .unwrap();
    assert_eq!(web_comment.status, ReviewStatus::Open);
}

#[test]
fn two_workspaces_holding_the_same_clone_both_show_its_comments_under_different_slots() {
    let fixture = Fixture::new();
    seed(&fixture);
    // A second branch of the same clone, in a second workspace.
    git(&fixture.api, &["branch", "feat2", "main"]);
    let ship = fixture.create("ship");
    let ship_api = fixture.add(&ship, &fixture.api.clone(), "feat2");
    let mut ship_ws = Session::open(&ship);
    let events = ship_ws.human(human_text("api/f.txt", "two", "ship reads two"));
    assert!(done(&events), "{}", failure(&events));

    let mut member = Session::open(&fixture.member_api);
    let seen = member.list(ReviewAuthorFilter::Any);
    assert_eq!(seen.len(), 3, "{seen:?}");
    let pay: Vec<&ReviewComment> = seen
        .iter()
        .filter(|c| c.workspace.as_deref() == Some("pay"))
        .collect();
    let ship_seen: Vec<&ReviewComment> = seen
        .iter()
        .filter(|c| c.workspace.as_deref() == Some("ship"))
        .collect();
    assert_eq!(pay.len(), 2);
    assert_eq!(ship_seen.len(), 1);
    assert_ne!(slot_of(pay[0].id), slot_of(ship_seen[0].id));

    // The clone is a member of both workspaces, so its other member folder lists the comments of
    // both, each under its workspace's slot, and locates them in its own checkout.
    let mut ship_member = Session::open(&ship_api);
    let from_ship = ship_member.list(ReviewAuthorFilter::Any);
    assert_eq!(from_ship.len(), 3, "{from_ship:?}");
    assert!(from_ship.iter().any(|c| c.id == ship_seen[0].id));
    let mut ids: Vec<i64> = seen.iter().map(|c| c.id).collect();
    let mut ship_ids: Vec<i64> = from_ship.iter().map(|c| c.id).collect();
    ids.sort_unstable();
    ship_ids.sort_unstable();
    assert_eq!(ids, ship_ids);
}

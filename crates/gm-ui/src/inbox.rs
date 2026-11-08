//! The Inbox (UI redesign, slice 6): what needs the person in the current repository, in four
//! sections. Pure and derived: every item is computed from state the window already holds
//! (the status, the review comments, the workflow runs, the memories, the stacks), so an item
//! leaves the Inbox by itself once its cause is gone. Nothing is stored.

use gm_schema::{ReviewComment, ReviewStatus, StackSnapshot, StackState, StatusEntry};

use crate::action_registry::{Action, Args};

/// A section of the Inbox, in the order the page shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Section {
    NeedsYou,
    ReturnedByAgents,
    WaitingOnAgents,
    FellBehind,
}

impl Section {
    pub(crate) const ALL: [Section; 4] = [
        Section::NeedsYou,
        Section::ReturnedByAgents,
        Section::WaitingOnAgents,
        Section::FellBehind,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Section::NeedsYou => "Needs you",
            Section::ReturnedByAgents => "Returned by agents",
            Section::WaitingOnAgents => "Waiting on agents",
            Section::FellBehind => "Fell behind",
        }
    }
}

/// One item: what it is, where, when, and the registry actions it offers, primary first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    /// Stable while the cause lasts, so a selection survives a refresh.
    pub(crate) key: String,
    pub(crate) section: Section,
    pub(crate) what: String,
    /// A path or a branch.
    pub(crate) place: String,
    /// Milliseconds since the epoch, when the cause has a time.
    pub(crate) when: Option<i64>,
    pub(crate) actions: Vec<(Action, Args)>,
}

/// A workflow run waiting for the person's approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Awaiting<'a> {
    pub(crate) run: i64,
    pub(crate) workflow: &'a str,
    pub(crate) summary: &'a str,
}

/// The state the items come from.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Inputs<'a> {
    pub(crate) entries: &'a [StatusEntry],
    pub(crate) comments: Option<&'a [ReviewComment]>,
    /// Oldest first; Approve acts on the newest.
    pub(crate) awaiting: &'a [Awaiting<'a>],
    pub(crate) proposed_memories: usize,
    pub(crate) stacks: Option<&'a StackSnapshot>,
    pub(crate) branch: Option<&'a str>,
    pub(crate) upstream: Option<&'a str>,
    pub(crate) behind: u32,
    /// The conflict resolver preview is on.
    pub(crate) resolver: bool,
}

/// Whether a comment was started by an agent (D200): its root author is `agent:<client>`.
fn agent_started(comment: &ReviewComment) -> bool {
    comment
        .author
        .as_deref()
        .is_some_and(|author| author.starts_with("agent"))
}

/// The time of a comment's last word: its newest reply, else its creation.
fn last_word(comment: &ReviewComment) -> i64 {
    comment
        .replies
        .iter()
        .map(|reply| reply.created_at)
        .max()
        .unwrap_or(comment.created_at)
}

/// Every item, by section, in the order each section lists them.
#[must_use]
pub(crate) fn items(inputs: &Inputs<'_>) -> Vec<Item> {
    let mut items = Vec::new();
    needs_you(inputs, &mut items);
    from_comments(inputs, &mut items);
    fell_behind(inputs, &mut items);
    items
}

/// Conflicts and workflow approvals.
fn needs_you(inputs: &Inputs<'_>, items: &mut Vec<Item>) {
    // Needs you: conflicts, approvals, questions an agent asked.
    for entry in inputs.entries.iter().filter(|entry| entry.conflicted) {
        let file = Args::File(entry.path.clone());
        let primary = if inputs.resolver {
            Action::ResolveConflict
        } else {
            Action::ViewFile
        };
        items.push(Item {
            key: format!("conflict:{}", entry.path),
            section: Section::NeedsYou,
            what: "Conflict to resolve".to_owned(),
            place: entry.path.clone(),
            when: None,
            actions: vec![(primary, file.clone()), (Action::CopyFilePath, file)],
        });
    }
    let newest = inputs.awaiting.iter().map(|run| run.run).max();
    for run in inputs.awaiting {
        let mut actions = Vec::new();
        if Some(run.run) == newest {
            actions.push((Action::ApproveWorkflowStep, Args::None));
            actions.push((Action::RejectWorkflowStep, Args::None));
        }
        actions.push((Action::ShowWorkflows, Args::None));
        items.push(Item {
            key: format!("approval:{}", run.run),
            section: Section::NeedsYou,
            what: format!("{} waits for approval: {}", run.workflow, run.summary),
            place: inputs.branch.unwrap_or_default().to_owned(),
            when: None,
            actions,
        });
    }
}

/// The comments: an agent's questions, handled comments, proposed memories and the person's
/// open comments.
fn from_comments(inputs: &Inputs<'_>, items: &mut Vec<Item>) {
    let comments = inputs.comments.unwrap_or_default();
    for comment in comments
        .iter()
        .filter(|comment| comment.needs_answer && comment.status != ReviewStatus::Resolved)
    {
        items.push(comment_item(
            comment,
            Section::NeedsYou,
            "An agent asks a question",
        ));
    }
    // Returned by agents: handled comments and proposed memories.
    for comment in comments
        .iter()
        .filter(|comment| comment.status == ReviewStatus::Handled)
    {
        items.push(comment_item(
            comment,
            Section::ReturnedByAgents,
            "Handled by an agent",
        ));
    }
    if inputs.proposed_memories > 0 {
        let count = inputs.proposed_memories;
        items.push(Item {
            key: "memories".to_owned(),
            section: Section::ReturnedByAgents,
            what: if count == 1 {
                "1 proposed memory".to_owned()
            } else {
                format!("{count} proposed memories")
            },
            place: String::new(),
            when: None,
            actions: vec![(Action::ShowMemories, Args::None)],
        });
    }
    // Waiting on agents: the person's open comments.
    for comment in comments.iter().filter(|comment| {
        comment.status == ReviewStatus::Open && !agent_started(comment) && !comment.needs_answer
    }) {
        items.push(comment_item(
            comment,
            Section::WaitingOnAgents,
            "Waiting for an agent",
        ));
    }
}

/// The branch behind its upstream, and stack branches behind their parent or orphaned.
fn fell_behind(inputs: &Inputs<'_>, items: &mut Vec<Item>) {
    if let (Some(branch), Some(upstream)) = (inputs.branch, inputs.upstream)
        && inputs.behind > 0
    {
        items.push(Item {
            key: format!("behind:{branch}"),
            section: Section::FellBehind,
            what: format!("{} commits behind {upstream}", inputs.behind),
            place: branch.to_owned(),
            when: None,
            actions: vec![(Action::Pull, Args::None), (Action::Fetch, Args::None)],
        });
    }
    let stacked = inputs
        .stacks
        .into_iter()
        .flat_map(|snapshot| &snapshot.stacks)
        .flat_map(|stack| &stack.branches);
    for entry in stacked {
        let branch = Args::Branch(entry.name.clone());
        let (what, actions) = match entry.state {
            StackState::Stale => (
                format!("{} commits behind its parent", entry.behind),
                vec![(Action::Restack, branch)],
            ),
            StackState::ParentGone | StackState::ParentMerged => (
                if entry.state == StackState::ParentGone {
                    "Its parent is gone".to_owned()
                } else {
                    "Its parent is merged".to_owned()
                },
                vec![
                    (Action::ReparentToGrandparent, branch.clone()),
                    (Action::RemoveFromStack, branch),
                ],
            ),
            _ => continue,
        };
        items.push(Item {
            key: format!("stack:{}", entry.name),
            section: Section::FellBehind,
            what,
            place: entry.name.clone(),
            when: None,
            actions,
        });
    }
}

fn comment_item(comment: &ReviewComment, section: Section, what: &str) -> Item {
    let args = Args::ReviewComment(comment.id);
    let mut actions = vec![(Action::OpenReviewComment, args.clone())];
    if section == Section::ReturnedByAgents {
        actions.push((Action::ResolveReviewComment, args.clone()));
        actions.push((Action::ReopenReviewComment, args));
    }
    actions.push((Action::ShowLocalReview, Args::None));
    let line = comment.body.lines().next().unwrap_or_default();
    Item {
        key: format!("comment:{}", comment.id),
        section,
        what: format!("{what}: {line}"),
        place: comment.path.clone(),
        when: Some(last_word(comment)),
        actions,
    }
}

/// The count on the sidebar's Inbox page: the items that need the person or came back.
#[must_use]
pub(crate) fn badge_count(items: &[Item]) -> usize {
    items
        .iter()
        .filter(|item| matches!(item.section, Section::NeedsYou | Section::ReturnedByAgents))
        .count()
}

#[cfg(test)]
mod tests {
    use gm_schema::{Change, Stack, StackBranch};

    use super::*;

    fn entry(path: &str, conflicted: bool) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            staged: None,
            unstaged: Some(Change::Modified),
            untracked: false,
            conflicted,
            submodule: None,
        }
    }

    fn comment(id: i64, status: ReviewStatus, author: Option<&str>, needs: bool) -> ReviewComment {
        let mut comment: ReviewComment = serde_json::from_value(serde_json::json!({
            "id": id,
            "path": "src/a.rs",
            "body": "rename this\nplease",
            "status": status,
            "anchor": { "text": "x", "before": [], "after": [], "start_line": 1 },
            "created_at": 10,
            "resolved_at": null,
            "snapshot": null,
            "location": null,
            "outdated": false,
        }))
        .expect("a comment");
        comment.author = author.map(str::to_owned);
        comment.needs_answer = needs;
        comment
    }

    fn sections(items: &[Item]) -> Vec<(Section, String)> {
        items
            .iter()
            .map(|item| (item.section, item.key.clone()))
            .collect()
    }

    #[test]
    fn each_cause_gives_its_item_in_its_section() {
        let entries = [entry("a.rs", true), entry("b.rs", false)];
        let comments = [
            comment(1, ReviewStatus::Handled, None, false),
            comment(2, ReviewStatus::Open, None, false),
            comment(3, ReviewStatus::Open, Some("agent:claude"), true),
            comment(4, ReviewStatus::Resolved, None, false),
        ];
        let awaiting = [Awaiting {
            run: 7,
            workflow: "deploy",
            summary: "push main",
        }];
        let stale = StackBranch {
            name: "ui".into(),
            parent: Some("main".into()),
            depth: 1,
            state: StackState::Stale,
            ahead: 1,
            behind: 2,
            restack: 1,
            commits: Vec::new(),
            base: None,
            parent_tip: None,
        };
        let snapshot = StackSnapshot {
            stacks: vec![Stack {
                branches: vec![stale],
            }],
            looped: Vec::new(),
        };
        let inputs = Inputs {
            entries: &entries,
            comments: Some(&comments),
            awaiting: &awaiting,
            proposed_memories: 2,
            stacks: Some(&snapshot),
            branch: Some("ui"),
            upstream: Some("origin/ui"),
            behind: 3,
            resolver: true,
        };
        let all = items(&inputs);
        assert_eq!(
            sections(&all),
            vec![
                (Section::NeedsYou, "conflict:a.rs".into()),
                (Section::NeedsYou, "approval:7".into()),
                (Section::NeedsYou, "comment:3".into()),
                (Section::ReturnedByAgents, "comment:1".into()),
                (Section::ReturnedByAgents, "memories".into()),
                (Section::WaitingOnAgents, "comment:2".into()),
                (Section::FellBehind, "behind:ui".into()),
                (Section::FellBehind, "stack:ui".into()),
            ]
        );
        assert_eq!(all[0].actions[0].0, Action::ResolveConflict);
        assert_eq!(all[1].actions[0].0, Action::ApproveWorkflowStep);
        assert_eq!(all[3].what, "Handled by an agent: rename this");
        assert_eq!(all[7].actions[0].0, Action::Restack);
        assert_eq!(badge_count(&all), 5);
    }

    #[test]
    fn an_item_clears_by_itself_when_its_cause_is_gone() {
        let comments = [comment(1, ReviewStatus::Resolved, None, false)];
        let entries = [entry("a.rs", false)];
        let inputs = Inputs {
            entries: &entries,
            comments: Some(&comments),
            branch: Some("ui"),
            upstream: Some("origin/ui"),
            behind: 0,
            ..Inputs::default()
        };
        assert!(items(&inputs).is_empty());
    }

    #[test]
    fn only_the_newest_awaiting_run_offers_approve() {
        let awaiting = [
            Awaiting {
                run: 1,
                workflow: "a",
                summary: "x",
            },
            Awaiting {
                run: 2,
                workflow: "b",
                summary: "y",
            },
        ];
        let inputs = Inputs {
            awaiting: &awaiting,
            ..Inputs::default()
        };
        let all = items(&inputs);
        assert_eq!(all[0].actions[0].0, Action::ShowWorkflows);
        assert_eq!(all[1].actions[0].0, Action::ApproveWorkflowStep);
    }
}

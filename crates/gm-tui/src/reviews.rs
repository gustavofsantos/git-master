//! The review threads grouped by who acts next (D240 (3)), over the app's filters
//! (`gm_view::local_review::Filter`).

use gm_schema::{ReviewComment, ReviewStatus};
use gm_view::local_review::Filter;
use gm_view::review;

/// Who acts next on a thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// The last word is the person's: the agent has not answered it yet.
    Agent,
    /// The agent handled it, or had the last word: the person reads it next.
    You,
    Resolved,
}

impl Turn {
    pub fn title(self) -> &'static str {
        match self {
            Turn::Agent => "Waiting for the agent",
            Turn::You => "Waiting for you",
            Turn::Resolved => "Resolved",
        }
    }
}

/// Who acts next on `comment`: a resolved thread is done; a handled one, or one whose last
/// word is the agent's (a reply, or the root of a thread the agent started), waits for the
/// person; any other waits for the agent.
pub fn turn(comment: &ReviewComment) -> Turn {
    match comment.status {
        ReviewStatus::Resolved => Turn::Resolved,
        ReviewStatus::Handled => Turn::You,
        ReviewStatus::Open => {
            let last_is_agent = review::thread(&comment.replies, true)
                .shown
                .last()
                .map_or_else(
                    || review::is_agent_root(comment),
                    |reply| review::is_agent_reply(reply),
                );
            if last_is_agent || comment.needs_answer {
                Turn::You
            } else {
                Turn::Agent
            }
        }
    }
}

/// When a thread last moved: its newest reply, else when it was written.
fn last_activity(comment: &ReviewComment) -> i64 {
    comment
        .replies
        .iter()
        .map(|reply| reply.created_at)
        .max()
        .unwrap_or(comment.created_at)
        .max(comment.created_at)
}

/// A line of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Group(Turn, usize),
    Thread(i64),
}

/// The list: the threads `filter` keeps, under who acts next, the newest first in each
/// group. A group with no thread has no line.
pub fn entries(comments: &[ReviewComment], filter: Filter) -> Vec<Entry> {
    let mut out = Vec::new();
    for group in [Turn::You, Turn::Agent, Turn::Resolved] {
        let mut threads: Vec<&ReviewComment> = comments
            .iter()
            .filter(|comment| filter.keeps(comment) && turn(comment) == group)
            .collect();
        if threads.is_empty() {
            continue;
        }
        threads.sort_by_key(|comment| (std::cmp::Reverse(last_activity(comment)), comment.id));
        out.push(Entry::Group(group, threads.len()));
        out.extend(threads.into_iter().map(|comment| Entry::Thread(comment.id)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{ReviewAnchor, ReviewReply};

    fn comment(id: i64, status: ReviewStatus, created_at: i64) -> ReviewComment {
        ReviewComment {
            id,
            path: "a.rs".to_owned(),
            body: "body".to_owned(),
            status,
            anchor: ReviewAnchor {
                text: "x".to_owned(),
                before: Vec::new(),
                after: Vec::new(),
                start_line: 1,
            },
            created_at,
            resolved_at: None,
            snapshot: None,
            after_snapshot: None,
            location: None,
            outdated: false,
            replies: Vec::new(),
            author: None,
            kind: None,
            severity: None,
            needs_answer: false,
            suggested_fix: None,
            label: None,
            decorations: Vec::new(),
            scorecard: None,
            workspace: None,
        }
    }

    fn reply(id: i64, author: &str, created_at: i64) -> ReviewReply {
        ReviewReply {
            id,
            author: author.to_owned(),
            body: "body".to_owned(),
            created_at,
        }
    }

    #[test]
    fn threads_are_grouped_by_who_acts_next() {
        let mine = comment(1, ReviewStatus::Open, 10);
        let mut answered = comment(2, ReviewStatus::Open, 20);
        answered.replies = vec![reply(9, "agent:claude-code", 30)];
        let mut asked_back = comment(3, ReviewStatus::Open, 5);
        asked_back.replies = vec![reply(7, "agent:claude-code", 6), reply(8, "human", 40)];
        let handled = comment(4, ReviewStatus::Handled, 15);
        let mut agents = comment(5, ReviewStatus::Open, 25);
        agents.author = Some("agent:claude-code".to_owned());
        let resolved = comment(6, ReviewStatus::Resolved, 1);
        let all = [mine, answered, asked_back, handled, agents, resolved];
        assert_eq!(turn(&all[0]), Turn::Agent);
        assert_eq!(turn(&all[1]), Turn::You);
        assert_eq!(turn(&all[2]), Turn::Agent);
        assert_eq!(turn(&all[3]), Turn::You);
        assert_eq!(turn(&all[4]), Turn::You);
        assert_eq!(
            entries(&all, Filter::All),
            [
                Entry::Group(Turn::You, 3),
                Entry::Thread(2),
                Entry::Thread(5),
                Entry::Thread(4),
                Entry::Group(Turn::Agent, 2),
                Entry::Thread(3),
                Entry::Thread(1),
                Entry::Group(Turn::Resolved, 1),
                Entry::Thread(6),
            ]
        );
        // The app's Open filter keeps what is not resolved.
        assert_eq!(entries(&all, Filter::Open).len(), 7);
    }
}

//! The **Reviews** tab's model (D236, D240). The page state is the app's own Local review
//! model, `gm_view::local_review::LocalReview`: the filter, the selected thread, its Before
//! and After texts and the lines that changed between them. The companion adds only the
//! grouping of the sketch: by who acts next.

use gm_schema::{OperationId, ReviewComment, ReviewStatus};
use gm_view::local_review::{Filter, LocalReview, reads_change};
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

/// The tab's state: the app's page model, and the list's cursor and scroll.
#[derive(Debug, Default)]
pub struct ReviewsView {
    pub page: LocalReview,
    pub entries: Vec<Entry>,
    /// Index into `entries`; always on a thread when there is one.
    pub cursor: usize,
    pub scroll: usize,
    /// The Before read sent, waiting for its After read to be sent, so both are expected
    /// together, as the page model takes them.
    before_read: Option<OperationId>,
}

impl ReviewsView {
    /// Lays the list out again and keeps the cursor on the thread it was on, else near.
    pub fn relist(&mut self, comments: &[ReviewComment]) {
        let kept = self.selected_entry();
        self.page.keep_listed(comments);
        self.entries = entries(comments, self.page.filter());
        self.cursor = kept
            .and_then(|id| {
                self.entries
                    .iter()
                    .position(|entry| *entry == Entry::Thread(id))
            })
            .or_else(|| self.nearest_thread(self.cursor))
            .unwrap_or(0);
    }

    fn selected_entry(&self) -> Option<i64> {
        match self.entries.get(self.cursor) {
            Some(Entry::Thread(id)) => Some(*id),
            _ => None,
        }
    }

    /// The thread the cursor is on.
    pub fn at_cursor(&self) -> Option<i64> {
        self.selected_entry()
    }

    fn nearest_thread(&self, from: usize) -> Option<usize> {
        let is_thread = |index: &usize| matches!(self.entries[*index], Entry::Thread(_));
        let from = from.min(self.entries.len().saturating_sub(1));
        (from..self.entries.len())
            .find(is_thread)
            .or_else(|| (0..from).rev().find(is_thread))
    }

    /// Moves the cursor `by` threads, past the group lines, and stops at either end.
    pub fn move_cursor(&mut self, by: isize) {
        let threads: Vec<usize> = (0..self.entries.len())
            .filter(|index| matches!(self.entries[*index], Entry::Thread(_)))
            .collect();
        let Some(at) = threads.iter().position(|&index| index == self.cursor) else {
            if let Some(&first) = threads.first() {
                self.cursor = first;
            }
            return;
        };
        let target = at.saturating_add_signed(by).min(threads.len() - 1);
        self.cursor = threads[target];
    }

    /// The next filter, in the order of their chips.
    pub fn next_filter(&mut self, comments: &[ReviewComment]) {
        let at = Filter::ALL
            .iter()
            .position(|filter| *filter == self.page.filter())
            .unwrap_or(0);
        self.set_filter(Filter::ALL[(at + 1) % Filter::ALL.len()], comments);
    }

    pub fn set_filter(&mut self, filter: Filter, comments: &[ReviewComment]) {
        self.page.set_filter(filter);
        self.relist(comments);
    }

    /// The reads the selected thread's texts need, as the app's page asks them: one read of
    /// its two snapshots once an agent handled it, else its snapshot and the file now.
    pub fn text_reads(&self, comments: &[ReviewComment]) -> Vec<gm_schema::CommandKind> {
        use gm_schema::CommandKind;
        let Some(id) = self.page.selected_id() else {
            return Vec::new();
        };
        let handled = comments
            .iter()
            .find(|comment| comment.id == id)
            .is_some_and(reads_change);
        if handled {
            vec![CommandKind::LoadReviewChange { id }]
        } else {
            vec![
                CommandKind::LoadReviewSnapshotText { id },
                CommandKind::LoadReviewCurrentText { id },
            ]
        }
    }

    /// The engine took a text read as operation `id`.
    pub fn sent(&mut self, id: OperationId, kind: &gm_schema::CommandKind) {
        use gm_schema::CommandKind;
        match kind {
            CommandKind::LoadReviewChange { .. } => self.page.expect_change(id),
            CommandKind::LoadReviewSnapshotText { .. } => self.before_read = Some(id),
            CommandKind::LoadReviewCurrentText { .. } => {
                if let Some(before) = self.before_read.take() {
                    self.page.expect(before, id);
                }
            }
            _ => {}
        }
    }

    /// Lines added and removed in the thread's lines between its two texts, once both are
    /// read: what the agent changed there, when they are its snapshots.
    pub fn measured(&self) -> Option<(usize, usize)> {
        use gm_view::local_review::Version;
        self.page.lines(Version::Before)?;
        self.page.lines(Version::After)?;
        let added = self
            .page
            .marks(Version::After)
            .iter()
            .filter(|m| **m)
            .count();
        let removed = self
            .page
            .marks(Version::Before)
            .iter()
            .filter(|m| **m)
            .count();
        Some((added, removed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{CommandKind, ReviewReply};

    fn comment(id: i64, status: ReviewStatus, created_at: i64) -> ReviewComment {
        let mut comment = crate::app::tests::comment(id, "a.rs", status);
        comment.created_at = created_at;
        comment
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

    #[test]
    fn the_cursor_skips_groups_and_follows_its_thread_across_a_relist() {
        let all = [
            comment(1, ReviewStatus::Open, 10),
            comment(2, ReviewStatus::Handled, 20),
        ];
        let mut view = ReviewsView::default();
        view.set_filter(Filter::All, &all);
        assert_eq!(view.at_cursor(), Some(2));
        view.move_cursor(1);
        assert_eq!(view.at_cursor(), Some(1));
        // Thread 2 is resolved meanwhile: thread 1 keeps the cursor.
        let later = [
            comment(1, ReviewStatus::Open, 10),
            comment(2, ReviewStatus::Resolved, 20),
        ];
        view.relist(&later);
        assert_eq!(view.at_cursor(), Some(1));
    }

    #[test]
    fn a_handled_thread_reads_its_two_snapshots_in_one_go() {
        let mut handled = comment(4, ReviewStatus::Handled, 1);
        handled.after_snapshot = Some("after".to_owned());
        let open = comment(5, ReviewStatus::Open, 2);
        let all = [handled, open];
        let mut view = ReviewsView::default();
        view.set_filter(Filter::All, &all);
        assert!(view.page.select(&all[0]));
        assert_eq!(
            view.text_reads(&all),
            [CommandKind::LoadReviewChange { id: 4 }]
        );
        assert!(view.page.select(&all[1]));
        assert_eq!(
            view.text_reads(&all),
            [
                CommandKind::LoadReviewSnapshotText { id: 5 },
                CommandKind::LoadReviewCurrentText { id: 5 }
            ]
        );
    }
}

//! The threads linked to the commit the history page shows (H3, T5). The commit
//! panel's Threads section lists them: the newest ten, read when a commit is
//! selected. Only the answer to the read in flight is kept, so a commit selected
//! after another never shows the other's threads.

use std::path::Path;

use gm_schema::{CommandKind, OperationId, ThreadFilter, ThreadSummary};

/// The most threads a commit's section lists. A full page says that more may follow.
pub const LIMIT: u32 = 10;

/// What selecting a row of the section does (D180 (2)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Opens the thread on the Threads page.
    Thread(String),
    /// Opens the workspace folder in a window of its own.
    Workspace(String),
}

/// The target of a row: the workspace a thread of another repository belongs to, else the thread.
#[must_use]
pub fn target(thread: &ThreadSummary) -> Target {
    match &thread.workspace {
        Some(folder) => Target::Workspace(folder.clone()),
        None => Target::Thread(thread.uid.clone()),
    }
}

/// The words a workspace thread's row shows after its agent: `in workspace <folder name>`.
/// `None` for a thread of the open repository.
#[must_use]
pub fn workspace_caption(thread: &ThreadSummary) -> Option<String> {
    let folder = thread.workspace.as_deref()?;
    let name = Path::new(folder).file_name()?.to_string_lossy();
    Some(format!("in workspace {name}"))
}

/// The read of the threads linked to commit `oid`: the full id the panel shows.
#[must_use]
pub fn plan(oid: &str) -> CommandKind {
    CommandKind::ListThreads {
        filter: ThreadFilter {
            commit: Some(oid.to_owned()),
            ..ThreadFilter::default()
        },
        before_at: None,
        before_uid: None,
        limit: LIMIT,
    }
}

/// The threads of the commit the panel last asked about, and the read in flight.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitThreads {
    oid: Option<String>,
    pending: Option<OperationId>,
    threads: Vec<ThreadSummary>,
}

impl CommitThreads {
    /// Asks about commit `oid`, which the read `id` answers. The threads of another
    /// commit go; those of the same commit stay until the new answer replaces them.
    pub fn request(&mut self, oid: &str, id: OperationId) {
        if self.oid.as_deref() != Some(oid) {
            self.threads.clear();
        }
        self.oid = Some(oid.to_owned());
        self.pending = Some(id);
    }

    /// The commit the threads are asked about, if any.
    #[must_use]
    pub fn oid(&self) -> Option<&str> {
        self.oid.as_deref()
    }

    /// Whether `id` is the read in flight.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// Keeps the answer when it is the read in flight. Returns false for any other.
    pub fn listed(&mut self, id: OperationId, threads: Vec<ThreadSummary>) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.pending = None;
        self.threads = threads;
        true
    }

    /// The read `id` ended without an answer: it is no longer in flight.
    pub fn ended(&mut self, id: OperationId) {
        if self.owns(id) {
            self.pending = None;
        }
    }

    /// The threads of commit `oid`, when they are the ones read for it. Empty otherwise.
    #[must_use]
    pub fn threads_of(&self, oid: &str) -> &[ThreadSummary] {
        if self.oid.as_deref() == Some(oid) {
            &self.threads
        } else {
            &[]
        }
    }

    /// Whether the answer is a full page, so more threads may be linked to the commit.
    #[must_use]
    pub fn is_full(&self) -> bool {
        u32::try_from(self.threads.len()).is_ok_and(|len| len >= LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(uid: &str) -> ThreadSummary {
        ThreadSummary {
            uid: uid.to_owned(),
            harness: "claude-code".to_owned(),
            harness_version: None,
            session_id: "s".to_owned(),
            worktree: "/repo".to_owned(),
            branch: None,
            model: None,
            title: format!("Thread {uid}"),
            note: String::new(),
            starred: false,
            tags: Vec::new(),
            started_at: 0,
            last_event_at: 1,
            ended_at: None,
            entry_count: 0,
            edit_count: 0,
            redaction_count: 0,
            workspace: None,
        }
    }

    #[test]
    fn a_workspace_thread_names_its_folder_and_opens_the_workspace() {
        let mut thread = summary("t1");
        assert_eq!(workspace_caption(&thread), None);
        assert_eq!(target(&thread), Target::Thread("t1".to_owned()));
        thread.workspace = Some("/home/me/workspaces/checkout-v2".to_owned());
        assert_eq!(
            workspace_caption(&thread).as_deref(),
            Some("in workspace checkout-v2")
        );
        assert_eq!(
            target(&thread),
            Target::Workspace("/home/me/workspaces/checkout-v2".to_owned()),
            "selecting it opens the workspace, not the thread here"
        );
    }

    #[test]
    fn the_read_asks_for_the_newest_ten_threads_linked_to_the_full_commit() {
        let oid = "0123456789abcdef0123456789abcdef01234567";
        let CommandKind::ListThreads {
            filter,
            before_at,
            before_uid,
            limit,
        } = plan(oid)
        else {
            panic!("not a list read");
        };
        assert_eq!(filter.commit.as_deref(), Some(oid));
        assert_eq!(filter.path, None, "no other filter is set");
        assert_eq!((before_at, before_uid, limit), (None, None, 10));
    }

    #[test]
    fn the_answer_to_the_read_in_flight_is_shown_for_its_commit() {
        let mut threads = CommitThreads::default();
        threads.request("aaa", OperationId(4));
        assert!(threads.owns(OperationId(4)));
        assert!(threads.listed(OperationId(4), vec![summary("t1")]));
        assert_eq!(threads.threads_of("aaa").len(), 1);
        assert!(
            threads.threads_of("bbb").is_empty(),
            "another commit shows none"
        );
    }

    #[test]
    fn an_answer_to_a_superseded_read_is_dropped() {
        let mut threads = CommitThreads::default();
        threads.request("aaa", OperationId(4));
        threads.request("bbb", OperationId(5));
        assert!(!threads.listed(OperationId(4), vec![summary("old")]));
        assert!(threads.threads_of("bbb").is_empty());
        assert!(threads.listed(OperationId(5), vec![summary("new")]));
        assert_eq!(threads.threads_of("bbb")[0].uid, "new");
    }

    #[test]
    fn the_same_commit_keeps_its_threads_until_the_new_answer() {
        let mut threads = CommitThreads::default();
        threads.request("aaa", OperationId(4));
        assert!(threads.listed(OperationId(4), vec![summary("t1")]));
        threads.request("aaa", OperationId(6));
        assert_eq!(
            threads.threads_of("aaa").len(),
            1,
            "no flicker on a refresh"
        );
    }

    #[test]
    fn a_full_page_says_more_may_follow() {
        let mut threads = CommitThreads::default();
        threads.request("aaa", OperationId(1));
        let full: Vec<ThreadSummary> = (0..10).map(|i| summary(&format!("t{i}"))).collect();
        assert!(threads.listed(OperationId(1), full));
        assert!(threads.is_full());
        threads.request("bbb", OperationId(2));
        assert!(threads.listed(OperationId(2), vec![summary("one")]));
        assert!(!threads.is_full());
    }

    #[test]
    fn a_failed_read_leaves_nothing_in_flight() {
        let mut threads = CommitThreads::default();
        threads.request("aaa", OperationId(3));
        threads.ended(OperationId(3));
        assert!(!threads.owns(OperationId(3)));
        assert!(threads.threads_of("aaa").is_empty());
    }
}

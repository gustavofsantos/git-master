//! The state of the Changes page's Branch and Compare tabs (D231): the tab on
//! screen, the revisions and the mode the tabs read, the answers in flight and
//! the pickers' suggestions. Pure: no GPUI, so every rule is unit-tested on its
//! own. The view draws what this holds and sends the commands it asks for.

// The Changes page draws these in a later slice; until then only the tests call them.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::HashSet;

use gm_schema::{BranchEntry, CommandKind, CommitFile, OperationId, RangeMode, RevisionRange};

use crate::fuzzy::fuzzy_match;

/// The tab of the Changes page on screen (D231 (1)). The choice lasts while the
/// window is open; opening another repository goes back to `Uncommitted`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChangesTab {
    #[default]
    Uncommitted,
    Branch,
    Compare,
}

impl ChangesTab {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ChangesTab::Uncommitted => "Uncommitted",
            ChangesTab::Branch => "Branch",
            ChangesTab::Compare => "Compare",
        }
    }
}

/// The answer a read-only tab shows. Engine range reads supersede each other, so
/// only the answer of the request in flight is taken.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RangeView {
    #[default]
    Idle,
    Loading {
        id: OperationId,
    },
    /// Boxed: a range is much larger than the other states.
    Shown(Box<RevisionRange>),
    Failed(String),
}

impl RangeView {
    /// The read `id` was sent. Whatever was shown before is replaced by the wait.
    pub fn request(&mut self, id: OperationId) {
        *self = RangeView::Loading { id };
    }

    /// Whether the request in flight is `id`.
    #[must_use]
    pub fn awaits(&self, id: OperationId) -> bool {
        matches!(self, RangeView::Loading { id: wanted } if *wanted == id)
    }

    /// Takes the answer to `id` when it is the request in flight; `false` drops a late one.
    pub fn received(&mut self, id: OperationId, range: RevisionRange) -> bool {
        if self.awaits(id) {
            *self = RangeView::Shown(Box::new(range));
            true
        } else {
            false
        }
    }

    /// Takes git's message for `id` when it is the request in flight.
    pub fn failed(&mut self, id: OperationId, message: String) -> bool {
        if self.awaits(id) {
            *self = RangeView::Failed(message);
            true
        } else {
            false
        }
    }

    pub fn reset(&mut self) {
        *self = RangeView::Idle;
    }

    #[must_use]
    pub fn is_loading(&self) -> bool {
        matches!(self, RangeView::Loading { .. })
    }

    #[must_use]
    pub fn range(&self) -> Option<&RevisionRange> {
        match self {
            RangeView::Shown(range) => Some(range.as_ref()),
            _ => None,
        }
    }
}

/// The stacked diff the view must read next: the files of a range that was just
/// shown, as a `LoadRevisionStack` of its `base` and `tip`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackRequest {
    pub base: String,
    pub tip: String,
    pub files: Vec<CommitFile>,
    /// Changed files past the listed ones.
    pub more: usize,
}

/// Everything the two tabs hold. The Branch tab reads `LoadBranchChange`, the
/// Compare tab `LoadRevisionRange`; the tag suggestions come from `ListTags`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompareState {
    pub tab: ChangesTab,
    /// The base of the Branch tab; `None` is the default branch.
    pub base: Option<String>,
    pub branch_view: RangeView,
    /// The **From** field of the Compare tab, as typed. Empty until the view fills it.
    pub from: String,
    /// The **To** field of the Compare tab, as typed. Empty until the view fills it.
    pub to: String,
    pub mode: RangeMode,
    pub compare_view: RangeView,
    /// The tag names for the pickers' suggestions, newest first.
    pub tags: Vec<String>,
    /// The `ListTags` in flight, if any.
    pub tags_id: Option<OperationId>,
    stack_request: Option<StackRequest>,
}

impl CompareState {
    pub fn set_tab(&mut self, tab: ChangesTab) {
        self.tab = tab;
    }

    pub fn set_base(&mut self, base: Option<String>) {
        self.base = base;
    }

    pub fn set_from(&mut self, from: String) {
        self.from = from;
    }

    pub fn set_to(&mut self, to: String) {
        self.to = to;
    }

    pub fn set_mode(&mut self, mode: RangeMode) {
        self.mode = mode;
    }

    /// Exchanges **From** and **To**, to read the same change backwards.
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.from, &mut self.to);
    }

    /// Another repository opened: back to Uncommitted with nothing of the old one.
    pub fn reset_for_new_repo(&mut self) {
        *self = Self::default();
    }

    /// Whether the Compare fields are both empty, so the view should fill them
    /// with the default branch and HEAD.
    #[must_use]
    pub fn needs_defaults(&self) -> bool {
        self.from.trim().is_empty() && self.to.trim().is_empty()
    }

    /// The read the shown tab needs now. Uncommitted reads nothing here, and the
    /// Compare tab reads only when both fields are filled.
    #[must_use]
    pub fn active_command(&self) -> Option<CommandKind> {
        match self.tab {
            ChangesTab::Uncommitted => None,
            ChangesTab::Branch => Some(CommandKind::LoadBranchChange {
                base: self.base.clone(),
            }),
            ChangesTab::Compare => {
                let from = self.from.trim();
                let to = self.to.trim();
                if from.is_empty() || to.is_empty() {
                    None
                } else {
                    Some(CommandKind::LoadRevisionRange {
                        from: from.to_owned(),
                        to: to.to_owned(),
                        mode: self.mode,
                    })
                }
            }
        }
    }

    /// The Branch tab's read `id` was sent. A range read supersedes the other one,
    /// so an answer the Compare tab still awaits never comes and it becomes idle.
    pub fn request_branch(&mut self, id: OperationId) {
        self.branch_view.request(id);
        if self.compare_view.is_loading() {
            self.compare_view.reset();
        }
    }

    /// The Compare tab's read `id` was sent; the same supersession as `request_branch`.
    pub fn request_compare(&mut self, id: OperationId) {
        self.compare_view.request(id);
        if self.branch_view.is_loading() {
            self.branch_view.reset();
        }
    }

    /// The `ListTags` `id` was sent.
    pub fn request_tags(&mut self, id: OperationId) {
        self.tags_id = Some(id);
    }

    /// Routes a range answer to the view that awaits `id`. When it is taken and
    /// lists files, the stacked diff of it becomes the next stack request.
    pub fn range_received(&mut self, id: OperationId, range: RevisionRange) -> bool {
        let request = (!range.files.is_empty()).then(|| StackRequest {
            base: range.base.clone(),
            tip: range.tip.clone(),
            files: range.files.clone(),
            more: range.more_files,
        });
        let taken = if self.branch_view.awaits(id) {
            self.branch_view.received(id, range)
        } else {
            self.compare_view.received(id, range)
        };
        if taken {
            self.stack_request = request;
        }
        taken
    }

    /// Routes git's message for a failed read to the view that awaits `id`, and
    /// drops a tag read that failed. `false` when neither awaits `id`.
    pub fn failed(&mut self, id: OperationId, message: String) -> bool {
        if self.tags_id == Some(id) {
            self.tags_id = None;
            return true;
        }
        if self.branch_view.awaits(id) {
            self.branch_view.failed(id, message)
        } else {
            self.compare_view.failed(id, message)
        }
    }

    /// Takes the tag answer for `id` when it is the read in flight.
    pub fn tags_listed(&mut self, id: OperationId, tags: Vec<String>) -> bool {
        if self.tags_id == Some(id) {
            self.tags = tags;
            self.tags_id = None;
            true
        } else {
            false
        }
    }

    /// The stacked diff the view must read next, once. `None` after it was taken.
    pub fn take_stack_request(&mut self) -> Option<StackRequest> {
        self.stack_request.take()
    }
}

/// The header of the shown range (D231 (2), (3)): `to against from` on the Branch
/// tab, `from (short) → to (short)` on the Compare tab. Empty for Uncommitted.
#[must_use]
pub fn header(range: &RevisionRange, tab: ChangesTab) -> String {
    match tab {
        ChangesTab::Uncommitted => String::new(),
        ChangesTab::Branch => format!("{} against {}", range.to_name, range.from_name),
        ChangesTab::Compare => format!(
            "{} ({}) → {} ({})",
            range.from_name, range.from_short, range.to_name, range.to_short
        ),
    }
}

/// The file count and the lines added and removed, for the header of the list.
#[must_use]
pub fn totals(range: &RevisionRange) -> String {
    let listed = range.files.len();
    let lines = format!("+{} −{}", range.added, range.deleted);
    if range.more_files == 0 {
        let files = if listed == 1 {
            "1 file".to_owned()
        } else {
            format!("{listed} files")
        };
        format!("{files} · {lines}")
    } else {
        let all = listed + range.more_files;
        format!("First {listed} of {all} files · {lines}")
    }
}

/// What an empty range says. Empty for Uncommitted, which has its own notices.
#[must_use]
pub fn empty_notice(tab: ChangesTab, range: &RevisionRange) -> String {
    match tab {
        ChangesTab::Uncommitted => String::new(),
        ChangesTab::Branch => format!("No changes against `{}`.", range.from_name),
        ChangesTab::Compare => "No changes between these revisions.".to_owned(),
    }
}

/// What a picker suggests for one typed revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub text: String,
    pub kind: SuggestionKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionKind {
    Branch,
    RemoteBranch,
    Tag,
}

/// The suggestions for `query`, at most `limit`. Local branches come first, then
/// remote branches, then tags, each in the order given. An empty query lists them
/// unfiltered; otherwise they are ranked by the palette's fuzzy match. A text is
/// listed once, the first time it appears.
#[must_use]
pub fn suggestions(
    query: &str,
    branches: &[BranchEntry],
    tags: &[String],
    limit: usize,
) -> Vec<Suggestion> {
    let query = query.trim();
    let local = branches
        .iter()
        .filter(|branch| !branch.is_remote)
        .map(|branch| (branch.name.as_str(), SuggestionKind::Branch));
    let remote = branches
        .iter()
        .filter(|branch| branch.is_remote)
        .map(|branch| (branch.name.as_str(), SuggestionKind::RemoteBranch));
    let tagged = tags.iter().map(|tag| (tag.as_str(), SuggestionKind::Tag));

    let mut seen = HashSet::new();
    let mut ranked: Vec<(i64, Suggestion)> = Vec::new();
    for (text, kind) in local.chain(remote).chain(tagged) {
        if !seen.insert(text) {
            continue;
        }
        let score = if query.is_empty() {
            0
        } else if let Some(found) = fuzzy_match(query, text) {
            found.score
        } else {
            continue;
        };
        ranked.push((
            score,
            Suggestion {
                text: text.to_owned(),
                kind,
            },
        ));
    }
    if !query.is_empty() {
        ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    }
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, suggestion)| suggestion)
        .collect()
}

/// One branch the Base picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseOption {
    pub name: String,
    /// The stack parent of the current branch, listed first.
    pub parent: bool,
}

/// The bases the Base picker offers (D231 (2)): the stack parent first, marked
/// `parent`, then the local branches and the remote ones. The current branch is
/// never offered, and a name is listed once.
#[must_use]
pub fn base_options(
    branches: &[BranchEntry],
    stack_parent: Option<&str>,
    current: Option<&str>,
) -> Vec<BaseOption> {
    let local = branches
        .iter()
        .filter(|branch| !branch.is_remote)
        .map(|branch| branch.name.as_str());
    let remote = branches
        .iter()
        .filter(|branch| branch.is_remote)
        .map(|branch| branch.name.as_str());

    let mut seen: HashSet<&str> = current.into_iter().collect();
    let mut options = Vec::new();
    for name in stack_parent.into_iter().chain(local).chain(remote) {
        if seen.insert(name) {
            options.push(BaseOption {
                name: name.to_owned(),
                parent: stack_parent == Some(name),
            });
        }
    }
    options
}

#[cfg(test)]
mod tests {
    use gm_schema::{Change, CommitFile};

    use super::*;

    fn id(n: u64) -> OperationId {
        OperationId(n)
    }

    fn file(path: &str) -> CommitFile {
        CommitFile {
            path: path.into(),
            old_path: None,
            change: Change::Modified,
            added: 1,
            deleted: 0,
            binary: false,
        }
    }

    fn range(files: &[&str], more: usize) -> RevisionRange {
        RevisionRange {
            from_oid: "a".repeat(40),
            to_oid: "b".repeat(40),
            from_short: "a1b2c3d".into(),
            to_short: "e4f5a6b".into(),
            from_name: "main".into(),
            to_name: "feature/billing".into(),
            base: "c".repeat(40),
            tip: "b".repeat(40),
            files: files.iter().map(|path| file(path)).collect(),
            more_files: more,
            added: 340,
            deleted: 96,
        }
    }

    fn branch(name: &str, remote: bool) -> BranchEntry {
        BranchEntry {
            name: name.to_owned(),
            is_remote: remote,
            is_current: false,
            upstream: None,
            author_email: None,
        }
    }

    #[test]
    fn the_tabs_start_on_uncommitted_and_are_labelled() {
        assert_eq!(ChangesTab::default(), ChangesTab::Uncommitted);
        assert_eq!(ChangesTab::Uncommitted.label(), "Uncommitted");
        assert_eq!(ChangesTab::Branch.label(), "Branch");
        assert_eq!(ChangesTab::Compare.label(), "Compare");
    }

    #[test]
    fn a_range_view_takes_only_the_answer_to_the_request_in_flight() {
        let mut view = RangeView::default();
        view.request(id(4));
        assert!(!view.received(id(3), range(&[], 0)));
        assert!(view.is_loading());
        assert!(view.received(id(4), range(&["a.txt"], 0)));
        assert_eq!(view.range().map(|r| r.files.len()), Some(1));
        // A second answer to the same id finds nothing in flight.
        assert!(!view.received(id(4), range(&[], 0)));
    }

    #[test]
    fn a_range_view_keeps_git_message_for_its_own_request_only() {
        let mut view = RangeView::default();
        view.request(id(4));
        assert!(!view.failed(id(2), "late".into()));
        assert!(view.failed(id(4), "unknown revision v9".into()));
        assert_eq!(view, RangeView::Failed("unknown revision v9".into()));
        view.reset();
        assert_eq!(view, RangeView::Idle);
    }

    #[test]
    fn the_uncommitted_tab_reads_nothing_and_branch_reads_its_base() {
        let mut state = CompareState::default();
        assert_eq!(state.active_command(), None);
        state.set_tab(ChangesTab::Branch);
        assert_eq!(
            state.active_command(),
            Some(CommandKind::LoadBranchChange { base: None })
        );
        state.set_base(Some("develop".into()));
        assert_eq!(
            state.active_command(),
            Some(CommandKind::LoadBranchChange {
                base: Some("develop".into())
            })
        );
    }

    #[test]
    fn compare_reads_only_when_both_fields_are_filled_and_trimmed() {
        let mut state = CompareState::default();
        state.set_tab(ChangesTab::Compare);
        assert_eq!(state.active_command(), None);
        state.set_from("  v1.4.0 ".into());
        assert_eq!(state.active_command(), None);
        state.set_to(" main\t".into());
        state.set_mode(RangeMode::SinceCommonAncestor);
        assert_eq!(
            state.active_command(),
            Some(CommandKind::LoadRevisionRange {
                from: "v1.4.0".into(),
                to: "main".into(),
                mode: RangeMode::SinceCommonAncestor,
            })
        );
    }

    #[test]
    fn the_compare_fields_need_defaults_only_when_both_are_empty() {
        let mut state = CompareState::default();
        assert!(state.needs_defaults());
        state.set_to("HEAD".into());
        assert!(!state.needs_defaults());
        state.set_to(" ".into());
        assert!(state.needs_defaults());
    }

    #[test]
    fn swap_exchanges_from_and_to_only() {
        let mut state = CompareState::default();
        state.set_from("main".into());
        state.set_to("v2".into());
        state.set_mode(RangeMode::SinceCommonAncestor);
        state.swap();
        assert_eq!((state.from.as_str(), state.to.as_str()), ("v2", "main"));
        assert_eq!(state.mode, RangeMode::SinceCommonAncestor);
    }

    #[test]
    fn a_new_repository_goes_back_to_uncommitted_with_nothing_kept() {
        let mut state = CompareState::default();
        state.set_tab(ChangesTab::Compare);
        state.set_from("main".into());
        state.request_tags(id(9));
        state.reset_for_new_repo();
        assert_eq!(state, CompareState::default());
    }

    #[test]
    fn a_range_with_files_asks_for_its_stack_once() {
        let mut state = CompareState::default();
        state.request_compare(id(5));
        assert!(state.range_received(id(5), range(&["a.txt", "b.txt"], 3)));
        let request = state.take_stack_request().expect("a stack request");
        assert_eq!(request.base, "c".repeat(40));
        assert_eq!(request.tip, "b".repeat(40));
        assert_eq!(request.files.len(), 2);
        assert_eq!(request.more, 3);
        assert_eq!(state.take_stack_request(), None);
    }

    #[test]
    fn a_range_with_no_files_asks_for_no_stack() {
        let mut state = CompareState::default();
        state.request_branch(id(5));
        assert!(state.range_received(id(5), range(&[], 0)));
        assert_eq!(state.take_stack_request(), None);
        assert!(state.branch_view.range().is_some());
    }

    #[test]
    fn a_range_answer_for_a_superseded_request_is_dropped() {
        let mut state = CompareState::default();
        state.request_branch(id(5));
        state.request_compare(id(6));
        assert_eq!(state.branch_view, RangeView::Idle);
        assert!(!state.range_received(id(5), range(&["a.txt"], 0)));
        assert_eq!(state.take_stack_request(), None);
        assert!(state.compare_view.is_loading());
    }

    #[test]
    fn a_failure_reaches_the_view_that_awaits_it() {
        let mut state = CompareState::default();
        state.request_compare(id(6));
        assert!(state.failed(id(6), "fatal: bad revision".into()));
        assert_eq!(
            state.compare_view,
            RangeView::Failed("fatal: bad revision".into())
        );
        assert!(!state.failed(id(7), "other".into()));
    }

    #[test]
    fn a_tag_read_is_taken_for_its_own_request_only() {
        let mut state = CompareState::default();
        state.request_tags(id(3));
        assert!(!state.tags_listed(id(2), vec!["old".into()]));
        assert!(state.tags.is_empty());
        assert!(state.tags_listed(id(3), vec!["v2.0".into(), "v1.0".into()]));
        assert_eq!(state.tags, ["v2.0", "v1.0"]);
        assert_eq!(state.tags_id, None);
    }

    #[test]
    fn a_failed_tag_read_forgets_its_request() {
        let mut state = CompareState::default();
        state.request_tags(id(3));
        assert!(state.failed(id(3), "git failed".into()));
        assert_eq!(state.tags_id, None);
        assert!(state.tags.is_empty());
    }

    #[test]
    fn the_branch_header_names_the_branch_against_its_base() {
        assert_eq!(
            header(&range(&[], 0), ChangesTab::Branch),
            "feature/billing against main"
        );
    }

    #[test]
    fn the_compare_header_shows_both_revisions_with_their_commits() {
        let mut shown = range(&[], 0);
        shown.from_name = "v1.4.0".into();
        shown.from_short = "a1b2c3d".into();
        shown.to_name = "main".into();
        shown.to_short = "e4f5a6b".into();
        assert_eq!(
            header(&shown, ChangesTab::Compare),
            "v1.4.0 (a1b2c3d) → main (e4f5a6b)"
        );
    }

    #[test]
    fn totals_count_files_and_lines() {
        assert_eq!(totals(&range(&["a"; 12], 0)), "12 files · +340 −96");
        assert_eq!(totals(&range(&["a"], 0)), "1 file · +340 −96");
        assert_eq!(totals(&range(&[], 0)), "0 files · +340 −96");
    }

    #[test]
    fn totals_say_how_many_files_are_listed_when_some_are_not() {
        assert_eq!(
            totals(&range(&["a"; 200], 40)),
            "First 200 of 240 files · +340 −96"
        );
    }

    #[test]
    fn the_empty_notices_name_the_base_or_the_pair() {
        assert_eq!(
            empty_notice(ChangesTab::Branch, &range(&[], 0)),
            "No changes against `main`."
        );
        assert_eq!(
            empty_notice(ChangesTab::Compare, &range(&[], 0)),
            "No changes between these revisions."
        );
    }

    #[test]
    fn an_empty_query_lists_the_first_branches_then_tags_unfiltered() {
        let branches = [
            branch("main", false),
            branch("feature", false),
            branch("origin/main", true),
        ];
        let tags = ["v1.0".to_owned()];
        let listed = suggestions("", &branches, &tags, 3);
        let texts: Vec<&str> = listed.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["main", "feature", "origin/main"]);
        assert_eq!(listed[2].kind, SuggestionKind::RemoteBranch);
        let all = suggestions("", &branches, &tags, 10);
        assert_eq!(all.last().map(|s| s.kind), Some(SuggestionKind::Tag));
    }

    #[test]
    fn a_query_keeps_only_the_matches_and_ranks_them_as_the_palette_does() {
        let branches = [branch("feature/billing", false), branch("main", false)];
        let tags = ["v2.0-billing-rc".to_owned()];
        let found = suggestions("bill", &branches, &tags, 10);
        let texts: Vec<&str> = found.iter().map(|s| s.text.as_str()).collect();
        // A word start nearer the front ranks higher, as in the palette.
        assert_eq!(texts, ["v2.0-billing-rc", "feature/billing"]);
        assert!(suggestions("zzz", &branches, &tags, 10).is_empty());
    }

    #[test]
    fn a_text_is_suggested_once_and_the_limit_holds() {
        let branches = [branch("main", false), branch("main", false)];
        let tags = ["main".to_owned(), "v1".to_owned()];
        let listed = suggestions("", &branches, &tags, 10);
        let texts: Vec<&str> = listed.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["main", "v1"]);
        assert_eq!(suggestions("", &branches, &tags, 1).len(), 1);
    }

    #[test]
    fn the_base_picker_lists_the_stack_parent_first_and_skips_the_current_branch() {
        let branches = [
            branch("main", false),
            branch("parent", false),
            branch("feature", false),
            branch("origin/main", true),
        ];
        let options = base_options(&branches, Some("parent"), Some("feature"));
        let names: Vec<(&str, bool)> = options
            .iter()
            .map(|option| (option.name.as_str(), option.parent))
            .collect();
        assert_eq!(
            names,
            [("parent", true), ("main", false), ("origin/main", false)]
        );
    }

    #[test]
    fn the_base_picker_without_a_stack_has_no_parent_mark() {
        let branches = [branch("main", false), branch("origin/main", true)];
        let options = base_options(&branches, None, Some("main"));
        assert_eq!(
            options,
            [BaseOption {
                name: "origin/main".into(),
                parent: false
            }]
        );
    }
}

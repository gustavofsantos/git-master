//! The labels, rows and choices of the Memories tab (D190, D193), with no GPUI in them: what a
//! row says, what the tab's badge and counts read, where a source opens, which detail actions a
//! memory offers, and the order the filter chips cycle through. The model is `memories.rs`; the
//! memory text is only counted or cut here, never read as anything else.

use std::collections::BTreeSet;

use gm_schema::threads::{
    Context, Memory, MemoryBasis, MemoryKind, MemorySource, MemorySourceKind, MemoryStatus,
};

use crate::memories::{self, Scope, TEXT_CHARS};

/// The longest first line a row shows before it is cut.
const SNIPPET_CHARS: usize = 160;
/// The characters of a thread uid a label shows.
const UID_CHARS: usize = 8;
/// The characters of a commit id a label shows.
const COMMIT_CHARS: usize = 12;

/// The kinds the kind chip cycles through: any kind first.
pub const KINDS: [Option<MemoryKind>; 5] = [
    None,
    Some(MemoryKind::Decision),
    Some(MemoryKind::Rule),
    Some(MemoryKind::Lesson),
    Some(MemoryKind::Fact),
];

/// The statuses the status chip cycles through: any status first.
pub const STATUSES: [Option<MemoryStatus>; 5] = [
    None,
    Some(MemoryStatus::Proposed),
    Some(MemoryStatus::Accepted),
    Some(MemoryStatus::Rejected),
    Some(MemoryStatus::Superseded),
];

/// The badge on the Memories tab: the proposals waiting, hidden at none (D190).
#[must_use]
pub fn tab_badge(proposed: usize) -> Option<String> {
    (proposed > 0).then(|| proposed.to_string())
}

/// The count above the list: the rows shown, with a plus when more follow.
#[must_use]
pub fn count_label(shown: usize, more: bool) -> String {
    match (shown, more) {
        (count, true) => format!("{count}+ memories"),
        (1, false) => "1 memory".to_owned(),
        (count, false) => format!("{count} memories"),
    }
}

/// A row's text: the first line of the memory, cut, with an ellipsis when more follows.
#[must_use]
pub fn snippet(text: &str) -> String {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or_default();
    let more = lines.next().is_some();
    let cut: String = first.chars().take(SNIPPET_CHARS).collect();
    if more || first.chars().count() > SNIPPET_CHARS {
        format!("{cut} …")
    } else {
        cut
    }
}

/// The characters left before the limit of a memory text, or how far over it the text is.
#[must_use]
pub fn remaining_text(chars: usize) -> String {
    if chars <= TEXT_CHARS {
        match TEXT_CHARS - chars {
            1 => "1 character left".to_owned(),
            left => format!("{left} characters left"),
        }
    } else {
        match chars - TEXT_CHARS {
            1 => "1 character over the limit".to_owned(),
            over => format!("{over} characters over the limit"),
        }
    }
}

/// Whether a text of `chars` characters is over the limit of a memory.
#[must_use]
pub fn over_limit(chars: usize) -> bool {
    chars > TEXT_CHARS
}

/// The label of a scope: the chip's words, `Everything`, `This repository` or a context's path.
#[must_use]
pub fn scope_label(scope: &Scope) -> String {
    match scope {
        Scope::Everything => "Everything".to_owned(),
        Scope::Repository => "This repository".to_owned(),
        Scope::Context(path) => path.clone(),
    }
}

/// The scopes the scope chip cycles through: everything, this repository, then each context the
/// repository belongs to.
#[must_use]
pub fn scope_options(contexts: &[Context]) -> Vec<Scope> {
    let mut options = vec![Scope::Everything, Scope::Repository];
    options.extend(
        contexts
            .iter()
            .map(|context| Scope::Context(context.path.clone())),
    );
    options
}

/// The option after `current` in `options`, wrapping to the first. When `current` is not one of
/// them, the first. None only when there are no options.
#[must_use]
pub fn next_choice<T: PartialEq + Clone>(options: &[T], current: &T) -> Option<T> {
    let next = options
        .iter()
        .position(|option| option == current)
        .map_or(0, |index| index + 1);
    options.get(next).or_else(|| options.first()).cloned()
}

/// The authors the author chip cycles through: any author first, then each origin the loaded
/// memories hold, sorted, with the chosen one even when no loaded memory holds it.
#[must_use]
pub fn author_options(loaded: &[Memory], chosen: Option<&str>) -> Vec<Option<String>> {
    let origins: BTreeSet<&str> = loaded
        .iter()
        .map(|memory| memory.origin.as_str())
        .chain(chosen)
        .collect();
    std::iter::once(None)
        .chain(origins.into_iter().map(|origin| Some(origin.to_owned())))
        .collect()
}

/// A filter chip's text: its name and the value it filters by, `Any` when none.
#[must_use]
pub fn chip_text(name: &str, value: Option<String>) -> String {
    format!("{name}: {}", value.unwrap_or_else(|| "Any".to_owned()))
}

/// The file the path box filters by: the text trimmed, none when it is blank.
#[must_use]
pub fn path_filter(text: &str) -> Option<String> {
    let path = text.trim();
    (!path.is_empty()).then(|| path.to_owned())
}

/// The label of a source in a memory's detail: what it is, and its entries when it is a range.
#[must_use]
pub fn source_label(source: &MemorySource) -> String {
    let what = match source.kind {
        MemorySourceKind::Thread => {
            format!("Thread {}", short(&source.reference, UID_CHARS))
        }
        MemorySourceKind::Commit => {
            format!("Commit {}", short(&source.reference, COMMIT_CHARS))
        }
        MemorySourceKind::File => format!("File {}", source.reference),
    };
    match (source.entry_from, source.entry_to) {
        (Some(from), Some(to)) if from == to => format!("{what}, entry {from}"),
        (Some(from), Some(to)) => format!("{what}, entries {from} to {to}"),
        (Some(from), None) => format!("{what}, from entry {from}"),
        (None, Some(to)) => format!("{what}, to entry {to}"),
        (None, None) => what,
    }
}

/// The first `count` characters of a reference, the part a label shows.
fn short(reference: &str, count: usize) -> String {
    reference.chars().take(count).collect()
}

/// Where a source opens (D190): a thread at its entry range, a commit in History, a file in the
/// Code page. A source of another repository is shown, never opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceTarget {
    /// The thread `uid`, read from entry `from_seq` on.
    Thread { uid: String, from_seq: u32 },
    /// The commit with this id.
    Commit(String),
    /// The file at this repository-relative path.
    File(String),
    /// The source is in a repository other than the open one, or the open one is not known.
    OtherRepository,
}

/// Where a source opens, given the id of the open repository (`None` when it is not known).
#[must_use]
pub fn source_target(source: &MemorySource, repo_id: Option<&str>) -> SourceTarget {
    let here = match (source.repo_id.as_deref(), repo_id) {
        (None, _) => true,
        (Some(theirs), Some(ours)) => theirs == ours,
        (Some(_), None) => false,
    };
    if !here {
        return SourceTarget::OtherRepository;
    }
    let reference = source.reference.clone();
    match source.kind {
        MemorySourceKind::Thread => SourceTarget::Thread {
            uid: reference,
            from_seq: u32::try_from(source.entry_from.unwrap_or(0).max(0)).unwrap_or(0),
        },
        MemorySourceKind::Commit => SourceTarget::Commit(reference),
        MemorySourceKind::File => SourceTarget::File(reference),
    }
}

/// The note a source shows when it cannot be opened here.
#[must_use]
pub fn source_note(target: &SourceTarget) -> Option<&'static str> {
    match target {
        SourceTarget::OtherRepository => Some("In another repository"),
        _ => None,
    }
}

/// Which detail actions a memory offers (D190): a proposal is accepted, edited, rejected or used
/// to supersede; any loaded memory can be deleted. Nothing is offered with no memory selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailAction {
    Accept,
    Reject,
    Edit,
    Supersede,
    MoveToContext,
    AttachToContext,
    DetachFromContext,
    Delete,
}

/// The detail actions the memory shown offers, in the order the pane shows them. A place action
/// is offered only when the engine takes it for this memory (D193 (8)).
#[must_use]
pub fn detail_actions(memory: Option<&Memory>) -> Vec<DetailAction> {
    let Some(memory) = memory else {
        return Vec::new();
    };
    let mut actions = if memory.status == MemoryStatus::Proposed {
        vec![
            DetailAction::Accept,
            DetailAction::Reject,
            DetailAction::Edit,
            DetailAction::Supersede,
        ]
    } else {
        Vec::new()
    };
    let place = memories::Placement::of(memory);
    if memories::check_move(place).is_ok() {
        actions.push(DetailAction::MoveToContext);
    }
    if memories::check_attach(place).is_ok() {
        actions.push(DetailAction::AttachToContext);
    }
    if memories::check_detach(place).is_ok() {
        actions.push(DetailAction::DetachFromContext);
    }
    actions.push(DetailAction::Delete);
    actions
}

/// The chip a memory's basis shows (D201 (1)). An observed memory, the default, shows none, so the
/// list is not noisier; a tested or an unconfirmed memory is named.
#[must_use]
pub fn basis_chip(basis: MemoryBasis) -> Option<&'static str> {
    match basis {
        MemoryBasis::Observed => None,
        MemoryBasis::Tested | MemoryBasis::Unconfirmed => Some(memories::basis_label(basis)),
    }
}

/// Whether a memory names a file that changed since it was accepted (D192 (6)).
#[must_use]
pub fn is_stale(memory: &Memory) -> bool {
    memories::files_changed(&memory.files_changed_since).is_stale()
}

/// The label of the bulk accept of the chosen proposals, none when nothing is chosen.
#[must_use]
pub fn accept_selected_label(chosen: usize) -> Option<String> {
    (chosen > 0).then(|| format!("Accept selected ({chosen})"))
}

/// The label of the bulk accept of one author's proposals.
#[must_use]
pub fn accept_author_label(author: &str) -> String {
    format!("Accept all from {}", memories::origin_label(author))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::threads::{ContextRepo, MemorySource, MemorySourceKind};

    fn memory(uid: &str, status: MemoryStatus, origin: &str, text: &str) -> Memory {
        Memory {
            id: 0,
            uid: uid.to_owned(),
            kind: MemoryKind::Rule,
            text: text.to_owned(),
            status,
            origin: origin.to_owned(),
            proposed_by: None,
            model: None,
            created_at: 1_000,
            decided_at: None,
            decided_by: None,
            reviewed: status != MemoryStatus::Proposed,
            sources: Vec::new(),
            supersedes: Vec::new(),
            superseded_by: Vec::new(),
            files_changed_since: Vec::new(),
            scope: "repository".to_owned(),
            contexts: Vec::new(),
            basis: MemoryBasis::Observed,
        }
    }

    fn source(kind: MemorySourceKind, reference: &str) -> MemorySource {
        MemorySource {
            kind,
            reference: reference.to_owned(),
            repo_id: None,
            entry_from: None,
            entry_to: None,
            blob_oid: None,
            blob_read: false,
        }
    }

    fn context(path: &str) -> Context {
        Context {
            id: 1,
            uid: "c1".to_owned(),
            name: path.to_owned(),
            parent: None,
            path: path.to_owned(),
            repos: vec![ContextRepo {
                repo_id: "r1".to_owned(),
                path: "/a".to_owned(),
                name: "a".to_owned(),
            }],
            accepted_memories: 0,
            sub_contexts: 0,
        }
    }

    #[test]
    fn only_a_tested_or_an_unconfirmed_memory_shows_its_basis() {
        assert_eq!(basis_chip(MemoryBasis::Observed), None);
        assert_eq!(basis_chip(MemoryBasis::Tested), Some("Tested"));
        assert_eq!(basis_chip(MemoryBasis::Unconfirmed), Some("Unconfirmed"));
    }

    #[test]
    fn the_badge_is_hidden_at_none_and_counts_the_proposals() {
        assert_eq!(tab_badge(0), None);
        assert_eq!(tab_badge(3), Some("3".to_owned()));
    }

    #[test]
    fn the_count_label_reads_its_rows_and_a_plus_when_more_follow() {
        assert_eq!(count_label(1, false), "1 memory");
        assert_eq!(count_label(0, false), "0 memories");
        assert_eq!(count_label(50, true), "50+ memories");
    }

    #[test]
    fn a_snippet_is_the_first_line_and_says_when_more_follows() {
        assert_eq!(snippet("Use retries.\nSecond line"), "Use retries. …");
        assert_eq!(snippet("  One line  "), "One line");
        assert_eq!(snippet("\n  \n"), "");
        let long = "x".repeat(SNIPPET_CHARS + 5);
        assert!(snippet(&long).ends_with(" …"));
        assert_eq!(snippet(&long).chars().count(), SNIPPET_CHARS + 2);
    }

    #[test]
    fn the_remaining_characters_count_down_to_the_limit_and_then_up() {
        assert_eq!(remaining_text(0), "1000 characters left");
        assert_eq!(remaining_text(999), "1 character left");
        assert_eq!(remaining_text(1000), "0 characters left");
        assert_eq!(remaining_text(1001), "1 character over the limit");
        assert_eq!(remaining_text(1003), "3 characters over the limit");
        assert!(!over_limit(1000));
        assert!(over_limit(1001));
    }

    #[test]
    fn the_scope_chip_names_the_choice_and_cycles_through_the_contexts() {
        let contexts = [context("Platform / Billing")];
        let options = scope_options(&contexts);
        assert_eq!(scope_label(&options[0]), "Everything");
        assert_eq!(scope_label(&options[1]), "This repository");
        assert_eq!(scope_label(&options[2]), "Platform / Billing");
        assert_eq!(
            next_choice(&options, &Scope::Repository),
            Some(options[2].clone())
        );
        assert_eq!(
            next_choice(&options, &options[2].clone()),
            Some(Scope::Everything)
        );
    }

    #[test]
    fn a_choice_cycles_and_an_unknown_one_starts_again_at_the_first() {
        assert_eq!(next_choice(&KINDS, &None), Some(Some(MemoryKind::Decision)));
        assert_eq!(next_choice(&KINDS, &Some(MemoryKind::Fact)), Some(None));
        assert_eq!(
            next_choice(&STATUSES, &Some(MemoryStatus::Rejected)),
            Some(Some(MemoryStatus::Superseded))
        );
        assert_eq!(next_choice::<u8>(&[], &1), None);
        assert_eq!(next_choice(&[1, 2], &9), Some(1));
    }

    #[test]
    fn the_author_chip_offers_the_loaded_origins_and_the_chosen_one() {
        let loaded = [
            memory("a", MemoryStatus::Proposed, "agent:claude-code", "x"),
            memory("b", MemoryStatus::Accepted, "human", "y"),
        ];
        assert_eq!(
            author_options(&loaded, Some("imported:ana")),
            vec![
                None,
                Some("agent:claude-code".to_owned()),
                Some("human".to_owned()),
                Some("imported:ana".to_owned()),
            ]
        );
        assert_eq!(chip_text("Author", None), "Author: Any");
        assert_eq!(chip_text("Kind", Some("Rule".to_owned())), "Kind: Rule");
    }

    #[test]
    fn a_path_filter_is_trimmed_and_blank_is_none() {
        assert_eq!(path_filter("   "), None);
        assert_eq!(path_filter(" src/a.rs "), Some("src/a.rs".to_owned()));
    }

    #[test]
    fn a_source_label_names_its_kind_and_its_entries() {
        let mut thread = source(MemorySourceKind::Thread, "abcdef0123456789");
        assert_eq!(source_label(&thread), "Thread abcdef01");
        thread.entry_from = Some(3);
        thread.entry_to = Some(5);
        assert_eq!(source_label(&thread), "Thread abcdef01, entries 3 to 5");
        thread.entry_to = Some(3);
        assert_eq!(source_label(&thread), "Thread abcdef01, entry 3");
        let commit = source(MemorySourceKind::Commit, "0123456789abcdef");
        assert_eq!(source_label(&commit), "Commit 0123456789ab");
        assert_eq!(
            source_label(&source(MemorySourceKind::File, "src/lib.rs")),
            "File src/lib.rs"
        );
    }

    #[test]
    fn a_source_opens_here_and_is_not_opened_in_another_repository() {
        let mut file = source(MemorySourceKind::File, "src/lib.rs");
        assert_eq!(
            source_target(&file, Some("here")),
            SourceTarget::File("src/lib.rs".to_owned())
        );
        file.repo_id = Some("here".to_owned());
        assert_eq!(
            source_target(&file, Some("here")),
            SourceTarget::File("src/lib.rs".to_owned())
        );
        file.repo_id = Some("there".to_owned());
        assert_eq!(
            source_target(&file, Some("here")),
            SourceTarget::OtherRepository
        );
        assert_eq!(
            source_note(&SourceTarget::OtherRepository),
            Some("In another repository")
        );
        assert_eq!(source_note(&SourceTarget::File("a".to_owned())), None);
        file.repo_id = Some("here".to_owned());
        assert_eq!(source_target(&file, None), SourceTarget::OtherRepository);
    }

    #[test]
    fn a_thread_source_opens_at_its_first_entry_and_a_commit_in_history() {
        let mut thread = source(MemorySourceKind::Thread, "t1");
        assert_eq!(
            source_target(&thread, None),
            SourceTarget::Thread {
                uid: "t1".to_owned(),
                from_seq: 0,
            }
        );
        thread.entry_from = Some(7);
        assert_eq!(
            source_target(&thread, None),
            SourceTarget::Thread {
                uid: "t1".to_owned(),
                from_seq: 7,
            }
        );
        thread.entry_from = Some(-4);
        assert_eq!(
            source_target(&thread, None),
            SourceTarget::Thread {
                uid: "t1".to_owned(),
                from_seq: 0,
            }
        );
        assert_eq!(
            source_target(&source(MemorySourceKind::Commit, "abc"), None),
            SourceTarget::Commit("abc".to_owned())
        );
    }

    #[test]
    fn a_proposal_offers_the_review_actions_and_an_accepted_memory_only_delete() {
        assert!(detail_actions(None).is_empty());
        let proposal = memory("p", MemoryStatus::Proposed, "agent:x", "t");
        assert_eq!(
            detail_actions(Some(&proposal)),
            vec![
                DetailAction::Accept,
                DetailAction::Reject,
                DetailAction::Edit,
                DetailAction::Supersede,
                DetailAction::MoveToContext,
                DetailAction::Delete,
            ]
        );
        let accepted = memory("a", MemoryStatus::Accepted, "human", "t");
        assert_eq!(
            detail_actions(Some(&accepted)),
            vec![DetailAction::MoveToContext, DetailAction::Delete]
        );
    }

    /// A context of the memory, as the engine lists it (D193 (1)).
    fn attached_to(id: i64, path: &str) -> gm_schema::threads::MemoryContext {
        gm_schema::threads::MemoryContext {
            id,
            path: path.to_owned(),
        }
    }

    #[test]
    fn a_context_memory_offers_attach_only_when_accepted_and_detach_when_it_has_a_context() {
        let mut proposal = memory("p", MemoryStatus::Proposed, "agent:x", "t");
        proposal.scope = "Platform".to_owned();
        proposal.contexts = vec![attached_to(7, "Platform")];
        assert_eq!(
            detail_actions(Some(&proposal)),
            vec![
                DetailAction::Accept,
                DetailAction::Reject,
                DetailAction::Edit,
                DetailAction::Supersede,
                DetailAction::DetachFromContext,
                DetailAction::Delete,
            ]
        );
        let mut accepted = memory("a", MemoryStatus::Accepted, "human", "t");
        accepted.scope = "Platform".to_owned();
        accepted.contexts = vec![attached_to(7, "Platform"), attached_to(9, "Partners")];
        assert_eq!(
            detail_actions(Some(&accepted)),
            vec![
                DetailAction::AttachToContext,
                DetailAction::DetachFromContext,
                DetailAction::Delete,
            ]
        );
    }

    #[test]
    fn a_context_memory_with_no_visible_context_is_not_offered_a_detach() {
        let mut accepted = memory("a", MemoryStatus::Accepted, "human", "t");
        accepted.scope = "Platform".to_owned();
        assert_eq!(
            detail_actions(Some(&accepted)),
            vec![DetailAction::AttachToContext, DetailAction::Delete]
        );
    }

    #[test]
    fn a_memory_is_stale_only_when_a_file_it_names_changed_or_went() {
        let mut stale = memory("s", MemoryStatus::Accepted, "human", "t");
        assert!(!is_stale(&stale));
        stale.files_changed_since = vec![gm_schema::threads::FileChange {
            path: "src/a.rs".to_owned(),
            state: gm_schema::threads::FileChangeState::Changed,
        }];
        assert!(is_stale(&stale));
    }

    #[test]
    fn the_bulk_labels_name_the_count_and_the_author() {
        assert_eq!(accept_selected_label(0), None);
        assert_eq!(
            accept_selected_label(2),
            Some("Accept selected (2)".to_owned())
        );
        assert_eq!(
            accept_author_label("agent:claude-code"),
            "Accept all from Agent: claude-code"
        );
    }
}

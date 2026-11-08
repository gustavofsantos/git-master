//! The checks of the New Memory form (D190), with no GPUI in them: a source as typed is turned
//! into the input the model plans, and the form's kind, scope, text and sources become the
//! request that saves it. The model checks the text and the sources again; this file gives the
//! form's own words for what the person typed (an entry that is not a number, a range that runs
//! backwards, a source with entries that is not a thread).

use gm_schema::threads::{MemoryBasis, MemoryKind, MemorySourceInput, MemorySourceKind};

use crate::memories::{MemoriesPage, NewMemory, Request, Scope};

const NOT_AN_ENTRY: &str = "An entry is a whole number from 0, such as 3.";
const NO_FIRST_ENTRY: &str = "A range needs a first entry.";
const ENTRIES_OF_A_THREAD: &str = "Only a thread source has entries.";
const BAD_RANGE: &str = "A thread range starts after it ends.";
const EMPTY_REFERENCE: &str = "A source needs a thread, a commit or a file.";

/// The kinds a memory can take, in the order the form shows them.
pub const KIND_CHOICES: [MemoryKind; 4] = [
    MemoryKind::Decision,
    MemoryKind::Rule,
    MemoryKind::Lesson,
    MemoryKind::Fact,
];

/// The bases a memory can take, in the order the form shows them. Observed is the default (D201 (1)).
pub const BASIS_CHOICES: [MemoryBasis; 3] = [
    MemoryBasis::Observed,
    MemoryBasis::Tested,
    MemoryBasis::Unconfirmed,
];

/// What a basis means, under its chip in the form: one short line, in the form's own words.
#[must_use]
pub fn basis_hint(basis: MemoryBasis) -> &'static str {
    match basis {
        MemoryBasis::Observed => "Seen in a thread or a file.",
        MemoryBasis::Tested => "Checked by running something.",
        MemoryBasis::Unconfirmed => {
            "A belief nothing has confirmed. It is marked, and never taken as fact."
        }
    }
}

/// The kinds of source a memory can name, in the order the form shows them.
pub const SOURCE_CHOICES: [MemorySourceKind; 3] = [
    MemorySourceKind::Thread,
    MemorySourceKind::Commit,
    MemorySourceKind::File,
];

/// A source as typed into the form, before it is checked. Its entries are text until then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDraft {
    pub kind: MemorySourceKind,
    pub reference: String,
    pub entry_from: String,
    pub entry_to: String,
}

/// The source a draft names, checked: a reference is given, the entries are whole numbers, a
/// range runs forward, and only a thread has entries. A range with a first entry and no last one
/// is that one entry.
pub fn source_input(draft: &SourceDraft) -> Result<MemorySourceInput, &'static str> {
    let reference = draft.reference.trim();
    if reference.is_empty() {
        return Err(EMPTY_REFERENCE);
    }
    let from = entry(&draft.entry_from)?;
    let to = entry(&draft.entry_to)?;
    let (entry_from, entry_to) = match (from, to) {
        (None, None) => (None, None),
        (None, Some(_)) => return Err(NO_FIRST_ENTRY),
        (Some(first), last) => (Some(first), Some(last.unwrap_or(first))),
    };
    if let (Some(first), Some(last)) = (entry_from, entry_to)
        && first > last
    {
        return Err(BAD_RANGE);
    }
    if draft.kind != MemorySourceKind::Thread && entry_from.is_some() {
        return Err(ENTRIES_OF_A_THREAD);
    }
    Ok(MemorySourceInput {
        kind: draft.kind,
        reference: reference.to_owned(),
        entry_from,
        entry_to,
    })
}

/// One entry number as typed: blank is none, else a whole number from 0.
fn entry(text: &str) -> Result<Option<i64>, &'static str> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<i64>() {
        Ok(number) if number >= 0 => Ok(Some(number)),
        _ => Err(NOT_AN_ENTRY),
    }
}

/// The request that saves the New Memory form: a memory of the human's, accepted at once (D190),
/// with the basis the form chose (D201 (1)). Refused with the reason the model gives for the text,
/// the scope or a source.
pub fn save_request(
    kind: MemoryKind,
    scope: Scope,
    text: &str,
    sources: &[MemorySourceInput],
    basis: MemoryBasis,
) -> Result<Request, &'static str> {
    MemoriesPage::plan_new(&NewMemory {
        kind,
        scope,
        text: text.to_owned(),
        sources: sources.to_vec(),
        basis,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::CommandKind;

    fn draft(kind: MemorySourceKind, reference: &str, from: &str, to: &str) -> SourceDraft {
        SourceDraft {
            kind,
            reference: reference.to_owned(),
            entry_from: from.to_owned(),
            entry_to: to.to_owned(),
        }
    }

    #[test]
    fn an_entry_is_blank_or_a_whole_number_from_zero() {
        assert_eq!(entry(" "), Ok(None));
        assert_eq!(entry(" 3 "), Ok(Some(3)));
        assert_eq!(entry("0"), Ok(Some(0)));
        assert_eq!(entry("-1"), Err(NOT_AN_ENTRY));
        assert_eq!(entry("two"), Err(NOT_AN_ENTRY));
        assert_eq!(entry("1.5"), Err(NOT_AN_ENTRY));
    }

    #[test]
    fn a_thread_range_is_checked_and_a_lone_entry_is_one_entry() {
        let range = source_input(&draft(MemorySourceKind::Thread, " t1 ", "2", "5")).unwrap();
        assert_eq!(range.reference, "t1");
        assert_eq!((range.entry_from, range.entry_to), (Some(2), Some(5)));
        let one = source_input(&draft(MemorySourceKind::Thread, "t1", "4", "")).unwrap();
        assert_eq!((one.entry_from, one.entry_to), (Some(4), Some(4)));
        let whole = source_input(&draft(MemorySourceKind::Thread, "t1", "", "")).unwrap();
        assert_eq!((whole.entry_from, whole.entry_to), (None, None));
    }

    #[test]
    fn a_source_that_names_nothing_or_runs_backwards_is_refused() {
        assert_eq!(
            source_input(&draft(MemorySourceKind::File, "  ", "", "")),
            Err(EMPTY_REFERENCE)
        );
        assert_eq!(
            source_input(&draft(MemorySourceKind::Thread, "t1", "", "3")),
            Err(NO_FIRST_ENTRY)
        );
        assert_eq!(
            source_input(&draft(MemorySourceKind::Thread, "t1", "5", "2")),
            Err(BAD_RANGE)
        );
        assert_eq!(
            source_input(&draft(MemorySourceKind::Commit, "abc", "1", "")),
            Err(ENTRIES_OF_A_THREAD)
        );
        assert_eq!(
            source_input(&draft(MemorySourceKind::File, "src/a.rs", "x", "")),
            Err(NOT_AN_ENTRY)
        );
    }

    #[test]
    fn saving_sends_a_human_proposal_for_the_repository_or_a_context() {
        let source = source_input(&draft(MemorySourceKind::File, "src/a.rs", "", "")).unwrap();
        let request = save_request(
            MemoryKind::Rule,
            Scope::Repository,
            "  Keep retries bounded. ",
            std::slice::from_ref(&source),
            MemoryBasis::Tested,
        )
        .unwrap();
        match request.kind {
            CommandKind::ProposeMemory {
                kind,
                text,
                sources,
                context,
                basis,
            } => {
                assert_eq!(kind, MemoryKind::Rule);
                assert_eq!(text, "Keep retries bounded.");
                assert_eq!(sources, vec![source.clone()]);
                assert_eq!(context, None);
                assert_eq!(basis, MemoryBasis::Tested);
            }
            other => panic!("not a proposal: {other:?}"),
        }
        let in_context = save_request(
            MemoryKind::Fact,
            Scope::Context("Platform / Billing".to_owned()),
            "A fact.",
            &[],
            MemoryBasis::Observed,
        )
        .unwrap();
        assert!(matches!(
            in_context.kind,
            CommandKind::ProposeMemory { context: Some(ref path), .. } if path == "Platform / Billing"
        ));
    }

    #[test]
    fn the_form_offers_three_bases_observed_first_and_each_has_a_hint() {
        assert_eq!(
            BASIS_CHOICES,
            [
                MemoryBasis::Observed,
                MemoryBasis::Tested,
                MemoryBasis::Unconfirmed
            ]
        );
        assert_eq!(MemoryBasis::default(), BASIS_CHOICES[0]);
        for basis in BASIS_CHOICES {
            assert!(!basis_hint(basis).is_empty());
        }
        assert!(basis_hint(MemoryBasis::Unconfirmed).contains("never taken as fact"));
    }

    #[test]
    fn saving_refuses_an_empty_or_long_text_and_no_scope_to_write_to() {
        assert_eq!(
            save_request(
                MemoryKind::Fact,
                Scope::Repository,
                "  ",
                &[],
                MemoryBasis::Observed
            )
            .err(),
            Some("A memory needs some text.")
        );
        let long = "x".repeat(1001);
        assert_eq!(
            save_request(
                MemoryKind::Fact,
                Scope::Repository,
                &long,
                &[],
                MemoryBasis::Observed
            )
            .err(),
            Some("A memory holds at most 1,000 characters.")
        );
        assert_eq!(
            save_request(
                MemoryKind::Fact,
                Scope::Everything,
                "text",
                &[],
                MemoryBasis::Observed
            )
            .err(),
            Some("Choose this repository or a context for the memory.")
        );
    }
}

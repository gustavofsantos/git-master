//! Transforms of field values (design/workflow-scripts.md, `WF3d`, D155): which `transform.<field>`
//! workflows run for a field, what each one reads as `input.trigger`, and how their answers fold
//! into the value of the field. Pure: the engine loads and runs the scripts and acts on the answers,
//! so every rule has a test without a repository or a window.

use serde_json::{Value, json};

use super::loader::sha256_hex;

use super::journal::RunStatus;
use super::phases::one_line;
use super::triggers::TriggerEvent;

/// The fields a transform builds in this slice: the commit message and the new branch's name.
pub const FIELDS: [&str; 2] = ["commit_message", "branch_name"];

/// Whether a transform runs for `field` in this slice.
#[must_use]
pub fn is_field(field: &str) -> bool {
    FIELDS.contains(&field)
}

/// The event a transform of `field` starts: `transform.<field>`, with the value so far as
/// `current`. `branch` is the branch HEAD is on, which `match` looks at.
#[must_use]
pub fn trigger(field: &str, branch: Option<String>, current: &str) -> TriggerEvent {
    TriggerEvent {
        on: format!("transform.{field}"),
        branch,
        payload: json!({ "field": field, "current": current }),
    }
}

/// What the repository looked like when a transform started: HEAD and the staged diff. Compared
/// when the transform answers, for `changed` (D155(1)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    /// The commit HEAD points at; `None` on an unborn branch.
    pub head: Option<String>,
    /// sha256 of the staged diff. The index is compared through its diff, so a restat that
    /// changes nothing staged does not count as a change.
    pub staged: String,
}

#[must_use]
pub fn fingerprint(head: Option<&str>, staged_diff: &str) -> Fingerprint {
    Fingerprint {
        head: head.map(str::to_owned),
        staged: sha256_hex(staged_diff.as_bytes()),
    }
}

/// The notice of a transform that is not reviewed: it does not run (the text `WF3a` shows).
#[must_use]
pub fn unreviewed(name: &str, on: &str) -> String {
    format!("{name} did not run on {on}: review it first.")
}

/// The value of a field while its transforms run in sequence, and what they said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sequence {
    /// The value so far: the person's text, then the answer of each transform that gave one.
    pub current: String,
    /// Whether a transform gave a value.
    pub produced: bool,
    /// The notices the transforms so far raised, not yet taken by the engine.
    notices: Vec<String>,
}

impl Sequence {
    /// The sequence of a field whose text is `current` before any transform ran.
    #[must_use]
    pub fn new(current: &str) -> Self {
        Self {
            current: current.to_owned(),
            produced: false,
            notices: Vec::new(),
        }
    }

    /// The transform `name` ended with `status`, `reason` and `value` (what `execute` returned).
    /// A non-empty text becomes the new value; `nil` and an empty text leave it; a run that did not
    /// finish is skipped with a notice, and the sequence goes on.
    #[must_use]
    pub fn advance(
        mut self,
        name: &str,
        status: RunStatus,
        reason: Option<&str>,
        value: Option<&Value>,
    ) -> Self {
        if status != RunStatus::Done {
            self.notices.push(failure(name, reason));
            return self;
        }
        match value {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) if text.trim().is_empty() => {}
            Some(Value::String(text)) => {
                text.clone_into(&mut self.current);
                self.produced = true;
            }
            Some(_) => self.notices.push(format!(
                "{name} failed: it returned a value that is not text"
            )),
        }
        self
    }

    /// Records a transform that did not run at all, with the notice to show.
    #[must_use]
    pub fn skipped(mut self, notice: String) -> Self {
        self.notices.push(notice);
        self
    }

    /// The notices raised since the last call, in order.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    /// The value of the field, when a transform gave one.
    #[must_use]
    pub fn value(&self) -> Option<String> {
        self.produced.then(|| self.current.clone())
    }
}

/// The notice of a transform that failed: its name and the reason, on one line.
fn failure(name: &str, reason: Option<&str>) -> String {
    let reason = one_line(reason.unwrap_or(""));
    if reason.is_empty() {
        format!("{name} failed: no reason was given")
    } else {
        format!("{name} failed: {reason}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(seq: Sequence, name: &str, text: &str) -> Sequence {
        seq.advance(name, RunStatus::Done, None, Some(&json!(text)))
    }

    #[test]
    fn only_the_two_fields_of_this_slice_have_transforms() {
        assert!(is_field("commit_message"));
        assert!(is_field("branch_name"));
        assert!(!is_field("conflict_hunk"));
        assert!(!is_field(""));
    }

    #[test]
    fn a_transform_event_names_its_field_and_carries_the_value_so_far() {
        let event = trigger("commit_message", Some("feat/x".into()), "draft");
        assert_eq!(event.on, "transform.commit_message");
        assert_eq!(event.branch.as_deref(), Some("feat/x"));
        assert_eq!(
            event.trigger_value(),
            json!({ "on": "transform.commit_message", "field": "commit_message", "current": "draft" })
        );
    }

    #[test]
    fn one_answer_becomes_the_value() {
        let seq = done(Sequence::new(""), "llm", "feat: from a workflow");
        assert_eq!(seq.value().as_deref(), Some("feat: from a workflow"));
        assert!(seq.produced);
    }

    #[test]
    fn the_second_transform_sees_the_first_answer_and_appends_to_it() {
        let seq = done(Sequence::new(""), "a", "one");
        assert_eq!(seq.current, "one", "the second reads this as current");
        let seq = done(seq, "b", "one+two");
        assert_eq!(seq.value().as_deref(), Some("one+two"));
    }

    #[test]
    fn nil_and_an_empty_text_leave_the_value_as_it_was() {
        let seq = Sequence::new("typed");
        let seq = seq.advance("a", RunStatus::Done, None, None);
        let seq = seq.advance("b", RunStatus::Done, None, Some(&Value::Null));
        let seq = done(seq, "c", "");
        let seq = done(seq, "d", "  \n ");
        assert_eq!(seq.current, "typed");
        assert_eq!(seq.value(), None, "nothing was produced");
    }

    #[test]
    fn a_failing_transform_is_skipped_with_a_notice_and_the_next_still_answers() {
        let mut seq = Sequence::new("").advance(
            "broken",
            RunStatus::Failed,
            Some("the tool\n  is missing"),
            None,
        );
        assert_eq!(seq.current, "");
        assert_eq!(seq.take_notices(), ["broken failed: the tool is missing"]);
        let seq = done(seq, "next", "feat: x");
        assert_eq!(seq.value().as_deref(), Some("feat: x"));
    }

    #[test]
    fn a_blocked_or_interrupted_transform_is_a_failure_too() {
        let mut seq = Sequence::new("")
            .advance("a", RunStatus::Blocked, Some("no"), None)
            .advance("b", RunStatus::Interrupted, None, None);
        assert_eq!(
            seq.take_notices(),
            ["a failed: no", "b failed: no reason was given"]
        );
    }

    #[test]
    fn a_value_that_is_not_text_is_a_failure() {
        let mut seq = Sequence::new("typed").advance(
            "odd",
            RunStatus::Done,
            None,
            Some(&json!({ "message": "x" })),
        );
        assert_eq!(seq.current, "typed");
        assert_eq!(
            seq.take_notices(),
            ["odd failed: it returned a value that is not text"]
        );
    }

    #[test]
    fn notices_are_taken_once() {
        let mut seq = Sequence::new("").skipped(unreviewed("llm", "transform.commit_message"));
        assert_eq!(
            seq.take_notices(),
            ["llm did not run on transform.commit_message: review it first."]
        );
        assert!(seq.take_notices().is_empty());
    }

    #[test]
    fn with_no_transform_the_text_is_not_a_value() {
        assert_eq!(Sequence::new("typed").value(), None);
    }

    #[test]
    fn the_fingerprint_moves_with_head_or_the_staged_diff() {
        let base = fingerprint(Some("a1"), "diff one");
        assert_eq!(base, fingerprint(Some("a1"), "diff one"));
        assert_ne!(base, fingerprint(Some("b2"), "diff one"));
        assert_ne!(base, fingerprint(Some("a1"), "diff two"));
        assert_ne!(base, fingerprint(None, "diff one"));
    }
}

//! The window's view of the transform workflows (`WF3d`, D155): the commit message and the new
//! branch's name, which a `transform.<field>` workflow may fill in. Pure: the rules that decide what
//! Suggest Message sends, when the composer reports that it opened, and what an answer does with
//! the text the person typed. The window reads the composer and the dialog and passes their text in,
//! so the rules are tested without a window.

use gm_schema::{CommandKind, Event, OperationId, WorkflowInfo};

use crate::hook_run::Fill;

/// A field a transform builds: the commit message, or the name of a new branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    CommitMessage,
    BranchName,
}

impl Field {
    /// The field's name in the engine: the `<field>` of `transform.<field>`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::CommitMessage => "commit_message",
            Self::BranchName => "branch_name",
        }
    }

    /// The field a request names, when the window fills it.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "commit_message" => Some(Self::CommitMessage),
            "branch_name" => Some(Self::BranchName),
            _ => None,
        }
    }
}

/// What a surface shows while a transform waits for its workflow (`WF3d`).
pub(crate) const WAITING: &str = "Waiting for a workflow…";

/// The link that cancels the waiting transform. Cancel leaves the field as it was.
pub(crate) const CANCEL: &str = "Cancel suggestion";

/// Whether a loaded workflow transforms `field`: it loads and its trigger names the field.
#[must_use]
pub fn loaded(listed: Option<&[WorkflowInfo]>, field: Field) -> bool {
    let on = format!("transform.{}", field.name());
    listed.is_some_and(|listed| {
        listed
            .iter()
            .any(|info| info.error.is_none() && info.trigger == on)
    })
}

/// What Suggest Message sends (D155(1)). With a `transform.commit_message` workflow loaded it runs
/// the transforms; otherwise it runs the `prepare-commit` chain, as before. Both paths coexist until
/// `WF3e` removes the hooks.
#[must_use]
pub fn suggest_message(message: &str, transform_loaded: bool) -> CommandKind {
    if transform_loaded {
        CommandKind::RunTransform {
            field: Field::CommitMessage.name().to_owned(),
            current: message.to_owned(),
        }
    } else {
        CommandKind::RunPrepareCommit {
            message: message.to_owned(),
        }
    }
}

/// The key of the staged state: HEAD and the staged paths, in the order the status lists them.
/// `None` when nothing is staged. Staging nothing new keeps the key; a new path changes it.
#[must_use]
pub fn staged_key<'a>(
    head: Option<&str>,
    staged: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let paths: Vec<&str> = staged.into_iter().collect();
    if paths.is_empty() {
        return None;
    }
    Some(format!("{}\n{}", head.unwrap_or(""), paths.join("\n")))
}

/// What an answer does with the field (`WF3d`). A transform writes the field only when the field
/// was empty when the request was sent, is still empty, and nothing changed meanwhile. Otherwise
/// the answer waits as a suggestion, since the person's text is never overwritten (the docs of the
/// working copy and of the triggers). An answer equal to the text is applied, as it changes nothing.
#[must_use]
pub fn fill_for_field(sent: &str, typed: &str, changed: bool, value: &str) -> Fill {
    let untouched = !changed && typed == sent && sent.trim().is_empty();
    if untouched || typed == value {
        Fill::Apply(value.to_owned())
    } else {
        Fill::Suggest(value.to_owned())
    }
}

/// The transform the window waits for: its operation, the field, and the text the field held when
/// the request was sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub id: OperationId,
    pub field: Field,
    pub sent: String,
}

/// What the window keeps of the transforms: the request in flight and the staged state the composer
/// last reported.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Transforms {
    pending: Option<Pending>,
    /// The staged state the composer reported (`ComposerOpened`) for, so it is reported once.
    opened_for: Option<String>,
}

impl Transforms {
    /// Forgets what belonged to the repository being left.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// A transform request was sent for `field`, with `sent` in the field.
    pub fn start(&mut self, id: OperationId, field: Field, sent: String) {
        self.pending = Some(Pending { id, field, sent });
    }

    /// The field the transform `id` fills, when it is the one the window waits for.
    #[must_use]
    pub fn field_of(&self, id: OperationId) -> Option<Field> {
        self.pending
            .as_ref()
            .filter(|pending| pending.id == id)
            .map(|pending| pending.field)
    }

    /// The operation Cancel stops for `field`: the transform in flight for it. `None` when none is,
    /// so no surface offers Cancel for an answer that already came.
    #[must_use]
    pub fn cancel_target(&self, field: Field) -> Option<OperationId> {
        self.pending
            .as_ref()
            .filter(|pending| pending.field == field)
            .map(|pending| pending.id)
    }

    /// Whether a transform for `field` is in flight.
    #[must_use]
    pub fn waiting_for(&self, field: Field) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.field == field)
    }

    /// Whether the composer's focus reports that it opened (D155(1)): the message box is empty, a
    /// change is staged, and this staged state was not reported yet. Records the state when it
    /// says so. Staging alone never reports; only the focus does.
    pub fn composer_opened(&mut self, typed: &str, staged: Option<&str>) -> bool {
        let (true, Some(key)) = (typed.trim().is_empty(), staged) else {
            return false;
        };
        if self.opened_for.as_deref() == Some(key) {
            return false;
        }
        self.opened_for = Some(key.to_owned());
        true
    }

    /// The transform `id` ended with `value` (`None`: nothing). Returns the field and what to do
    /// with the answer, given the field's text now (see `fill_for_field`). `None` when `id` is not
    /// the transform in flight or it answered nothing.
    pub fn finish(
        &mut self,
        id: OperationId,
        typed: &str,
        value: Option<String>,
        changed: bool,
    ) -> Option<(Field, Fill)> {
        let pending = self.take(id)?;
        let value = value?;
        Some((
            pending.field,
            fill_for_field(&pending.sent, typed, changed, &value),
        ))
    }

    /// An operation ended: a transform in flight that ends without an answer is forgotten, so a
    /// later answer is never taken for it.
    pub fn settle_event(&mut self, event: &Event) {
        if let Event::OperationChanged { id, state, .. } = event
            && state.is_terminal()
        {
            self.take(*id);
        }
    }

    fn take(&mut self, id: OperationId) -> Option<Pending> {
        if self.field_of(id).is_some() {
            self.pending.take()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use gm_schema::WorkflowInput;
    use gm_schema::{Actor, OperationState};

    use super::*;

    fn id(n: u64) -> OperationId {
        OperationId(n)
    }

    fn workflow(name: &str, trigger: &str, error: Option<&str>) -> WorkflowInfo {
        WorkflowInfo {
            name: name.into(),
            path: format!("/w/{name}.lua"),
            trigger: trigger.into(),
            key: None,
            inputs: Vec::<WorkflowInput>::new(),
            hash: "h".into(),
            error: error.map(str::to_owned),
            warnings: Vec::new(),
            source: String::new(),
        }
    }

    #[test]
    fn suggest_message_runs_the_transforms_when_one_is_loaded() {
        assert_eq!(
            suggest_message("draft", true),
            CommandKind::RunTransform {
                field: "commit_message".into(),
                current: "draft".into(),
            }
        );
    }

    #[test]
    fn suggest_message_keeps_the_prepare_commit_path_without_a_transform() {
        assert_eq!(
            suggest_message("draft", false),
            CommandKind::RunPrepareCommit {
                message: "draft".into(),
            }
        );
    }

    #[test]
    fn a_transform_is_loaded_for_its_field_only_and_only_when_it_loads() {
        let listed = [
            workflow("llm", "transform.commit_message", None),
            workflow("broken", "transform.branch_name", Some("line 3: bad")),
            workflow("ui", "composer.opened", None),
            workflow("run", "manual", None),
        ];
        assert!(loaded(Some(&listed), Field::CommitMessage));
        assert!(!loaded(Some(&listed), Field::BranchName), "does not load");
        assert!(!loaded(None, Field::CommitMessage), "not listed yet");
        assert!(!loaded(Some(&listed[2..]), Field::CommitMessage));
    }

    #[test]
    fn a_field_name_round_trips_and_an_unknown_one_is_none() {
        for field in [Field::CommitMessage, Field::BranchName] {
            assert_eq!(Field::from_name(field.name()), Some(field));
        }
        assert_eq!(Field::from_name("conflict_hunk"), None);
    }

    #[test]
    fn nothing_staged_has_no_staged_state() {
        assert_eq!(staged_key(Some("abc"), []), None);
    }

    #[test]
    fn the_staged_state_follows_head_and_the_staged_paths() {
        let one = staged_key(Some("abc"), ["a.txt"]);
        assert_eq!(one, staged_key(Some("abc"), ["a.txt"]));
        assert_ne!(one, staged_key(Some("abc"), ["a.txt", "b.txt"]));
        assert_ne!(one, staged_key(Some("def"), ["a.txt"]));
    }

    #[test]
    fn the_focus_reports_an_empty_box_with_a_staged_change_once() {
        let mut t = Transforms::default();
        assert!(t.composer_opened("", Some("k1")));
        assert!(
            !t.composer_opened("", Some("k1")),
            "not again for the same state"
        );
        assert!(
            t.composer_opened("", Some("k2")),
            "a new staged state reports"
        );
    }

    #[test]
    fn staging_alone_and_a_typed_box_do_not_report() {
        let mut t = Transforms::default();
        assert!(!t.composer_opened("", None), "nothing staged");
        assert!(!t.composer_opened("fix", Some("k1")), "the box has text");
        assert!(
            t.composer_opened("", Some("k1")),
            "a refused report is not recorded, so a later empty focus reports"
        );
    }

    #[test]
    fn a_typed_text_is_never_overwritten_even_when_it_did_not_change() {
        let mut t = Transforms::default();
        t.start(id(2), Field::CommitMessage, "typed".into());
        assert_eq!(
            t.finish(id(2), "typed", Some("feat: x".into()), false),
            Some((Field::CommitMessage, Fill::Suggest("feat: x".into())))
        );
        assert_eq!(
            fill_for_field("", "", false, ""),
            Fill::Apply(String::new()),
            "an answer equal to the text changes nothing"
        );
    }

    #[test]
    fn an_answer_goes_in_when_the_text_was_empty_and_nothing_changed() {
        let mut t = Transforms::default();
        t.start(id(3), Field::CommitMessage, String::new());
        assert_eq!(
            t.finish(id(3), "", Some("feat: x".into()), false),
            Some((Field::CommitMessage, Fill::Apply("feat: x".into())))
        );
        assert_eq!(t.field_of(id(3)), None, "an answer is taken once");
    }

    #[test]
    fn typed_text_or_a_changed_repository_turns_the_answer_into_a_suggestion() {
        let mut t = Transforms::default();
        t.start(id(3), Field::CommitMessage, String::new());
        assert_eq!(
            t.finish(id(3), "fix", Some("feat: x".into()), false),
            Some((Field::CommitMessage, Fill::Suggest("feat: x".into())))
        );
        t.start(id(4), Field::CommitMessage, String::new());
        assert_eq!(
            t.finish(id(4), "", Some("feat: x".into()), true),
            Some((Field::CommitMessage, Fill::Suggest("feat: x".into())))
        );
    }

    #[test]
    fn a_branch_name_fills_an_empty_field_and_suggests_over_typed_text() {
        let mut t = Transforms::default();
        t.start(id(5), Field::BranchName, String::new());
        assert_eq!(
            t.finish(id(5), "", Some("feat/login".into()), false),
            Some((Field::BranchName, Fill::Apply("feat/login".into())))
        );
        t.start(id(6), Field::BranchName, "login".into());
        assert_eq!(
            t.finish(id(6), "login", Some("feat/login".into()), false),
            Some((Field::BranchName, Fill::Suggest("feat/login".into()))),
            "typed text stays; Replace takes the name"
        );
    }

    #[test]
    fn no_value_does_nothing_and_clears_the_request() {
        let mut t = Transforms::default();
        t.start(id(7), Field::CommitMessage, String::new());
        assert_eq!(t.finish(id(7), "", None, false), None);
        assert_eq!(t.field_of(id(7)), None);
    }

    #[test]
    fn an_answer_to_another_operation_is_not_taken() {
        let mut t = Transforms::default();
        t.start(id(7), Field::CommitMessage, String::new());
        assert_eq!(t.finish(id(8), "", Some("late".into()), false), None);
        assert_eq!(
            t.field_of(id(7)),
            Some(Field::CommitMessage),
            "still waiting"
        );
    }

    #[test]
    fn a_request_that_ends_without_an_answer_is_forgotten() {
        let mut t = Transforms::default();
        t.start(id(9), Field::CommitMessage, String::new());
        let event = |state| Event::OperationChanged {
            id: id(9),
            actor: Actor::Human,
            state,
            detail: None,
        };
        t.settle_event(&event(OperationState::Executing));
        assert_eq!(
            t.field_of(id(9)),
            Some(Field::CommitMessage),
            "not ended yet"
        );
        t.settle_event(&event(OperationState::Cancelled));
        assert!(!t.waiting_for(Field::CommitMessage));
    }

    #[test]
    fn cancel_is_offered_for_the_field_in_flight_only() {
        let mut t = Transforms::default();
        assert_eq!(t.cancel_target(Field::CommitMessage), None);
        t.start(id(3), Field::CommitMessage, String::new());
        assert_eq!(t.cancel_target(Field::CommitMessage), Some(id(3)));
        assert_eq!(t.cancel_target(Field::BranchName), None, "another field");
        t.finish(id(3), "", None, false);
        assert_eq!(t.cancel_target(Field::CommitMessage), None, "answered");
        t.start(id(4), Field::BranchName, String::new());
        let cancelled = Event::OperationChanged {
            id: id(4),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        };
        t.settle_event(&cancelled);
        assert_eq!(t.cancel_target(Field::BranchName), None, "cancelled");
    }

    #[test]
    fn reset_forgets_the_request_and_the_reported_state() {
        let mut t = Transforms::default();
        t.start(id(1), Field::BranchName, String::new());
        assert!(t.composer_opened("", Some("k")));
        t.reset();
        assert_eq!(t, Transforms::default());
    }
}

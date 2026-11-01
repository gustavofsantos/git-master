//! The curation of the Labels page (P4, D145): what the New Label, Rename and Delete
//! dialogs check and send, and what the Remove button of an attachment sends. The
//! rules are pure and tested: the limits of a name, a description and a colour, the
//! commands, the words the delete confirmation says, and the tracker that holds a
//! write until the engine settles it. Label writes are Human-only (D119), so the
//! window sends them with `Actor::Human`. The window side is in `shell_labels.rs`.

use gm_schema::threads::{Label, LabelAttachment};
use gm_schema::{CommandKind, OperationId, OperationState};

use crate::action_registry::Reason;
use crate::labels_page::hex_colour;

/// The longest label name, in characters (D145).
pub(crate) const NAME_CHARS: usize = 64;
/// The longest description, in characters (D145).
pub(crate) const DESCRIPTION_CHARS: usize = 500;
const NAME_EMPTY: Reason = "A label needs a name.";
const NAME_LONG: Reason = "A label name is at most 64 characters.";
const NAME_CONTROL: Reason = "A label name cannot contain control characters.";
const NAME_TAKEN: Reason = "A label with this name already exists.";
const NAME_SAME: Reason = "The label already has this name.";
const DESCRIPTION_LONG: Reason = "A description is at most 500 characters.";
const COLOUR_INVALID: Reason = "A colour is #rrggbb, for example #ff8800.";

/// The name of a label, trimmed. It has 1 to 64 characters and no control characters,
/// and no other label has it: names are case-sensitive and unique. `editing` is the id
/// of the label being renamed, whose own name is then refused as unchanged.
pub(crate) fn name_of(
    text: &str,
    labels: &[Label],
    editing: Option<i64>,
) -> Result<String, Reason> {
    let name = text.trim();
    if name.is_empty() {
        return Err(NAME_EMPTY);
    }
    if name.chars().count() > NAME_CHARS {
        return Err(NAME_LONG);
    }
    if name.chars().any(char::is_control) {
        return Err(NAME_CONTROL);
    }
    if let Some(taken) = labels.iter().find(|label| label.name == name) {
        return Err(if Some(taken.id) == editing {
            NAME_SAME
        } else {
            NAME_TAKEN
        });
    }
    Ok(name.to_owned())
}

/// The description, trimmed. It may be empty, and is at most 500 characters.
pub(crate) fn description_of(text: &str) -> Result<String, Reason> {
    let description = text.trim();
    if description.chars().count() > DESCRIPTION_CHARS {
        Err(DESCRIPTION_LONG)
    } else {
        Ok(description.to_owned())
    }
}

/// The colour, trimmed and in lower case, or none when the text is blank. Any other
/// text must be `#rrggbb`, since the store keeps that form.
pub(crate) fn colour_of(text: &str) -> Result<Option<String>, Reason> {
    let colour = text.trim();
    if colour.is_empty() {
        return Ok(None);
    }
    let lower = colour.to_ascii_lowercase();
    if hex_colour(&lower).is_some() {
        Ok(Some(lower))
    } else {
        Err(COLOUR_INVALID)
    }
}

/// The command a New Label dialog sends for its three fields, or the reason the text is
/// refused. The name is checked against the loaded labels: the engine would return the
/// label of that name unchanged, so a duplicate is refused here.
pub(crate) fn new_label_command(
    name: &str,
    description: &str,
    colour: &str,
    labels: &[Label],
) -> Result<CommandKind, Reason> {
    let name = name_of(name, labels, None)?;
    let description = description_of(description)?;
    let colour = colour_of(colour)?;
    Ok(CommandKind::CreateLabel {
        name,
        description,
        color: colour,
    })
}

/// The command a Rename Label dialog sends for label `id`, or the reason the text is refused.
pub(crate) fn rename_label_command(
    id: i64,
    text: &str,
    labels: &[Label],
) -> Result<CommandKind, Reason> {
    Ok(rename_command(id, name_of(text, labels, Some(id))?))
}

/// The command that renames label `id`.
#[must_use]
pub(crate) fn rename_command(id: i64, name: String) -> CommandKind {
    CommandKind::RenameLabel { id, name }
}

/// The command that deletes label `id` with its attachments.
#[must_use]
pub(crate) fn delete_command(id: i64) -> CommandKind {
    CommandKind::DeleteLabel { id }
}

/// The command that removes one attachment from the label `label`, by its name.
#[must_use]
pub(crate) fn detach_command(label: &str, attachment: &LabelAttachment) -> CommandKind {
    CommandKind::DetachLabel {
        label: label.to_owned(),
        kind: attachment.kind,
        reference: attachment.reference.clone(),
    }
}

/// The title of the delete confirmation.
#[must_use]
pub(crate) fn delete_title(name: &str) -> String {
    format!("Delete label \"{name}\"?")
}

/// The body of the delete confirmation: how many attachments go with the label, then
/// that the delete cannot be undone.
#[must_use]
pub(crate) fn delete_lines(attachments: u32) -> Vec<String> {
    let count = match attachments {
        0 => "It has no attachments.".to_owned(),
        1 => "1 attachment is removed with it.".to_owned(),
        n => format!("{n} attachments are removed with it."),
    };
    vec![count, "This cannot be undone.".to_owned()]
}

/// The notice of a write that succeeded, by what it was.
#[must_use]
pub(crate) fn done_notice(write: Write) -> &'static str {
    match write {
        Write::Create => "Label created",
        Write::Rename => "Label renamed",
        Write::Delete => "Label deleted",
        Write::Detach => "Removed from label",
    }
}

/// What a label write is for, so its outcome goes to the dialog that asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Write {
    Create,
    Rename,
    Delete,
    Detach,
}

/// How a label write ended, until the window takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The change was made.
    Done(Write),
    /// The engine refused, with its message.
    Failed(String),
}

/// The write in flight, and its outcome until the window takes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LabelWrites {
    pending: Option<(OperationId, Write)>,
    outcome: Option<Outcome>,
}

impl LabelWrites {
    pub(crate) fn start(&mut self, id: OperationId, write: Write) {
        self.pending = Some((id, write));
        self.outcome = None;
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Whether the write in flight is `write`, so its dialog shows it is saving.
    pub(crate) fn is_working(&self, write: Write) -> bool {
        self.pending.is_some_and(|(_, pending)| pending == write)
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending.is_some_and(|(pending, _)| pending == id)
    }

    /// Ends the write with the operation's terminal `state`.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, write)) = self.pending.take() else {
            return;
        };
        self.outcome = Some(if state == OperationState::Done {
            Outcome::Done(write)
        } else {
            Outcome::Failed(detail.unwrap_or_else(|| "The change was not saved.".to_owned()))
        });
    }

    pub(crate) fn take(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
}

#[cfg(test)]
mod tests {
    use gm_schema::threads::{AttachKind, CommitState};

    use super::*;

    fn label(id: i64, name: &str) -> Label {
        Label {
            id,
            name: name.to_owned(),
            description: String::new(),
            color: None,
            origin: "human".to_owned(),
            created_at: 0,
            attachments: 0,
        }
    }

    #[test]
    fn a_name_is_trimmed_and_has_one_to_sixty_four_characters() {
        assert_eq!(name_of("  auth  ", &[], None), Ok("auth".to_owned()));
        assert_eq!(name_of("   ", &[], None), Err(NAME_EMPTY));
        let longest = "n".repeat(NAME_CHARS);
        assert_eq!(name_of(&longest, &[], None), Ok(longest.clone()));
        let too_long = "n".repeat(NAME_CHARS + 1);
        assert_eq!(name_of(&too_long, &[], None), Err(NAME_LONG));
        // Characters, not bytes: 64 accented letters are 128 bytes and still fit.
        let accented = "á".repeat(NAME_CHARS);
        assert_eq!(name_of(&accented, &[], None), Ok(accented.clone()));
    }

    #[test]
    fn a_name_has_no_control_characters_inside_it() {
        assert_eq!(name_of("a\tb", &[], None), Err(NAME_CONTROL));
        assert_eq!(name_of("a\nb", &[], None), Err(NAME_CONTROL));
        assert_eq!(name_of("a\u{7}b", &[], None), Err(NAME_CONTROL));
    }

    #[test]
    fn a_name_is_unique_and_case_sensitive() {
        let labels = [label(1, "Flaky"), label(2, "perf")];
        assert_eq!(name_of("flaky", &labels, None), Ok("flaky".to_owned()));
        assert_eq!(name_of("Flaky", &labels, None), Err(NAME_TAKEN));
        assert_eq!(name_of(" perf ", &labels, None), Err(NAME_TAKEN));
    }

    #[test]
    fn a_rename_to_the_label_own_name_is_refused_as_unchanged() {
        let labels = [label(1, "Flaky"), label(2, "perf")];
        assert_eq!(name_of("Flaky", &labels, Some(1)), Err(NAME_SAME));
        assert_eq!(name_of("Flaky", &labels, Some(2)), Err(NAME_TAKEN));
        assert_eq!(name_of("ci", &labels, Some(1)), Ok("ci".to_owned()));
    }

    #[test]
    fn a_description_is_trimmed_and_at_most_five_hundred_characters() {
        assert_eq!(description_of(""), Ok(String::new()));
        assert_eq!(description_of("  about it "), Ok("about it".to_owned()));
        let longest = "d".repeat(DESCRIPTION_CHARS);
        assert_eq!(description_of(&longest), Ok(longest.clone()));
        let too_long = "d".repeat(DESCRIPTION_CHARS + 1);
        assert_eq!(description_of(&too_long), Err(DESCRIPTION_LONG));
    }

    #[test]
    fn a_colour_is_empty_or_hash_six_hex_digits_stored_in_lower_case() {
        assert_eq!(colour_of("   "), Ok(None));
        assert_eq!(colour_of(" #FF8800 "), Ok(Some("#ff8800".to_owned())));
        assert_eq!(colour_of("#ff8800"), Ok(Some("#ff8800".to_owned())));
        assert_eq!(colour_of("ff8800"), Err(COLOUR_INVALID));
        assert_eq!(colour_of("#fff"), Err(COLOUR_INVALID));
        assert_eq!(colour_of("#gg8800"), Err(COLOUR_INVALID));
    }

    #[test]
    fn the_new_label_dialog_sends_the_checked_fields_or_nothing() {
        let labels = [label(1, "Flaky")];
        assert_eq!(
            new_label_command("  bug ", " about it ", " #FF8800 ", &labels),
            Ok(CommandKind::CreateLabel {
                name: "bug".into(),
                description: "about it".into(),
                color: Some("#ff8800".into()),
            })
        );
        assert_eq!(
            new_label_command("bug", "", "", &labels),
            Ok(CommandKind::CreateLabel {
                name: "bug".into(),
                description: String::new(),
                color: None,
            })
        );
        assert_eq!(new_label_command("Flaky", "", "", &labels), Err(NAME_TAKEN));
        assert_eq!(
            new_label_command("bug", "", "red", &labels),
            Err(COLOUR_INVALID)
        );
        let long = "d".repeat(DESCRIPTION_CHARS + 1);
        assert_eq!(
            new_label_command("bug", &long, "", &labels),
            Err(DESCRIPTION_LONG)
        );
    }

    #[test]
    fn the_rename_dialog_sends_the_new_name_of_its_label_only() {
        let labels = [label(1, "Flaky"), label(2, "perf")];
        assert_eq!(
            rename_label_command(1, " ci ", &labels),
            Ok(CommandKind::RenameLabel {
                id: 1,
                name: "ci".into()
            })
        );
        assert_eq!(rename_label_command(1, "Flaky", &labels), Err(NAME_SAME));
        assert_eq!(rename_label_command(1, "perf", &labels), Err(NAME_TAKEN));
    }

    #[test]
    fn each_action_sends_its_own_label_command() {
        assert_eq!(delete_command(4), CommandKind::DeleteLabel { id: 4 });
    }

    #[test]
    fn a_detach_names_the_label_and_the_attachment_as_stored() {
        let attachment = LabelAttachment {
            kind: AttachKind::Commit,
            reference: "0123456789abcdef".into(),
            origin: "agent:claude-code".into(),
            at: 1,
            subject: Some("Fix".into()),
            author: None,
            committed_at: None,
            resolved: None,
            state: Some(CommitState::Current),
            current_path: None,
        };
        assert_eq!(
            detach_command("auth", &attachment),
            CommandKind::DetachLabel {
                label: "auth".into(),
                kind: AttachKind::Commit,
                reference: "0123456789abcdef".into(),
            }
        );
    }

    #[test]
    fn the_delete_confirmation_counts_the_attachments_it_removes() {
        assert_eq!(delete_title("flaky"), "Delete label \"flaky\"?");
        assert_eq!(
            delete_lines(0),
            vec![
                "It has no attachments.".to_owned(),
                "This cannot be undone.".to_owned()
            ]
        );
        assert_eq!(delete_lines(1)[0], "1 attachment is removed with it.");
        assert_eq!(delete_lines(7)[0], "7 attachments are removed with it.");
    }

    #[test]
    fn the_notices_name_what_happened() {
        assert_eq!(done_notice(Write::Create), "Label created");
        assert_eq!(done_notice(Write::Rename), "Label renamed");
        assert_eq!(done_notice(Write::Delete), "Label deleted");
        assert_eq!(done_notice(Write::Detach), "Removed from label");
    }

    #[test]
    fn a_write_is_tracked_only_while_its_operation_runs() {
        let mut writes = LabelWrites::default();
        writes.start(OperationId(7), Write::Rename);
        assert!(writes.is_pending());
        assert!(writes.is_working(Write::Rename));
        assert!(!writes.is_working(Write::Delete));
        assert!(writes.owns(OperationId(7)));
        assert!(!writes.owns(OperationId(8)));
        writes.settle(OperationState::Done, None);
        assert!(!writes.is_pending());
        assert_eq!(writes.take(), Some(Outcome::Done(Write::Rename)));
        assert_eq!(writes.take(), None);
    }

    #[test]
    fn a_refused_write_keeps_the_engines_message() {
        let mut writes = LabelWrites::default();
        writes.start(OperationId(1), Write::Create);
        writes.settle(OperationState::Failed, Some("label exists".into()));
        assert_eq!(writes.take(), Some(Outcome::Failed("label exists".into())));
        writes.start(OperationId(2), Write::Delete);
        writes.settle(OperationState::Failed, None);
        assert_eq!(
            writes.take(),
            Some(Outcome::Failed("The change was not saved.".into()))
        );
    }

    #[test]
    fn a_write_that_has_not_ended_is_still_pending() {
        let mut writes = LabelWrites::default();
        writes.start(OperationId(1), Write::Detach);
        writes.settle(OperationState::Executing, None);
        assert!(writes.is_pending());
        assert_eq!(writes.take(), None);
    }
}

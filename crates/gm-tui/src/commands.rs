//! The command list (`Ctrl+K`, D270): the commands that apply where you are, a key column and a
//! short description, filtered by the words typed. A row of a group in `SUBLISTS` opens a list
//! of its own, stacked on the one before. Pure: the overlay, the keys and the drawing live in
//! `ui.rs` and `draw.rs`.

use gm_schema::{MemberUpdate, UpdateResult};

use crate::nav::action::Spec;

/// The registry groups whose actions are listed in a list of their own, with the description of
/// the row that opens it (D270 (5)).
pub const SUBLISTS: &[(&str, &str)] = &[
    ("Filter", "Filter the threads"),
    ("Diff", "Diffs and long lines"),
];

/// What a row runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The action of this name.
    Action(&'static str),
    /// The list of this registry group.
    List(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The key that runs it, as the keys screen shows it; empty when it has none.
    pub key: String,
    pub description: String,
    pub target: Target,
}

/// One list of the stack: its rows, the text typed in its search field and the row under the
/// cursor, among the rows the text keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandList {
    /// The group a stacked list shows; `None` for the first list.
    pub group: Option<&'static str>,
    pub rows: Vec<Row>,
    pub query: String,
    pub selected: usize,
}

/// Whether an action belongs in the list: the moves, the folds, the text box's own keys and
/// going back only make sense as keys (D270 (3)).
fn listed(spec: &Spec) -> bool {
    !matches!(spec.group, "Move" | "Fold" | "Write")
        && !matches!(
            spec.name,
            "open"
                | "screen.back"
                | "screen.close"
                | "search.next"
                | "search.previous"
                | "commands.open"
        )
}

/// The description a row shows: the registry's, without its final full stop.
fn short(description: &str) -> String {
    description.trim_end_matches('.').to_owned()
}

impl CommandList {
    /// The list of `group`, or the first list for `None`, from the `specs` that `applies` keeps;
    /// `key` names the key of an action, empty for none. A group of `SUBLISTS` is one row in the
    /// first list, shown when one of its actions applies.
    pub fn build(
        specs: &[Spec],
        group: Option<&'static str>,
        applies: impl Fn(&str) -> bool,
        key: impl Fn(&str) -> String,
    ) -> Self {
        let mut rows: Vec<Row> = Vec::new();
        for spec in specs
            .iter()
            .filter(|spec| listed(spec) && applies(spec.name))
        {
            let sub = SUBLISTS.iter().find(|(name, _)| *name == spec.group);
            match (group, sub) {
                (None, Some((name, description))) => {
                    if !rows.iter().any(|row| row.target == Target::List(name)) {
                        rows.push(Row {
                            key: String::new(),
                            description: format!("{description}…"),
                            target: Target::List(name),
                        });
                    }
                }
                (None, None) => rows.push(Row {
                    key: key(spec.name),
                    description: short(spec.description),
                    target: Target::Action(spec.name),
                }),
                (Some(wanted), _) if wanted == spec.group => rows.push(Row {
                    key: key(spec.name),
                    description: short(spec.description),
                    target: Target::Action(spec.name),
                }),
                (Some(_), _) => {}
            }
        }
        Self {
            group,
            rows,
            query: String::new(),
            selected: 0,
        }
    }

    /// The title of the list: the description of the row that opened it.
    pub fn title(&self) -> &'static str {
        self.group
            .and_then(|group| SUBLISTS.iter().find(|(name, _)| *name == group))
            .map_or("Commands", |(_, description)| description)
    }

    /// The indices of the rows the typed text keeps: those whose description holds every word,
    /// ignoring case, in their order (D270 (4)).
    pub fn shown(&self) -> Vec<usize> {
        let words: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                let text = row.description.to_lowercase();
                words.iter().all(|word| text.contains(word))
            })
            .map(|(index, _)| index)
            .collect()
    }

    pub fn type_text(&mut self, text: &str) {
        self.query.push_str(text);
        self.selected = 0;
    }

    pub fn backspace(&mut self) {
        if self.query.pop().is_some() {
            self.selected = 0;
        }
    }

    /// Moves the cursor by `by` rows among those shown, wrapping around as the picker does.
    pub fn move_by(&mut self, by: isize) {
        let count = self.shown().len();
        if count == 0 {
            return;
        }
        let count = isize::try_from(count).unwrap_or(isize::MAX);
        let at = isize::try_from(self.selected).unwrap_or(0);
        self.selected = usize::try_from((at + by).rem_euclid(count)).unwrap_or(0);
    }

    /// The row at position `at` among those shown, or under the cursor for `None`.
    pub fn chosen(&self, at: Option<usize>) -> Option<&Row> {
        let shown = self.shown();
        let index = *shown.get(at.unwrap_or(self.selected))?;
        self.rows.get(index)
    }
}

/// The hint line after Update Workspace (D269 (4)): how many members moved, were up to date,
/// skipped or failed, then each member that did not move and why.
pub fn update_summary(members: &[MemberUpdate]) -> String {
    let count = |result| members.iter().filter(|m| m.result == result).count();
    let mut parts = vec![
        format!("Updated {}", count(UpdateResult::Updated)),
        format!("up to date {}", count(UpdateResult::UpToDate)),
    ];
    let why = |m: &MemberUpdate| {
        let reason = m.reason.as_deref().unwrap_or("");
        match (m.result, reason) {
            // Git's message can hold several lines; the hint line has room for its first.
            (UpdateResult::Failed, _) => format!(
                "{} failed: {}",
                m.member,
                reason.lines().next().unwrap_or_default()
            ),
            (_, "changes") => format!("{} has changes", m.member),
            (_, "diverged") => format!("{} has diverged", m.member),
            (_, "no upstream") => format!("{} has no upstream", m.member),
            (_, "missing") => format!("{} is missing", m.member),
            (_, other) => format!("{} {other}", m.member),
        }
    };
    for (result, label) in [
        (UpdateResult::Skipped, "skipped"),
        (UpdateResult::Failed, "failed"),
    ] {
        let these: Vec<String> = members
            .iter()
            .filter(|m| m.result == result)
            .map(why)
            .collect();
        if !these.is_empty() {
            parts.push(format!("{label} {}: {}", these.len(), these.join(", ")));
        }
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::Mode;
    use crate::nav::action::Behavior;

    const ALL: &[Mode] = &[Mode::Normal, Mode::Visual];

    fn spec(name: &'static str, group: &'static str, description: &'static str) -> Spec {
        Spec {
            name,
            title: name,
            description,
            group,
            modes: ALL,
            behavior: Behavior::OneShot,
        }
    }

    fn specs() -> Vec<Spec> {
        vec![
            spec("cursor.down", "Move", "Move the cursor down a row."),
            spec("stage", "Stage", "Stage what is under the cursor."),
            spec("filter.open", "Filter", "Show the threads not resolved."),
            spec("filter.all", "Filter", "Show every thread."),
            spec("remote.fetch", "Remote", "Fetch every remote."),
            spec("commands.open", "Help", "Find a command by what it does."),
        ]
    }

    fn key(name: &str) -> String {
        if name == "stage" {
            "Space s".to_owned()
        } else {
            String::new()
        }
    }

    #[test]
    fn the_first_list_has_a_row_per_command_and_one_per_group_with_a_list_of_its_own() {
        let list = CommandList::build(&specs(), None, |_| true, key);
        let rows: Vec<(&str, &str)> = list
            .rows
            .iter()
            .map(|row| (row.key.as_str(), row.description.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("Space s", "Stage what is under the cursor"),
                ("", "Filter the threads…"),
                ("", "Fetch every remote"),
            ]
        );
        assert_eq!(list.rows[1].target, Target::List("Filter"));
        assert_eq!(list.title(), "Commands");
    }

    #[test]
    fn a_stacked_list_holds_the_actions_of_its_group_that_apply() {
        let list = CommandList::build(&specs(), Some("Filter"), |name| name != "filter.all", key);
        assert_eq!(list.rows.len(), 1);
        assert_eq!(list.rows[0].target, Target::Action("filter.open"));
        assert_eq!(list.title(), "Filter the threads");
    }

    #[test]
    fn a_group_row_is_left_out_when_none_of_its_actions_applies() {
        let list = CommandList::build(&specs(), None, |name| !name.starts_with("filter."), key);
        assert!(
            list.rows
                .iter()
                .all(|row| row.target != Target::List("Filter"))
        );
    }

    #[test]
    fn typing_keeps_the_rows_holding_every_word_and_the_cursor_wraps() {
        let mut list = CommandList::build(&specs(), None, |_| true, key);
        list.type_text("THE thr");
        assert_eq!(list.shown(), [1]);
        assert_eq!(
            list.chosen(None).map(|row| row.target),
            Some(Target::List("Filter"))
        );
        list.backspace();
        list.backspace();
        list.backspace();
        list.backspace();
        assert_eq!(list.query, "THE");
        list.type_text("");
        list.query.clear();
        list.move_by(-1);
        assert_eq!(list.selected, 2, "up from the first row wraps to the last");
        assert_eq!(
            list.chosen(Some(0)).map(|row| row.target),
            Some(Target::Action("stage"))
        );
    }

    #[test]
    fn the_update_summary_counts_and_names_what_did_not_move() {
        let row = |member: &str, result, reason: Option<&str>| MemberUpdate {
            member: member.to_owned(),
            result,
            from: None,
            to: None,
            reason: reason.map(str::to_owned),
        };
        let members = [
            row("shared", UpdateResult::Updated, None),
            row("api", UpdateResult::Updated, None),
            row("web", UpdateResult::UpToDate, None),
            row("billing", UpdateResult::Skipped, Some("changes")),
        ];
        assert_eq!(
            update_summary(&members),
            "Updated 2 · up to date 1 · skipped 1: billing has changes"
        );
    }

    #[test]
    fn a_failure_shows_the_first_line_of_what_git_said() {
        let failed = MemberUpdate {
            member: "shared".to_owned(),
            result: UpdateResult::Failed,
            from: None,
            to: None,
            reason: Some("fatal: no such remote\nfatal: could not read".to_owned()),
        };
        assert_eq!(
            update_summary(&[failed]),
            "Updated 0 · up to date 0 · failed 1: shared failed: fatal: no such remote"
        );
    }
}

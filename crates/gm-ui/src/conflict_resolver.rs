//! The conflict resolver's view model (D22, D114, D115): the three panes of one
//! conflicted file, the counter, the choices and what Apply sends. No GPUI here:
//! the window renders what this reports. The Result pane holds the parsed
//! document. A free edit re-parses it, so the counts always follow the text in
//! the pane, and a text that does not parse is kept as typed until it does.

use gm_core::conflict::{self, ConflictDoc, Hunk, Resolution, Segment};
use gm_schema::{
    ConflictFileRead, ConflictOperation, ConflictSide, ConflictSite, Event, HunkOrigin,
    RecordedResolution, RestackResolution,
};

/// The note when a choice is made while the Result pane cannot be read.
const UNREADABLE_CHOICE: &str = "Fix the conflict markers in the result before choosing.";

/// The note after the engine staged the file.
const STAGED: &str = "Resolved and staged.";

/// The banner above the panes while Git has a recorded resolution for the file (D117).
const RECORDED_BANNER: &str = "Resolved by recorded resolution";

/// The note after the recorded resolution was forgotten. The file is read again.
const FORGOTTEN: &str = "Recorded resolution forgotten. The conflict is open again.";

/// The restack header's line while no file is open and the list is empty (rerere resolved all).
const NOTHING_LEFT: &str = "Nothing is left to resolve. Continue restack replays the commit.";

/// The restack page's line while the first file is being read.
const OPENING: &str = "Opening the conflicted file.";

/// How many characters of the commit id the restack header shows.
const SHORT: usize = 7;

/// Which side of a conflict a pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
}

/// What Apply does with the state now: send the result, or say what remains.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ApplyDecision {
    /// Every conflict is resolved: the engine writes `text` and stages the file.
    Ready { text: String },
    /// Something remains. `message` says what, and nothing is sent.
    Pending { message: String },
}

/// One conflict as the three panes show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HunkView {
    pub index: usize,
    pub left: String,
    pub right: String,
    /// The common ancestor, only when the base is shown and the file has one.
    pub base: Option<String>,
    pub resolution_label: &'static str,
    /// Who made the choice: "human", "recorded resolution" or "script". Empty while unresolved.
    pub origin_label: &'static str,
    pub resolved: bool,
}

/// The document in the Result pane.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Doc {
    Parsed(ConflictDoc),
    /// The text as typed, and why it does not parse. The file cannot be
    /// resolved from here until the markers are fixed.
    Unreadable {
        text: String,
        message: String,
    },
}

/// The resolver of one conflicted file, from its read until it is staged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolverState {
    path: String,
    operation: ConflictOperation,
    left: ConflictSide,
    right: ConflictSide,
    doc: Doc,
    show_base: bool,
    /// The conflict the panes point at, by its index among the conflicts.
    selected: usize,
    /// A line for the status under the panes: why a choice did nothing, or that
    /// the file was staged.
    applied_note: Option<String>,
    /// Set once the engine has staged the file.
    resolved: bool,
    /// The resolution Git recorded for the file (D117).
    recorded: Recorded,
    /// Who made each hunk's choice, by hunk index. `None` for a hunk with no choice.
    origins: Vec<Option<HunkOrigin>>,
    /// Whether the Result pane still holds the recorded text as Review loaded it.
    from_recorded: bool,
    /// Which worktree the file is in: the open checkout, or the restack's resolution worktree (D154).
    site: ConflictSite,
}

/// Git's recorded resolution of the file, as the resolver holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Recorded {
    /// Nothing is recorded for the file.
    Absent,
    /// Git recorded a resolution and applied it to the working tree file.
    Present(RecordedResolution),
    /// The resolution was forgotten. The file must be read again.
    Forgotten,
}

/// Parses the text of the Result pane. Text that does not parse is kept as typed.
fn parse_doc(text: String) -> Doc {
    match conflict::parse(&text) {
        Ok(doc) => Doc::Parsed(doc),
        Err(error) => Doc::Unreadable {
            message: error.to_string(),
            text,
        },
    }
}

impl ResolverState {
    /// The resolver of the file `read` answered. Text that does not parse is
    /// kept as an `Unreadable` document, never a panic.
    #[must_use]
    pub(crate) fn new(read: &ConflictFileRead) -> Self {
        Self {
            path: read.path.clone(),
            operation: read.operation,
            left: read.left.clone(),
            right: read.right.clone(),
            doc: parse_doc(read.text.clone()),
            show_base: false,
            selected: 0,
            applied_note: None,
            resolved: false,
            recorded: read
                .recorded
                .clone()
                .map_or(Recorded::Absent, Recorded::Present),
            origins: Vec::new(),
            from_recorded: false,
            site: ConflictSite::WorkingCopy,
        }
    }

    /// The same resolver, for the worktree `site` names: a restack's conflict is read,
    /// resolved and forgotten in the resolution worktree (D154).
    #[must_use]
    pub(crate) fn with_site(mut self, site: ConflictSite) -> Self {
        self.site = site;
        self
    }

    /// The worktree the file is in: every conflict command it sends names it.
    #[must_use]
    pub(crate) fn site(&self) -> ConflictSite {
        self.site
    }

    /// The resolver of a file read again after its recorded resolution was
    /// forgotten: the same as `new`, with the note that says the conflict is open.
    #[must_use]
    pub(crate) fn reread_after_forget(read: &ConflictFileRead) -> Self {
        let mut state = Self::new(read);
        state.applied_note = Some(FORGOTTEN.to_owned());
        state
    }

    /// The banner above the panes, while Git has a recorded resolution for the file.
    #[must_use]
    pub(crate) fn recorded_banner(&self) -> Option<&'static str> {
        self.has_recorded().then_some(RECORDED_BANNER)
    }

    /// The path of `ForgetRecordedResolution`, when the file has a recorded resolution.
    #[must_use]
    pub(crate) fn forget_request(&self) -> Option<String> {
        self.has_recorded().then(|| self.path.clone())
    }

    /// Whether the file was forgotten and must be read again (the view sends `ReadConflict`).
    #[must_use]
    pub(crate) fn needs_reread(&self) -> bool {
        matches!(self.recorded, Recorded::Forgotten)
    }

    fn has_recorded(&self) -> bool {
        matches!(self.recorded, Recorded::Present(_))
    }

    #[must_use]
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    /// The operation's name as the header shows it.
    #[must_use]
    pub(crate) fn operation_label(&self) -> &'static str {
        match self.operation {
            ConflictOperation::Merge => "Merge",
            ConflictOperation::Rebase => "Rebase",
            ConflictOperation::CherryPick => "Cherry-pick",
            ConflictOperation::Revert => "Revert",
            ConflictOperation::Unknown => "Conflict",
        }
    }

    /// The index of the conflict the panes point at.
    #[must_use]
    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    #[must_use]
    pub(crate) fn show_base(&self) -> bool {
        self.show_base
    }

    #[must_use]
    pub(crate) fn applied_note(&self) -> Option<&str> {
        self.applied_note.as_deref()
    }

    /// Whether the engine has staged the file.
    #[must_use]
    pub(crate) fn is_staged(&self) -> bool {
        self.resolved
    }

    /// Whether the Result pane parses, so its conflicts can be listed.
    #[must_use]
    pub(crate) fn is_readable(&self) -> bool {
        matches!(self.doc, Doc::Parsed(_))
    }

    /// How many conflicts the Result pane holds, resolved or not. A text that
    /// does not parse holds none that can be counted.
    #[must_use]
    pub(crate) fn conflict_count(&self) -> usize {
        self.hunks().len()
    }

    /// How many of those conflicts have a choice.
    #[must_use]
    pub(crate) fn resolved_count(&self) -> usize {
        self.hunks()
            .iter()
            .filter(|hunk| hunk.resolution.is_some())
            .count()
    }

    /// "N of M conflicts resolved", with "conflict" in the singular for one.
    ///
    /// Known gap: the non-conflicting changes Git applied on its own (the "3
    /// changes applied" note) are not counted here. The markers do not say how
    /// many there were, so the note needs the engine to report the number of
    /// cleanly merged hunks with the read.
    #[must_use]
    pub(crate) fn counter_label(&self) -> String {
        if let Doc::Unreadable { .. } = self.doc {
            return "The result has conflict markers that cannot be read".to_owned();
        }
        let resolved = self.resolved_count();
        let total = self.conflict_count();
        if total == 1 {
            format!("{resolved} of 1 conflict resolved")
        } else {
            format!("{resolved} of {total} conflicts resolved")
        }
    }

    /// Chooses `resolution` for the conflict at `index`. Does nothing for an
    /// index with no conflict, and says so when the Result pane cannot be read.
    pub(crate) fn choose(&mut self, index: usize, resolution: Resolution) {
        self.set_resolution(index, Some(resolution));
    }

    /// Takes the choice back from the conflict at `index`, so it shows its
    /// markers again.
    pub(crate) fn clear(&mut self, index: usize) {
        self.set_resolution(index, None);
    }

    fn set_resolution(&mut self, index: usize, resolution: Option<Resolution>) {
        let origin = resolution.as_ref().map(|_| HunkOrigin::Human);
        let count = self.conflict_count();
        let Doc::Parsed(doc) = &mut self.doc else {
            self.applied_note = Some(UNREADABLE_CHOICE.to_owned());
            return;
        };
        doc.set_resolution(index, resolution);
        self.applied_note = None;
        if index < count {
            self.set_origin(index, origin);
        }
    }

    /// Loads the recorded text into the Result pane (Review). Its hunks are marked
    /// Rerere; the user can still edit the text. Does nothing without a recorded
    /// resolution.
    pub(crate) fn use_recorded(&mut self) {
        let Recorded::Present(recorded) = &self.recorded else {
            return;
        };
        let text = recorded.text.clone();
        self.doc = parse_doc(text);
        self.origins = vec![Some(HunkOrigin::Rerere); self.conflict_count()];
        self.from_recorded = true;
        self.clamp_selected();
    }

    fn set_origin(&mut self, index: usize, origin: Option<HunkOrigin>) {
        self.origins.resize(self.origins.len().max(index + 1), None);
        self.origins[index] = origin;
    }

    fn origin_at(&self, index: usize) -> Option<HunkOrigin> {
        self.origins.get(index).copied().flatten()
    }

    /// Keeps the selected conflict among the conflicts the Result pane holds.
    fn clamp_selected(&mut self) {
        self.selected = self.selected.min(self.conflict_count().saturating_sub(1));
    }

    /// Points the panes at the conflict `index`, when there is one.
    pub(crate) fn select(&mut self, index: usize) {
        if index < self.conflict_count() {
            self.selected = index;
        }
    }

    /// Points at the next unresolved conflict after the selected one, wrapping
    /// around. When every conflict is resolved it moves to the next one.
    pub(crate) fn select_next(&mut self) {
        let hunks = self.hunks();
        let total = hunks.len();
        if total == 0 {
            self.selected = 0;
            return;
        }
        let start = self.selected % total;
        let next = (1..=total)
            .map(|step| (start + step) % total)
            .find(|&index| hunks[index].resolution.is_none())
            .unwrap_or((start + 1) % total);
        self.selected = next;
    }

    /// Points at the previous unresolved conflict before the selected one,
    /// wrapping around. When every conflict is resolved it moves to the previous one.
    pub(crate) fn select_prev(&mut self) {
        let hunks = self.hunks();
        let total = hunks.len();
        if total == 0 {
            self.selected = 0;
            return;
        }
        let start = self.selected % total;
        let previous = (1..=total)
            .map(|step| (start + total - step) % total)
            .find(|&index| hunks[index].resolution.is_none())
            .unwrap_or((start + total - 1) % total);
        self.selected = previous;
    }

    /// Shows the common ancestor of each conflict, or hides it again.
    pub(crate) fn toggle_base(&mut self) {
        self.show_base = !self.show_base;
    }

    /// The pane title of `side`: "<branch> · <role>", for example
    /// "feature/x · merged branch". The roles name the sides; ours and theirs
    /// never appear.
    #[must_use]
    pub(crate) fn side_title(&self, side: Side) -> String {
        let side = self.side(side);
        format!("{} · {}", side.branch, side.role)
    }

    /// The line under the pane title: the subject of the side's commit.
    #[must_use]
    pub(crate) fn side_subtitle(&self, side: Side) -> String {
        self.side(side).subject.clone()
    }

    /// The text of the Result pane: resolved conflicts as their chosen text, the
    /// others as the markers they came from.
    #[must_use]
    pub(crate) fn result_text(&self) -> String {
        match &self.doc {
            Doc::Parsed(doc) => doc.render(),
            Doc::Unreadable { text, .. } => text.clone(),
        }
    }

    /// Takes a free edit of the Result pane. The text is parsed again, so the
    /// counts follow it. A text that does not parse is kept as typed, and Apply
    /// says why until the markers are fixed.
    pub(crate) fn set_result_text(&mut self, text: String) {
        if text != self.result_text() {
            // Text that came from the recorded resolution is no longer as Git left it.
            self.origins.clear();
            self.from_recorded = false;
        }
        self.doc = parse_doc(text);
        self.clamp_selected();
    }

    /// Who resolved each conflict, in file order: one entry per conflict. A file
    /// with no conflict markers left has none, unless its whole text is the
    /// recorded resolution and unchanged, which is one `Rerere` entry.
    #[must_use]
    pub(crate) fn origins(&self) -> Vec<HunkOrigin> {
        let count = self.conflict_count();
        if count == 0 {
            return if self.from_recorded {
                vec![HunkOrigin::Rerere]
            } else {
                Vec::new()
            };
        }
        (0..count)
            .map(|index| self.origin_at(index).unwrap_or(HunkOrigin::Human))
            .collect()
    }

    /// What Apply does now: `Ready` with the text when every conflict is
    /// resolved, otherwise `Pending` with what remains. It is never disabled.
    #[must_use]
    pub(crate) fn apply_request(&self) -> ApplyDecision {
        match &self.doc {
            Doc::Unreadable { message, .. } => ApplyDecision::Pending {
                message: format!(
                    "The result has conflict markers that cannot be read ({message}). Fix them, or resolve the conflicts in the panes."
                ),
            },
            Doc::Parsed(doc) => {
                let remaining = self.conflict_count() - self.resolved_count();
                match remaining {
                    0 => ApplyDecision::Ready { text: doc.render() },
                    1 => ApplyDecision::Pending {
                        message: "1 conflict is still unresolved. Resolve it or edit the result."
                            .to_owned(),
                    },
                    many => ApplyDecision::Pending {
                        message: format!(
                            "{many} conflicts are still unresolved. Resolve them or edit the result."
                        ),
                    },
                }
            }
        }
    }

    /// Apply: the path, text and origins of the `ResolveConflict` command when every
    /// conflict is resolved. Otherwise nothing is sent, and the note says what remains.
    pub(crate) fn apply(&mut self) -> Option<(String, String, Vec<HunkOrigin>)> {
        match self.apply_request() {
            ApplyDecision::Ready { text } => Some((self.path.clone(), text, self.origins())),
            ApplyDecision::Pending { message } => {
                self.applied_note = Some(message);
                None
            }
        }
    }

    /// One view per conflict, in file order, for the three panes.
    #[must_use]
    pub(crate) fn hunk_views(&self) -> Vec<HunkView> {
        self.hunks()
            .into_iter()
            .enumerate()
            .map(|(index, hunk)| HunkView {
                index,
                left: hunk.left.clone(),
                right: hunk.right.clone(),
                base: if self.show_base {
                    hunk.base.clone()
                } else {
                    None
                },
                resolution_label: resolution_label(hunk.resolution.as_ref()),
                origin_label: origin_label(hunk.resolution.is_some(), self.origin_at(index)),
                resolved: hunk.resolution.is_some(),
            })
            .collect()
    }

    /// Applies an engine event. Returns true when it was about this file.
    pub(crate) fn on_event(&mut self, event: &Event) -> bool {
        match event {
            Event::ConflictResolved { path, .. } if *path == self.path => {
                self.resolved = true;
                self.applied_note = Some(STAGED.to_owned());
                true
            }
            Event::RecordedResolutionForgotten { path, .. } if *path == self.path => {
                self.recorded = Recorded::Forgotten;
                self.applied_note = Some(FORGOTTEN.to_owned());
                true
            }
            _ => false,
        }
    }

    fn side(&self, side: Side) -> &ConflictSide {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    /// The conflicts of the Result pane, in file order. Empty when it cannot be read.
    fn hunks(&self) -> Vec<&Hunk> {
        let Doc::Parsed(doc) = &self.doc else {
            return Vec::new();
        };
        doc.segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Conflict(hunk) => Some(hunk),
                Segment::Text(_) => None,
            })
            .collect()
    }
}

/// The restack conflict the resolver works on (D154): the engine's `RestackResolution`
/// and its conflicted files, each marked resolved once its Apply succeeded. It also holds
/// the line the restack header shows when Continue or a command did not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestackConflicts {
    resolution: RestackResolution,
    files: Vec<RestackFile>,
    note: Option<String>,
}

/// One conflicted file of a restack, in the order the engine lists them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestackFile {
    pub(crate) path: String,
    pub(crate) resolved: bool,
}

impl RestackConflicts {
    /// The restack conflict `resolution` describes, with every file still to resolve.
    #[must_use]
    pub(crate) fn new(resolution: RestackResolution) -> Self {
        let files = resolution
            .paths
            .iter()
            .map(|path| RestackFile {
                path: path.clone(),
                resolved: false,
            })
            .collect();
        Self {
            resolution,
            files,
            note: None,
        }
    }

    #[must_use]
    pub(crate) fn resolution(&self) -> &RestackResolution {
        &self.resolution
    }

    #[must_use]
    pub(crate) fn files(&self) -> &[RestackFile] {
        &self.files
    }

    /// The line under the header: why Continue did not run, or why an engine command was refused.
    #[must_use]
    pub(crate) fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub(crate) fn set_note(&mut self, note: Option<String>) {
        self.note = note;
    }

    /// The header's second line: the commit being replayed, with its subject and short id, and
    /// the new parent it is replayed onto.
    #[must_use]
    pub(crate) fn header(&self) -> String {
        let short: String = self.resolution.commit.chars().take(SHORT).collect();
        format!(
            "Commit being replayed: {} ({short}). New parent: {}.",
            self.resolution.subject, self.resolution.parent
        )
    }

    /// Marks the file `path` resolved, once its Apply succeeded. Other paths are not in the list.
    pub(crate) fn mark_resolved(&mut self, path: &str) {
        if let Some(file) = self.files.iter_mut().find(|file| file.path == path) {
            file.resolved = true;
        }
    }

    /// How many files are still to resolve.
    #[must_use]
    pub(crate) fn unresolved(&self) -> usize {
        self.files.iter().filter(|file| !file.resolved).count()
    }

    /// The first file still to resolve: the resolver opens on it at once.
    #[must_use]
    pub(crate) fn first_unresolved(&self) -> Option<&str> {
        self.files
            .iter()
            .find(|file| !file.resolved)
            .map(|file| file.path.as_str())
    }

    /// The next file still to resolve after `path`, wrapping round the list. Never `path` itself.
    #[must_use]
    pub(crate) fn next_unresolved_after(&self, path: &str) -> Option<&str> {
        let total = self.files.len();
        let start = self
            .files
            .iter()
            .position(|file| file.path == path)
            .map_or(0, |at| at + 1);
        (0..total)
            .map(|step| &self.files[(start + step) % total])
            .find(|file| !file.resolved && file.path != path)
            .map(|file| file.path.as_str())
    }

    /// What Continue says while files remain, or `None` when it may run. Continue is never
    /// disabled: it answers with this message instead (D116(4)).
    #[must_use]
    pub(crate) fn continue_refusal(&self) -> Option<String> {
        match self.unresolved() {
            0 => None,
            1 => Some("1 file is still unresolved. Resolve it and Apply, or Cancel.".to_owned()),
            many => Some(format!(
                "{many} files are still unresolved. Resolve them and Apply, or Cancel."
            )),
        }
    }

    /// The resolver's body while no file is open: the file being read, or, when rerere left
    /// nothing to resolve, that Continue replays the commit.
    #[must_use]
    pub(crate) fn empty_message(&self) -> &'static str {
        if self.files.is_empty() {
            NOTHING_LEFT
        } else {
            OPENING
        }
    }
}

/// Who made a hunk's choice, as its card names it. Empty while the hunk is
/// unresolved. The labels name the people and Git, never the sides.
fn origin_label(resolved: bool, origin: Option<HunkOrigin>) -> &'static str {
    if !resolved {
        return "";
    }
    match origin.unwrap_or(HunkOrigin::Human) {
        HunkOrigin::Human => "human",
        HunkOrigin::Rerere => "recorded resolution",
        HunkOrigin::Script => "script",
    }
}

/// The label of a hunk's choice, as the panes and the counter name it.
fn resolution_label(resolution: Option<&Resolution>) -> &'static str {
    match resolution {
        None => "Unresolved",
        Some(Resolution::Left) => "Left",
        Some(Resolution::Right) => "Right",
        Some(Resolution::Both) => "Both",
        Some(Resolution::Custom(_)) => "Custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::OperationId;

    fn side(branch: &str, role: &str, subject: &str) -> ConflictSide {
        ConflictSide {
            branch: branch.into(),
            role: role.into(),
            subject: subject.into(),
        }
    }

    fn read_with(text: &str, has_base: bool) -> ConflictFileRead {
        ConflictFileRead {
            path: "src/lib.rs".into(),
            operation: ConflictOperation::Merge,
            left: side("main", "current branch", "Add the parser"),
            right: side("feature/x", "merged branch", "Rewrite the parser"),
            text: text.into(),
            has_base,
            recorded: None,
        }
    }

    fn read(text: &str) -> ConflictFileRead {
        read_with(text, false)
    }

    const TWO: &str = "top\n<<<<<<< main\nleft one\n=======\nright one\n>>>>>>> feature/x\nmiddle\n<<<<<<< main\nleft two\n=======\nright two\n>>>>>>> feature/x\nend\n";

    const THREE: &str = "<<<<<<< main\na-left\n=======\na-right\n>>>>>>> feature/x\n<<<<<<< main\nb-left\n=======\nb-right\n>>>>>>> feature/x\n<<<<<<< main\nc-left\n=======\nc-right\n>>>>>>> feature/x\n";

    const WITH_BASE: &str = "<<<<<<< main\nleft\n||||||| merged common ancestors\nbase text\n=======\nright\n>>>>>>> feature/x\n";

    #[test]
    fn a_result_that_does_not_parse_is_not_readable() {
        let mut state = ResolverState::new(&read(TWO));
        assert!(state.is_readable());
        state.set_result_text("<<<<<<< x\n".to_owned());
        assert!(!state.is_readable());
    }

    #[test]
    fn the_counter_counts_resolved_conflicts_of_the_result() {
        let mut state = ResolverState::new(&read(TWO));
        assert_eq!(state.path(), "src/lib.rs");
        assert_eq!(state.operation_label(), "Merge");
        assert_eq!(state.counter_label(), "0 of 2 conflicts resolved");
        state.choose(0, Resolution::Left);
        assert_eq!(state.counter_label(), "1 of 2 conflicts resolved");
        state.choose(1, Resolution::Right);
        assert_eq!(state.counter_label(), "2 of 2 conflicts resolved");
        let single = ResolverState::new(&read(WITH_BASE));
        assert_eq!(single.counter_label(), "0 of 1 conflict resolved");
    }

    #[test]
    fn choose_and_clear_set_and_take_back_a_resolution() {
        let mut state = ResolverState::new(&read(TWO));
        state.choose(0, Resolution::Left);
        let views = state.hunk_views();
        assert!(views[0].resolved);
        assert_eq!(views[0].resolution_label, "Left");
        assert!(!views[1].resolved);
        assert_eq!(views[1].resolution_label, "Unresolved");
        state.choose(0, Resolution::Right);
        assert_eq!(state.hunk_views()[0].resolution_label, "Right");
        state.clear(0);
        assert!(!state.hunk_views()[0].resolved);
        assert_eq!(state.result_text(), TWO);
    }

    #[test]
    fn the_result_takes_the_chosen_side_and_both_puts_the_left_first() {
        let mut state = ResolverState::new(&read(TWO));
        state.choose(0, Resolution::Left);
        state.choose(1, Resolution::Both);
        assert_eq!(
            state.result_text(),
            "top\nleft one\nmiddle\nleft two\nright two\nend\n"
        );
        assert_eq!(state.hunk_views()[1].resolution_label, "Both");
    }

    #[test]
    fn choosing_for_a_conflict_that_is_not_there_does_nothing() {
        let mut state = ResolverState::new(&read(TWO));
        state.choose(5, Resolution::Left);
        assert_eq!(state.resolved_count(), 0);
        assert_eq!(state.result_text(), TWO);
    }

    #[test]
    fn select_next_wraps_and_skips_resolved_conflicts() {
        let mut state = ResolverState::new(&read(THREE));
        state.choose(1, Resolution::Left);
        assert_eq!(state.selected(), 0);
        state.select_next();
        assert_eq!(state.selected(), 2);
        state.select_next();
        assert_eq!(state.selected(), 0);
        state.select_next();
        assert_eq!(state.selected(), 2);
    }

    #[test]
    fn select_prev_wraps_and_skips_resolved_conflicts() {
        let mut state = ResolverState::new(&read(THREE));
        state.choose(1, Resolution::Right);
        state.select_prev();
        assert_eq!(state.selected(), 2);
        state.select_prev();
        assert_eq!(state.selected(), 0);
        state.select_prev();
        assert_eq!(state.selected(), 2);
    }

    #[test]
    fn selection_moves_without_conflicts_stays_at_zero() {
        let mut state = ResolverState::new(&read("plain\n"));
        state.select_next();
        state.select_prev();
        assert_eq!(state.selected(), 0);
        assert_eq!(state.conflict_count(), 0);
    }

    #[test]
    fn the_titles_name_the_role_and_never_ours_or_theirs() {
        let state = ResolverState::new(&read(TWO));
        assert_eq!(state.side_title(Side::Left), "main · current branch");
        assert_eq!(state.side_title(Side::Right), "feature/x · merged branch");
        assert_eq!(state.side_subtitle(Side::Left), "Add the parser");
        assert_eq!(state.side_subtitle(Side::Right), "Rewrite the parser");
        for text in [
            state.side_title(Side::Left),
            state.side_title(Side::Right),
            state.side_subtitle(Side::Left),
            state.side_subtitle(Side::Right),
        ] {
            let lower = text.to_lowercase();
            assert!(!lower.contains("ours"), "{text}");
            assert!(!lower.contains("theirs"), "{text}");
        }
    }

    #[test]
    fn the_base_is_shown_only_when_toggled() {
        let mut state = ResolverState::new(&read_with(WITH_BASE, true));
        assert_eq!(state.hunk_views()[0].base, None);
        state.toggle_base();
        assert!(state.show_base());
        assert_eq!(state.hunk_views()[0].base.as_deref(), Some("base text\n"));
        state.toggle_base();
        assert_eq!(state.hunk_views()[0].base, None);
    }

    #[test]
    fn a_free_edit_re_parses_so_the_counts_follow_the_result() {
        let mut state = ResolverState::new(&read(TWO));
        state.choose(0, Resolution::Left);
        let typed = "a\n<<<<<<< x\nl\n=======\nr\n>>>>>>> y\nb\n";
        state.set_result_text(typed.to_owned());
        assert_eq!(state.conflict_count(), 1);
        assert_eq!(state.resolved_count(), 0);
        assert_eq!(state.result_text(), typed);
        state.set_result_text("plain\n".to_owned());
        assert_eq!(state.conflict_count(), 0);
        assert_eq!(
            state.apply_request(),
            ApplyDecision::Ready {
                text: "plain\n".to_owned()
            }
        );
    }

    #[test]
    fn a_free_edit_that_does_not_parse_is_kept_and_blocks_apply_until_fixed() {
        let mut state = ResolverState::new(&read(TWO));
        let broken = "<<<<<<< x\nl\n";
        state.set_result_text(broken.to_owned());
        assert_eq!(state.result_text(), broken);
        assert!(state.counter_label().contains("cannot be read"));
        assert!(matches!(
            state.apply_request(),
            ApplyDecision::Pending { .. }
        ));
        assert_eq!(state.apply(), None);
        state.choose(0, Resolution::Left);
        assert_eq!(state.result_text(), broken);
        assert!(state.applied_note().is_some());
        state.set_result_text(TWO.to_owned());
        assert_eq!(state.conflict_count(), 2);
    }

    #[test]
    fn apply_names_what_remains_with_singular_and_plural() {
        let mut state = ResolverState::new(&read(TWO));
        assert_eq!(
            state.apply_request(),
            ApplyDecision::Pending {
                message: "2 conflicts are still unresolved. Resolve them or edit the result."
                    .to_owned()
            }
        );
        state.choose(0, Resolution::Left);
        assert_eq!(
            state.apply_request(),
            ApplyDecision::Pending {
                message: "1 conflict is still unresolved. Resolve it or edit the result."
                    .to_owned()
            }
        );
        assert_eq!(state.apply(), None);
        assert_eq!(
            state.applied_note(),
            Some("1 conflict is still unresolved. Resolve it or edit the result.")
        );
        state.choose(1, Resolution::Right);
        let expected = "top\nleft one\nmiddle\nright two\nend\n";
        assert_eq!(
            state.apply_request(),
            ApplyDecision::Ready {
                text: expected.to_owned()
            }
        );
        assert_eq!(
            state.apply(),
            Some((
                "src/lib.rs".to_owned(),
                expected.to_owned(),
                vec![HunkOrigin::Human, HunkOrigin::Human]
            ))
        );
    }

    #[test]
    fn a_file_with_no_conflict_is_ready_at_once() {
        let state = ResolverState::new(&read("plain\n"));
        assert_eq!(
            state.apply_request(),
            ApplyDecision::Ready {
                text: "plain\n".to_owned()
            }
        );
    }

    #[test]
    fn an_unreadable_file_keeps_its_text_and_never_panics() {
        let text = "<<<<<<< main\nleft\n";
        let mut state = ResolverState::new(&read(text));
        assert_eq!(state.result_text(), text);
        assert_eq!(state.conflict_count(), 0);
        assert!(state.hunk_views().is_empty());
        assert!(state.counter_label().contains("cannot be read"));
        let ApplyDecision::Pending { message } = state.apply_request() else {
            panic!("an unreadable file is never ready");
        };
        assert!(message.contains("line 1"), "{message}");
        assert_eq!(state.apply(), None);
        assert_eq!(state.applied_note(), Some(message.as_str()));
    }

    #[test]
    fn select_points_at_a_conflict_that_exists_and_ignores_the_rest() {
        let mut state = ResolverState::new(&read(THREE));
        state.select(2);
        assert_eq!(state.selected(), 2);
        state.select(9);
        assert_eq!(state.selected(), 2);
    }

    #[test]
    fn the_operation_has_a_name_for_the_header() {
        let mut state = ResolverState::new(&read(TWO));
        assert_eq!(state.operation_label(), "Merge");
        state.operation = ConflictOperation::CherryPick;
        assert_eq!(state.operation_label(), "Cherry-pick");
        state.operation = ConflictOperation::Unknown;
        assert_eq!(state.operation_label(), "Conflict");
    }

    #[test]
    fn the_resolved_event_for_this_file_marks_it_staged() {
        let mut state = ResolverState::new(&read(TWO));
        let other = Event::ConflictResolved {
            id: OperationId(4),
            path: "other.rs".to_owned(),
        };
        assert!(!state.on_event(&other));
        assert!(!state.is_staged());
        let ours = Event::ConflictResolved {
            id: OperationId(5),
            path: "src/lib.rs".to_owned(),
        };
        assert!(state.on_event(&ours));
        assert!(state.is_staged());
        assert_eq!(state.applied_note(), Some(STAGED));
    }

    /// A read whose file has a recorded resolution with `recorded` as its text.
    fn read_recorded(text: &str, recorded: &str) -> ConflictFileRead {
        let mut file = read(text);
        file.recorded = Some(RecordedResolution {
            text: recorded.into(),
        });
        file
    }

    fn forgotten(path: &str) -> Event {
        Event::RecordedResolutionForgotten {
            id: OperationId(6),
            path: path.to_owned(),
        }
    }

    #[test]
    fn a_recorded_resolution_shows_the_banner_until_it_is_forgotten() {
        let mut state = ResolverState::new(&read_recorded(TWO, "plain\n"));
        assert_eq!(
            state.recorded_banner(),
            Some("Resolved by recorded resolution")
        );
        assert_eq!(state.forget_request().as_deref(), Some("src/lib.rs"));
        assert!(!state.needs_reread());
        assert!(!state.on_event(&forgotten("other.rs")));
        assert!(state.recorded_banner().is_some());
        assert!(!state.needs_reread());
        assert!(state.on_event(&forgotten("src/lib.rs")));
        assert_eq!(state.recorded_banner(), None);
        assert_eq!(state.forget_request(), None);
        assert!(state.needs_reread());
        assert_eq!(
            state.applied_note(),
            Some("Recorded resolution forgotten. The conflict is open again.")
        );
    }

    #[test]
    fn a_file_without_a_recorded_resolution_has_no_banner_and_nothing_to_review() {
        let mut state = ResolverState::new(&read(TWO));
        assert_eq!(state.recorded_banner(), None);
        assert_eq!(state.forget_request(), None);
        state.use_recorded();
        assert_eq!(state.result_text(), TWO);
        assert!(
            state
                .origins()
                .iter()
                .all(|origin| *origin == HunkOrigin::Human)
        );
    }

    #[test]
    fn review_loads_the_recorded_text_and_the_user_can_still_edit_it() {
        let mut state = ResolverState::new(&read_recorded(TWO, "plain\n"));
        state.use_recorded();
        assert_eq!(state.result_text(), "plain\n");
        assert_eq!(state.conflict_count(), 0);
        state.set_result_text("plain, edited\n".to_owned());
        assert_eq!(state.result_text(), "plain, edited\n");
        assert_eq!(
            state.apply(),
            Some((
                "src/lib.rs".to_owned(),
                "plain, edited\n".to_owned(),
                Vec::new()
            ))
        );
    }

    #[test]
    fn a_recorded_text_without_markers_is_one_rerere_origin_until_it_changes() {
        let mut state = ResolverState::new(&read_recorded(TWO, "plain\n"));
        state.use_recorded();
        assert_eq!(state.origins(), vec![HunkOrigin::Rerere]);
        assert_eq!(
            state.apply(),
            Some((
                "src/lib.rs".to_owned(),
                "plain\n".to_owned(),
                vec![HunkOrigin::Rerere]
            ))
        );
        state.set_result_text("plain, edited\n".to_owned());
        assert_eq!(state.origins(), Vec::<HunkOrigin>::new());
    }

    #[test]
    fn a_file_with_no_conflict_and_no_recorded_text_has_no_origins() {
        let state = ResolverState::new(&read("plain\n"));
        assert_eq!(state.origins(), Vec::<HunkOrigin>::new());
    }

    #[test]
    fn origins_are_one_per_conflict_in_file_order() {
        let mut state = ResolverState::new(&read(THREE));
        assert_eq!(state.origins().len(), 3);
        state.choose(0, Resolution::Left);
        state.choose(1, Resolution::Right);
        state.choose(2, Resolution::Both);
        assert_eq!(state.origins(), vec![HunkOrigin::Human; 3]);
    }

    #[test]
    fn a_choice_after_review_is_human_and_the_others_keep_their_origin() {
        let mut state = ResolverState::new(&read_recorded(TWO, TWO));
        state.use_recorded();
        assert_eq!(
            state.origins(),
            vec![HunkOrigin::Rerere, HunkOrigin::Rerere]
        );
        assert_eq!(
            state
                .hunk_views()
                .iter()
                .map(|v| v.origin_label)
                .collect::<Vec<_>>(),
            vec!["", ""],
            "an unresolved hunk has no origin label"
        );
        state.choose(0, Resolution::Left);
        let views = state.hunk_views();
        assert_eq!(views[0].origin_label, "human");
        assert_eq!(views[1].origin_label, "");
        assert_eq!(state.origins(), vec![HunkOrigin::Human, HunkOrigin::Rerere]);
    }

    #[test]
    fn a_cleared_choice_has_no_origin_label_and_a_new_choice_is_human() {
        let mut state = ResolverState::new(&read(TWO));
        state.choose(0, Resolution::Left);
        assert_eq!(state.hunk_views()[0].origin_label, "human");
        state.clear(0);
        assert_eq!(state.hunk_views()[0].origin_label, "");
        state.choose(0, Resolution::Right);
        assert_eq!(state.hunk_views()[0].origin_label, "human");
    }

    #[test]
    fn a_free_edit_that_changes_the_text_drops_the_rerere_origins() {
        let mut state = ResolverState::new(&read_recorded(TWO, TWO));
        state.use_recorded();
        state.set_result_text(format!("{TWO}extra\n"));
        assert_eq!(state.origins(), vec![HunkOrigin::Human, HunkOrigin::Human]);
        state.choose(0, Resolution::Left);
        assert_eq!(state.origins(), vec![HunkOrigin::Human, HunkOrigin::Human]);
    }

    #[test]
    fn an_origin_label_is_empty_while_unresolved_and_never_names_a_side() {
        assert_eq!(origin_label(false, Some(HunkOrigin::Rerere)), "");
        assert_eq!(origin_label(true, None), "human");
        assert_eq!(origin_label(true, Some(HunkOrigin::Human)), "human");
        assert_eq!(
            origin_label(true, Some(HunkOrigin::Rerere)),
            "recorded resolution"
        );
        assert_eq!(origin_label(true, Some(HunkOrigin::Script)), "script");
        let mut state = ResolverState::new(&read_recorded(TWO, TWO));
        state.use_recorded();
        state.choose(0, Resolution::Left);
        let mut texts = vec![
            state.recorded_banner().unwrap_or_default().to_owned(),
            FORGOTTEN.to_owned(),
        ];
        texts.extend(
            state
                .hunk_views()
                .into_iter()
                .map(|v| v.origin_label.to_owned()),
        );
        for text in texts {
            let lower = text.to_lowercase();
            assert!(!lower.contains("ours"), "{text}");
            assert!(!lower.contains("theirs"), "{text}");
        }
    }

    #[test]
    fn a_forgotten_resolution_asks_for_a_read_and_the_new_read_keeps_the_note() {
        let mut state = ResolverState::new(&read_recorded(TWO, "plain\n"));
        assert!(!state.needs_reread());
        assert!(state.on_event(&forgotten("src/lib.rs")));
        assert!(state.needs_reread());
        let reread = ResolverState::reread_after_forget(&read(TWO));
        assert!(!reread.needs_reread());
        assert_eq!(reread.recorded_banner(), None);
        assert_eq!(
            reread.applied_note(),
            Some("Recorded resolution forgotten. The conflict is open again.")
        );
        assert_eq!(reread.conflict_count(), 2);
    }

    fn restack(paths: &[&str]) -> RestackResolution {
        RestackResolution {
            from: "main".into(),
            conflicted: "feature/x".into(),
            parent: "main".into(),
            commit: "0123456789abcdef".into(),
            subject: "Rewrite the parser".into(),
            onto: "base".into(),
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        }
    }

    #[test]
    fn the_restack_header_names_the_commit_its_short_id_and_the_new_parent() {
        let conflicts = RestackConflicts::new(restack(&["a.txt"]));
        assert_eq!(conflicts.resolution().conflicted, "feature/x");
        assert_eq!(
            conflicts.header(),
            "Commit being replayed: Rewrite the parser (0123456). New parent: main."
        );
    }

    #[test]
    fn marking_a_file_moves_the_first_and_the_next_unresolved_and_never_stays_on_itself() {
        let mut conflicts = RestackConflicts::new(restack(&["a.txt", "b.txt", "c.txt"]));
        assert_eq!(conflicts.first_unresolved(), Some("a.txt"));
        assert_eq!(conflicts.next_unresolved_after("a.txt"), Some("b.txt"));
        assert_eq!(
            conflicts.next_unresolved_after("c.txt"),
            Some("a.txt"),
            "wraps"
        );
        conflicts.mark_resolved("b.txt");
        assert!(conflicts.files()[1].resolved);
        assert_eq!(conflicts.unresolved(), 2);
        assert_eq!(
            conflicts.next_unresolved_after("a.txt"),
            Some("c.txt"),
            "skips the applied one"
        );
        conflicts.mark_resolved("a.txt");
        conflicts.mark_resolved("c.txt");
        assert_eq!(conflicts.first_unresolved(), None);
        assert_eq!(conflicts.next_unresolved_after("a.txt"), None);
        conflicts.mark_resolved("not-listed.txt");
        assert_eq!(
            conflicts.unresolved(),
            0,
            "a path not in the list changes nothing"
        );
    }

    #[test]
    fn continue_refusal_counts_what_remains_in_the_singular_and_the_plural() {
        let mut conflicts = RestackConflicts::new(restack(&["a.txt", "b.txt"]));
        assert_eq!(
            conflicts.continue_refusal().as_deref(),
            Some("2 files are still unresolved. Resolve them and Apply, or Cancel.")
        );
        conflicts.mark_resolved("a.txt");
        assert_eq!(
            conflicts.continue_refusal().as_deref(),
            Some("1 file is still unresolved. Resolve it and Apply, or Cancel.")
        );
        conflicts.mark_resolved("b.txt");
        assert_eq!(conflicts.continue_refusal(), None);
    }

    #[test]
    fn with_no_file_left_to_resolve_the_page_says_so_and_continue_may_run() {
        let conflicts = RestackConflicts::new(restack(&[]));
        assert_eq!(conflicts.continue_refusal(), None);
        assert_eq!(
            conflicts.empty_message(),
            "Nothing is left to resolve. Continue restack replays the commit."
        );
        let open = RestackConflicts::new(restack(&["a.txt"]));
        assert_eq!(open.empty_message(), "Opening the conflicted file.");
    }

    #[test]
    fn a_resolver_reads_the_working_copy_unless_it_is_told_the_restack() {
        let read = ConflictFileRead {
            path: "a.txt".into(),
            operation: ConflictOperation::Merge,
            left: side("main", "new parent", "Add"),
            right: side("feature/x", "commit being replayed", "Rewrite"),
            text: "one\n".into(),
            has_base: false,
            recorded: None,
        };
        assert_eq!(ResolverState::new(&read).site(), ConflictSite::WorkingCopy);
        let in_restack = ResolverState::new(&read).with_site(ConflictSite::Restack);
        assert_eq!(in_restack.site(), ConflictSite::Restack);
        assert_eq!(in_restack.path(), "a.txt", "the site changes nothing else");
    }
}

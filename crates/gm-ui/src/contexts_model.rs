//! The Manage Contexts dialog's model (D191, design/threads.md "Contexts"), with no GPUI in it:
//! the contexts as an indented tree in the order the dialog lists them, the context selected and
//! its repositories, the checks a name, a move or a delete gets before it is sent, and the answers
//! to those commands. A read or a write is matched by its `OperationId` as `memories.rs` does: an
//! answer that is not the one in flight is dropped.
//!
//! Context names, paths and repository folders are text the person typed or the engine stored;
//! this module carries them and never reads them as instructions.

use std::collections::{HashMap, HashSet};

use gm_schema::threads::{Context, ContextRepo};
use gm_schema::{CommandKind, OperationId, OperationState};

/// The longest context name, in characters.
pub const NAME_CHARS: usize = 80;
/// The separator of a context path. A name cannot contain it.
const SEPARATOR: &str = " / ";

const NOT_SHOWN: &str = "That context is not shown.";
const NO_PARENT: &str = "Select the parent context first.";
const EMPTY_NAME: &str = "A context needs a name.";
const TOO_LONG: &str = "A context name holds at most 80 characters.";
const HAS_SEPARATOR: &str = "A context name cannot contain \" / \", the path separator.";
const SIBLING_NAME: &str = "Another context here has that name.";
const MOVE_UNDER_ITSELF: &str = "A context cannot move under itself or one of its sub-contexts.";
const HAS_SUB_CONTEXTS: &str =
    "A context with sub-contexts cannot be deleted. Move or delete them first.";
const NO_FOLDER: &str = "Choose a repository folder.";
const NOT_IN_CONTEXT: &str = "That repository is not in this context.";
const NO_CLASH: &str = "This clone does not share its repository id.";
/// The message of a failed command when the engine gave none.
const NO_MESSAGE: &str = "The contexts could not be changed.";

/// One line of the tree: a context and how deep it sits under the roots (0 at a root).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextRow {
    pub id: i64,
    pub depth: usize,
    pub name: String,
    pub path: String,
    pub repo_count: usize,
    pub accepted_memories: u32,
    pub sub_contexts: u32,
    /// Whether the open repository is bound to this context directly (D223 (2)).
    pub bound: bool,
}

/// What a delete would remove, as the engine previews it (`ContextDeletePreview`). The glue
/// converts that event into this struct. `total` counts the memories that belong to no other
/// context; `titles` names them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeletePreview {
    pub sub_contexts: u32,
    pub total: u32,
    pub titles: Vec<String>,
}

/// A delete that needs the person's confirmation before its memories go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteConfirm {
    pub id: i64,
    pub preview: DeletePreview,
}

impl DeleteConfirm {
    /// The delete the person confirmed: the context and the memories held alone.
    #[must_use]
    pub fn confirmed(&self) -> CommandKind {
        CommandKind::DeleteContext {
            id: self.id,
            delete_memories: true,
        }
    }
}

/// The result of planning a delete: sent at once when no memory is held alone, or confirmed first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeletePlan {
    Now(Box<CommandKind>),
    Confirm(DeleteConfirm),
}

/// What a write in flight does to the list when its answer comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Write {
    /// Changes one context or a repository of it: the list is read again and nothing is removed.
    Other,
    /// Deletes context `id`: it leaves the list when the answer comes.
    Removes(i64),
    /// Creates a context: the answer names it, and the next list selects it.
    Creates,
}

/// The state of the Manage Contexts dialog: the contexts, the context selected and the writes
/// in flight.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextsModel {
    contexts: Vec<Context>,
    rows: Vec<ContextRow>,
    listed: bool,
    repo_id: Option<String>,
    clash: Option<String>,
    selected: Option<i64>,
    /// The list read in flight. A later list, or a write, ends it: its late answer is dropped.
    list_pending: Option<OperationId>,
    /// The writes in flight, each with what it does to the list when it answers.
    writes: Vec<(OperationId, Write)>,
    /// The context a create answered with. The next applied list selects it when it is listed.
    select_when_listed: Option<i64>,
    deleted_memories: u32,
    failure: Option<String>,
}

impl ContextsModel {
    /// The contexts as the tree lists them: roots first, siblings by name, case-insensitively.
    #[must_use]
    pub fn rows(&self) -> &[ContextRow] {
        &self.rows
    }

    /// The context with `id`, when it was read.
    #[must_use]
    pub fn find(&self, id: i64) -> Option<&Context> {
        self.contexts.iter().find(|context| context.id == id)
    }

    /// Whether a list has been applied.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.listed
    }

    /// Whether a list read is in flight.
    #[must_use]
    pub fn is_loading(&self) -> bool {
        self.list_pending.is_some()
    }

    /// The folder of the other clone that holds the open repository's id, when there is a clash.
    /// The dialog offers `RenewRepoId` then.
    #[must_use]
    pub fn clash(&self) -> Option<&str> {
        self.clash.as_deref()
    }

    /// The message of the last failed command, if any.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// The memories the last applied delete removed.
    #[must_use]
    pub fn deleted_memories(&self) -> u32 {
        self.deleted_memories
    }

    /// The id of the selected context.
    #[must_use]
    pub fn selected(&self) -> Option<i64> {
        self.selected
    }

    /// The selected context, when one is selected and read.
    #[must_use]
    pub fn selected_context(&self) -> Option<&Context> {
        self.selected.and_then(|id| self.find(id))
    }

    /// The repositories of the selected context, for the right-hand pane.
    #[must_use]
    pub fn selected_repos(&self) -> &[ContextRepo] {
        self.selected_context()
            .map_or(&[], |context| context.repos.as_slice())
    }

    /// Selects context `id`. Returns false when it is not read.
    pub fn select(&mut self, id: i64) -> bool {
        if self.find(id).is_none() {
            return false;
        }
        self.selected = Some(id);
        true
    }

    /// Clears the selection, and the context a create asked to select.
    pub fn deselect(&mut self) {
        self.selected = None;
        self.select_when_listed = None;
    }

    /// The list read, sent from the dialog: every context of the tree, repository or not, each with
    /// whether the open repository is bound to it (D222 (3)).
    #[must_use]
    pub fn plan_list() -> CommandKind {
        CommandKind::ListContexts {
            repo: true,
            all: true,
        }
    }

    /// Records that `kind` was sent as operation `id`. A list read replaces the one in flight. A
    /// write ends the list read in flight, since that answer may predate the write; the dialog
    /// sends a new list once the write answers.
    pub fn sent(&mut self, kind: &CommandKind, id: OperationId) {
        self.failure = None;
        match kind {
            CommandKind::ListContexts { .. } => self.list_pending = Some(id),
            CommandKind::DeleteContext { id: context, .. } => {
                self.list_pending = None;
                self.writes.push((id, Write::Removes(*context)));
            }
            CommandKind::CreateContext { .. } => {
                self.list_pending = None;
                self.writes.push((id, Write::Creates));
            }
            _ => {
                self.list_pending = None;
                self.writes.push((id, Write::Other));
            }
        }
    }

    /// Whether `id` is the list read or one of the writes in flight.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.list_pending == Some(id) || self.writes.iter().any(|(pending, _)| *pending == id)
    }

    /// Applies the list answered by `id`. Returns false for an answer that is not the list read in
    /// flight. The selection stays when its context is still listed and clears otherwise. The
    /// context a create answered with is selected when it is listed, and is not selected otherwise.
    pub fn listed(
        &mut self,
        id: OperationId,
        contexts: Vec<Context>,
        repo_id: Option<String>,
        clash: Option<String>,
    ) -> bool {
        if self.list_pending != Some(id) {
            return false;
        }
        self.list_pending = None;
        self.contexts = unique(contexts);
        self.repo_id = repo_id;
        self.clash = clash;
        self.listed = true;
        self.failure = None;
        self.refresh();
        if let Some(created) = self.select_when_listed.take() {
            self.select(created);
        }
        true
    }

    /// Applies the answer of the write `id`: the context as it now is, or none after a delete
    /// (`deleted_memories` is what the delete removed). Returns false for an answer that is not a
    /// write in flight. The paths of sub-contexts are derived on read, so the dialog sends a new
    /// list after a change.
    pub fn changed(
        &mut self,
        id: OperationId,
        context: Option<Context>,
        deleted_memories: u32,
    ) -> bool {
        let Some(index) = self.writes.iter().position(|(pending, _)| *pending == id) else {
            return false;
        };
        let (_, write) = self.writes.remove(index);
        if let Write::Removes(removed) = write {
            self.contexts.retain(|context| context.id != removed);
        }
        if let Some(context) = context {
            if write == Write::Creates {
                self.select_when_listed = Some(context.id);
            }
            self.upsert(context);
        }
        self.deleted_memories = deleted_memories;
        self.refresh();
        true
    }

    /// A read or write `id` ended without an answer. A failure is kept for the dialog to show; a
    /// stopped one leaves the dialog as it was.
    pub fn ended(&mut self, id: OperationId, state: OperationState, detail: Option<String>) {
        if !self.owns(id) || !state.is_terminal() {
            return;
        }
        if self.list_pending == Some(id) {
            self.list_pending = None;
        }
        self.writes.retain(|(pending, _)| *pending != id);
        if state == OperationState::Failed {
            self.failure = Some(detail.unwrap_or_else(|| NO_MESSAGE.to_owned()));
        }
    }

    /// Checks a context name for a context under `parent` (the root for `None`), ignoring the
    /// sibling `except` (the context being renamed or moved). The name is trimmed.
    pub fn checked_name(
        &self,
        name: &str,
        parent: Option<i64>,
        except: Option<i64>,
    ) -> Result<String, &'static str> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EMPTY_NAME);
        }
        if name.chars().count() > NAME_CHARS {
            return Err(TOO_LONG);
        }
        if name.contains(SEPARATOR) {
            return Err(HAS_SEPARATOR);
        }
        if self.sibling_taken(name, parent, except) {
            return Err(SIBLING_NAME);
        }
        Ok(name.to_owned())
    }

    /// Plans `CreateContext` under the selected context when `parent_selected`, else at the root.
    pub fn new_context(
        &self,
        name: &str,
        parent_selected: bool,
    ) -> Result<CommandKind, &'static str> {
        let parent = if parent_selected {
            Some(self.selected.ok_or(NO_PARENT)?)
        } else {
            None
        };
        let name = self.checked_name(name, parent, None)?;
        Ok(CommandKind::CreateContext { name, parent })
    }

    /// Plans the rename of context `id`.
    pub fn rename(&self, id: i64, name: &str) -> Result<CommandKind, &'static str> {
        let context = self.find(id).ok_or(NOT_SHOWN)?;
        let name = self.checked_name(name, context.parent, Some(id))?;
        Ok(CommandKind::UpdateContext {
            id,
            name: Some(name),
            parent: None,
        })
    }

    /// Plans the move of context `id` under `new_parent`, or to the root for `None`. Refuses a
    /// move under itself or one of its descendants, and a name that a sibling there already has.
    /// The command's parent is `Some(new_parent)`, so a root move is `"parent": null`.
    pub fn move_to(&self, id: i64, new_parent: Option<i64>) -> Result<CommandKind, &'static str> {
        let context = self.find(id).ok_or(NOT_SHOWN)?;
        if let Some(parent) = new_parent {
            if self.find(parent).is_none() {
                return Err(NOT_SHOWN);
            }
            if self.ancestors_inclusive(parent).contains(&id) {
                return Err(MOVE_UNDER_ITSELF);
            }
        }
        if self.sibling_taken(&context.name, new_parent, Some(id)) {
            return Err(SIBLING_NAME);
        }
        Ok(CommandKind::UpdateContext {
            id,
            name: None,
            parent: Some(new_parent),
        })
    }

    /// The parents the Move… picker offers for context `id`: the top level and every context that
    /// is not `id` or one of its sub-contexts, filtered by `filter`.
    #[must_use]
    pub fn move_choices(&self, id: i64, filter: &str) -> Vec<Pick> {
        let excluded: HashSet<i64> = self
            .contexts
            .iter()
            .filter(|context| self.ancestors_inclusive(context.id).contains(&id))
            .map(|context| context.id)
            .collect();
        pick(&self.contexts, filter, &excluded, true)
    }

    /// Plans `AddRepoToContext` for the folder `path`.
    pub fn add_repository(&self, context: i64, path: &str) -> Result<CommandKind, &'static str> {
        self.find(context).ok_or(NOT_SHOWN)?;
        let path = path.trim();
        if path.is_empty() {
            return Err(NO_FOLDER);
        }
        Ok(CommandKind::AddRepoToContext {
            context,
            path: path.to_owned(),
        })
    }

    /// Plans `RemoveRepoFromContext`. The repository must be in the context.
    pub fn remove_repository(
        &self,
        context: i64,
        repo_id: &str,
    ) -> Result<CommandKind, &'static str> {
        let found = self.find(context).ok_or(NOT_SHOWN)?;
        if !found.repos.iter().any(|repo| repo.repo_id == repo_id) {
            return Err(NOT_IN_CONTEXT);
        }
        Ok(CommandKind::RemoveRepoFromContext {
            context,
            repo_id: repo_id.to_owned(),
        })
    }

    /// Plans `RenewRepoId` for the open repository. Only a clash allows it.
    pub fn renew_repo_id(&self) -> Result<CommandKind, &'static str> {
        if self.clash.is_none() {
            return Err(NO_CLASH);
        }
        Ok(CommandKind::RenewRepoId)
    }

    /// Plans the delete of context `id` from its preview. Refused while it has sub-contexts. A
    /// context with no memory held alone is deleted at once; one with memories asks first.
    pub fn delete(&self, id: i64, preview: &DeletePreview) -> Result<DeletePlan, &'static str> {
        let context = self.find(id).ok_or(NOT_SHOWN)?;
        if context.sub_contexts > 0 || preview.sub_contexts > 0 {
            return Err(HAS_SUB_CONTEXTS);
        }
        if preview.total == 0 {
            return Ok(DeletePlan::Now(Box::new(CommandKind::DeleteContext {
                id,
                delete_memories: false,
            })));
        }
        Ok(DeletePlan::Confirm(DeleteConfirm {
            id,
            preview: preview.clone(),
        }))
    }

    /// Whether a sibling of `parent` other than `except` has `name`, ignoring case.
    fn sibling_taken(&self, name: &str, parent: Option<i64>, except: Option<i64>) -> bool {
        let wanted = name.to_lowercase();
        self.contexts.iter().any(|context| {
            context.parent == parent
                && Some(context.id) != except
                && context.name.to_lowercase() == wanted
        })
    }

    /// `id` and the contexts above it, following parents. Stops at a cycle or a missing parent.
    fn ancestors_inclusive(&self, id: i64) -> HashSet<i64> {
        let mut seen = HashSet::new();
        let mut current = Some(id);
        while let Some(next) = current {
            if !seen.insert(next) {
                break;
            }
            current = self.find(next).and_then(|context| context.parent);
        }
        seen
    }

    fn upsert(&mut self, context: Context) {
        match self.contexts.iter().position(|old| old.id == context.id) {
            Some(index) => self.contexts[index] = context,
            None => self.contexts.push(context),
        }
    }

    /// Rebuilds the rows and drops the selection when its context is no longer read.
    fn refresh(&mut self) {
        self.rows = build_rows(&self.contexts);
        if self.selected.is_some_and(|id| self.find(id).is_none()) {
            self.selected = None;
        }
    }
}

/// The label of the choice that makes a context a root.
pub const TOP_LEVEL: &str = "Top level";

/// One choice of a filterable context picker: a context by its id and path, or the top level
/// (`id` none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub id: Option<i64>,
    pub label: String,
}

/// The choices of a filterable context picker. A context is listed when its path holds `filter`,
/// ignoring case, unless it is `excluded`; the contexts come by path. The top level comes first
/// when `top_level` is set and the filter names it. An empty filter lists everything.
#[must_use]
pub fn pick(
    contexts: &[Context],
    filter: &str,
    excluded: &HashSet<i64>,
    top_level: bool,
) -> Vec<Pick> {
    let wanted = filter.trim().to_lowercase();
    let mut contexts: Vec<&Context> = contexts
        .iter()
        .filter(|context| !excluded.contains(&context.id))
        .filter(|context| context.path.to_lowercase().contains(&wanted))
        .collect();
    contexts.sort_by_key(|context| context.path.to_lowercase());
    let mut choices = Vec::with_capacity(contexts.len() + 1);
    if top_level && TOP_LEVEL.to_lowercase().contains(&wanted) {
        choices.push(Pick {
            id: None,
            label: TOP_LEVEL.to_owned(),
        });
    }
    choices.extend(contexts.into_iter().map(|context| Pick {
        id: Some(context.id),
        label: context.path.clone(),
    }));
    choices
}

/// The contexts with their first occurrence of each id. A repeated id is not listed twice.
fn unique(contexts: Vec<Context>) -> Vec<Context> {
    let mut seen = HashSet::new();
    contexts
        .into_iter()
        .filter(|context| seen.insert(context.id))
        .collect()
}

/// The tree in list order, pre-order: each context, then its sub-contexts by name. A parent id
/// that is not read makes its context a root. A cycle in the input has no root, so each cycle is
/// entered at one of its members; every context is listed exactly once and the walk terminates.
fn build_rows(contexts: &[Context]) -> Vec<ContextRow> {
    let index: HashMap<i64, usize> = contexts
        .iter()
        .enumerate()
        .map(|(position, context)| (context.id, position))
        .collect();
    let parent_of: Vec<Option<usize>> = contexts
        .iter()
        .enumerate()
        .map(|(position, context)| {
            context
                .parent
                .and_then(|parent| index.get(&parent).copied())
                .filter(|&parent| parent != position)
        })
        .collect();

    let mut order: Vec<usize> = (0..contexts.len()).collect();
    order.sort_by_key(|&position| {
        (
            contexts[position].name.to_lowercase(),
            contexts[position].id,
        )
    });
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); contexts.len()];
    for &position in &order {
        if let Some(parent) = parent_of[position] {
            children[parent].push(position);
        }
    }

    // The roots first, by name; then the contexts a cycle keeps from any root.
    let starts = order
        .iter()
        .copied()
        .filter(|&position| parent_of[position].is_none())
        .chain(
            order
                .iter()
                .copied()
                .filter(|&position| parent_of[position].is_some()),
        );
    let mut visited = vec![false; contexts.len()];
    let mut rows = Vec::with_capacity(contexts.len());
    for start in starts {
        if visited[start] {
            continue;
        }
        // Climb to the top of the chain: a root, or a member of a cycle the climb closes.
        let mut top = start;
        let mut climbed = HashSet::from([start]);
        while let Some(parent) = parent_of[top] {
            if visited[parent] || !climbed.insert(parent) {
                break;
            }
            top = parent;
        }
        let mut stack = vec![(top, 0_usize)];
        while let Some((position, depth)) = stack.pop() {
            if visited[position] {
                continue;
            }
            visited[position] = true;
            let context = &contexts[position];
            rows.push(ContextRow {
                id: context.id,
                depth,
                name: context.name.clone(),
                path: context.path.clone(),
                repo_count: context.repos.len(),
                accepted_memories: context.accepted_memories,
                sub_contexts: context.sub_contexts,
                bound: context.bound,
            });
            for &child in children[position].iter().rev() {
                if !visited[child] {
                    stack.push((child, depth + 1));
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(id: i64, name: &str, parent: Option<i64>) -> Context {
        Context {
            id,
            uid: format!("uid-{id}"),
            name: name.to_owned(),
            parent,
            path: name.to_owned(),
            repos: Vec::new(),
            accepted_memories: 0,
            sub_contexts: 0,
            members: Vec::new(),
            bound: false,
            inherited_memories: 0,
        }
    }

    #[test]
    fn the_tree_lists_a_context_with_no_repository_and_marks_the_bound_ones() {
        let mut platform = context(1, "Platform", None);
        platform.bound = true;
        let billing = context(2, "Billing", Some(1));
        let sales = context(3, "Sales", None);
        let rows = build_rows(&[sales, billing, platform]);
        let shown: Vec<(&str, usize, bool, usize)> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.depth, row.bound, row.repo_count))
            .collect();
        assert_eq!(
            shown,
            [
                ("Platform", 0, true, 0),
                ("Billing", 1, false, 0),
                ("Sales", 0, false, 0),
            ],
            "every context is a row, with or without a repository"
        );
    }

    #[test]
    fn the_list_read_asks_for_every_context_with_the_open_repository_identity() {
        assert_eq!(
            ContextsModel::plan_list(),
            CommandKind::ListContexts {
                repo: true,
                all: true,
            }
        );
    }

    /// The repro's store: `billing` bound to itself and to Finance / Payments. The Memories read
    /// answers first with an id the dialog does not hold; the dialog's own read is then applied.
    #[test]
    fn the_dialog_read_is_applied_when_the_memories_read_answers_first() {
        let mut billing = context(1, "billing", None);
        billing.bound = true;
        billing.repos = vec![repo("billing")];
        let finance = context(2, "Finance", None);
        let mut payments = context(3, "Payments", Some(2));
        payments.path = "Finance / Payments".to_owned();
        payments.bound = true;
        let mut accounting = context(4, "Accounting", Some(2));
        accounting.path = "Finance / Accounting".to_owned();
        let incident = context(5, "Incident: stuck exports", None);
        let all = vec![billing, finance, payments, accounting, incident];

        let mut model = ContextsModel::default();
        model.sent(&ContextsModel::plan_list(), OperationId(20));
        assert!(!model.listed(OperationId(19), all.clone(), None, None));
        assert!(!model.is_loaded());
        assert!(model.listed(OperationId(20), all, Some("repo".to_owned()), None));
        let shown: Vec<(&str, usize, bool)> = model
            .rows()
            .iter()
            .map(|row| (row.name.as_str(), row.depth, row.bound))
            .collect();
        assert_eq!(
            shown,
            [
                ("billing", 0, true),
                ("Finance", 0, false),
                ("Accounting", 1, false),
                ("Payments", 1, true),
                ("Incident: stuck exports", 0, false),
            ]
        );
    }

    fn repo(repo_id: &str) -> ContextRepo {
        ContextRepo {
            repo_id: repo_id.to_owned(),
            path: format!("/work/{repo_id}"),
            name: repo_id.to_owned(),
        }
    }

    /// The guide's example: Platform with Billing under it, and Partners at the root.
    fn guide_tree() -> Vec<Context> {
        let mut platform = context(1, "Platform", None);
        platform.repos = vec![repo("api"), repo("web")];
        let mut billing = context(2, "Billing", Some(1));
        billing.repos = vec![repo("billing")];
        let partners = context(3, "Partners", None);
        vec![platform, billing, partners]
    }

    fn model_with(contexts: Vec<Context>) -> ContextsModel {
        let mut model = ContextsModel::default();
        let id = OperationId(1);
        model.sent(&ContextsModel::plan_list(), id);
        assert!(model.listed(id, contexts, Some("this-repo".to_owned()), None));
        model
    }

    fn names(model: &ContextsModel) -> Vec<(String, usize)> {
        model
            .rows()
            .iter()
            .map(|row| (row.name.clone(), row.depth))
            .collect()
    }

    fn no_preview() -> DeletePreview {
        DeletePreview::default()
    }

    #[test]
    fn the_guide_example_lists_roots_first_with_sub_contexts_indented() {
        let model = model_with(guide_tree());
        assert_eq!(
            names(&model),
            vec![
                ("Partners".to_owned(), 0),
                ("Platform".to_owned(), 0),
                ("Billing".to_owned(), 1),
            ]
        );
        let platform = model.rows().iter().find(|row| row.id == 1).unwrap();
        assert_eq!(platform.repo_count, 2);
    }

    #[test]
    fn siblings_sort_by_name_case_insensitively() {
        let model = model_with(vec![
            context(1, "zeta", None),
            context(2, "Alpha", None),
            context(3, "beta", None),
        ]);
        let order: Vec<&str> = model.rows().iter().map(|row| row.name.as_str()).collect();
        assert_eq!(order, ["Alpha", "beta", "zeta"]);
    }

    #[test]
    fn a_missing_parent_makes_its_context_a_root() {
        let model = model_with(vec![context(7, "Orphan", Some(99))]);
        assert_eq!(names(&model), vec![("Orphan".to_owned(), 0)]);
    }

    #[test]
    fn a_cycle_and_a_self_parent_terminate_and_list_every_context_once() {
        let model = model_with(vec![
            context(1, "a", Some(2)),
            context(2, "b", Some(1)),
            context(3, "self", Some(3)),
            context(4, "root", None),
        ]);
        let mut ids: Vec<i64> = model.rows().iter().map(|row| row.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2, 3, 4]);
        assert!(model.rows().iter().all(|row| row.depth <= 1));
    }

    #[test]
    fn a_move_under_itself_or_a_descendant_is_refused() {
        let model = model_with(guide_tree());
        assert_eq!(model.move_to(1, Some(1)), Err(MOVE_UNDER_ITSELF));
        assert_eq!(model.move_to(1, Some(2)), Err(MOVE_UNDER_ITSELF));
        assert!(model.move_to(2, Some(3)).is_ok());
        assert_eq!(model.move_to(1, Some(42)), Err(NOT_SHOWN));
    }

    #[test]
    fn a_move_to_the_root_is_a_null_parent_on_the_wire() {
        let model = model_with(guide_tree());
        let kind = model.move_to(2, None).unwrap();
        assert_eq!(
            kind,
            CommandKind::UpdateContext {
                id: 2,
                name: None,
                parent: Some(None),
            }
        );
        let json = serde_json::to_value(&kind).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "update_context", "id": 2, "name": null, "parent": null })
        );
        assert!(json["parent"].is_null());
    }

    #[test]
    fn a_duplicate_sibling_name_is_refused_case_insensitively() {
        let model = model_with(guide_tree());
        assert_eq!(
            model.new_context("  platform ", false),
            Err(SIBLING_NAME),
            "a root named like a root sibling"
        );
        assert!(
            model.new_context("Billing", false).is_ok(),
            "Billing is under Platform, not a root"
        );
        assert_eq!(model.new_context("PARTNERS", false), Err(SIBLING_NAME));
        assert!(
            model.checked_name("Billing", Some(3), None).is_ok(),
            "the same name under another parent is fine"
        );
        assert_eq!(model.rename(3, "platform"), Err(SIBLING_NAME));
        assert!(
            model.rename(3, "Partners").is_ok(),
            "a context keeps its own name"
        );
    }

    #[test]
    fn a_move_to_a_parent_with_a_sibling_of_the_same_name_is_refused() {
        let mut model = model_with(guide_tree());
        let id = OperationId(7);
        model.sent(&ContextsModel::plan_list(), id);
        assert!(model.listed(
            id,
            vec![
                context(1, "Platform", None),
                context(2, "Billing", Some(1)),
                context(4, "Billing", None)
            ],
            None,
            None,
        ));
        assert_eq!(model.move_to(2, None), Err(SIBLING_NAME));
        assert!(
            model.move_to(2, Some(1)).is_ok(),
            "under Platform there is no other Billing"
        );
    }

    #[test]
    fn a_name_is_trimmed_and_checked_for_length_and_the_path_separator() {
        let model = model_with(Vec::new());
        assert_eq!(model.checked_name("   ", None, None), Err(EMPTY_NAME));
        let long = "x".repeat(NAME_CHARS + 1);
        assert_eq!(model.checked_name(&long, None, None), Err(TOO_LONG));
        let exact = "x".repeat(NAME_CHARS);
        assert_eq!(model.checked_name(&exact, None, None), Ok(exact.clone()));
        assert_eq!(
            model.checked_name("Platform / Billing", None, None),
            Err(HAS_SEPARATOR)
        );
        assert_eq!(
            model.checked_name(" Ops ", None, None),
            Ok("Ops".to_owned())
        );
    }

    #[test]
    fn a_new_sub_context_needs_a_selected_parent() {
        let mut model = model_with(guide_tree());
        assert_eq!(model.new_context("Ledger", true), Err(NO_PARENT));
        assert!(model.select(1));
        assert_eq!(
            model.new_context("Ledger", true),
            Ok(CommandKind::CreateContext {
                name: "Ledger".to_owned(),
                parent: Some(1),
            })
        );
        assert_eq!(
            model.new_context("Ledger", false),
            Ok(CommandKind::CreateContext {
                name: "Ledger".to_owned(),
                parent: None,
            })
        );
    }

    #[test]
    fn a_delete_with_sub_contexts_is_refused() {
        let mut platform = context(1, "Platform", None);
        platform.sub_contexts = 1;
        let model = model_with(vec![platform, context(2, "Billing", Some(1))]);
        assert_eq!(model.delete(1, &no_preview()), Err(HAS_SUB_CONTEXTS));
        let preview = DeletePreview {
            sub_contexts: 1,
            total: 0,
            titles: Vec::new(),
        };
        let from_the_preview = model_with(guide_tree());
        assert_eq!(from_the_preview.delete(1, &preview), Err(HAS_SUB_CONTEXTS));
    }

    #[test]
    fn a_delete_with_no_memory_held_alone_goes_at_once() {
        let model = model_with(guide_tree());
        assert_eq!(
            model.delete(2, &no_preview()),
            Ok(DeletePlan::Now(Box::new(CommandKind::DeleteContext {
                id: 2,
                delete_memories: false,
            })))
        );
    }

    #[test]
    fn a_delete_with_two_memories_held_alone_asks_first_then_deletes_them() {
        let model = model_with(guide_tree());
        let preview = DeletePreview {
            sub_contexts: 0,
            total: 2,
            titles: vec!["Retry budget".to_owned(), "Invoice rounding".to_owned()],
        };
        let plan = model.delete(2, &preview).unwrap();
        let DeletePlan::Confirm(confirm) = plan else {
            panic!("expected the confirm state, got {plan:?}");
        };
        assert_eq!(confirm.id, 2);
        assert_eq!(confirm.preview.total, 2);
        assert_eq!(confirm.preview.titles.len(), 2);
        assert_eq!(
            serde_json::to_value(confirm.confirmed()).unwrap(),
            serde_json::json!({ "type": "delete_context", "id": 2, "delete_memories": true })
        );
    }

    #[test]
    fn a_repository_is_added_by_its_folder_and_removed_only_when_it_is_in_the_context() {
        let model = model_with(guide_tree());
        assert_eq!(model.add_repository(2, "  "), Err(NO_FOLDER));
        assert_eq!(
            model.add_repository(2, " /work/sync "),
            Ok(CommandKind::AddRepoToContext {
                context: 2,
                path: "/work/sync".to_owned(),
            })
        );
        assert_eq!(model.remove_repository(2, "api"), Err(NOT_IN_CONTEXT));
        assert_eq!(
            model.remove_repository(1, "api"),
            Ok(CommandKind::RemoveRepoFromContext {
                context: 1,
                repo_id: "api".to_owned(),
            })
        );
    }

    #[test]
    fn the_renew_planner_needs_a_clash() {
        let mut model = model_with(guide_tree());
        assert_eq!(model.renew_repo_id(), Err(NO_CLASH));
        let id = OperationId(2);
        model.sent(&ContextsModel::plan_list(), id);
        assert!(model.listed(
            id,
            guide_tree(),
            Some("this-repo".to_owned()),
            Some("/work/other".to_owned()),
        ));
        assert_eq!(model.clash(), Some("/work/other"));
        assert_eq!(model.renew_repo_id(), Ok(CommandKind::RenewRepoId));
    }

    #[test]
    fn the_selected_repositories_follow_the_selection() {
        let mut model = model_with(guide_tree());
        assert!(model.selected_repos().is_empty());
        assert!(!model.select(99));
        assert!(model.select(1));
        let repos: Vec<(&str, &str)> = model
            .selected_repos()
            .iter()
            .map(|repo| (repo.name.as_str(), repo.path.as_str()))
            .collect();
        assert_eq!(repos, [("api", "/work/api"), ("web", "/work/web")]);
    }

    #[test]
    fn the_selection_survives_a_refresh_and_clears_when_its_context_is_gone() {
        let mut model = model_with(guide_tree());
        assert!(model.select(2));

        let id = OperationId(5);
        model.sent(&ContextsModel::plan_list(), id);
        assert!(model.listed(
            id,
            vec![context(2, "Billing", Some(1)), context(4, "New", None)],
            None,
            None,
        ));
        assert_eq!(model.selected(), Some(2), "the id still exists");

        let id = OperationId(6);
        model.sent(&ContextsModel::plan_list(), id);
        assert!(model.listed(id, vec![context(4, "New", None)], None, None));
        assert_eq!(model.selected(), None, "the id is gone");
    }

    #[test]
    fn a_late_list_answer_is_dropped_and_so_is_an_answer_to_no_request() {
        let mut model = ContextsModel::default();
        model.sent(&ContextsModel::plan_list(), OperationId(1));
        model.sent(&ContextsModel::plan_list(), OperationId(2));
        assert!(
            !model.listed(OperationId(1), vec![context(1, "Stale", None)], None, None),
            "the first list was superseded"
        );
        assert!(!model.is_loaded());
        assert!(model.listed(OperationId(2), vec![context(2, "Fresh", None)], None, None));
        assert_eq!(names(&model), vec![("Fresh".to_owned(), 0)]);
        assert!(!model.listed(OperationId(9), Vec::new(), None, None));
    }

    #[test]
    fn a_write_drops_the_list_in_flight_so_its_older_answer_is_not_shown() {
        let mut model = model_with(guide_tree());
        model.sent(&ContextsModel::plan_list(), OperationId(3));
        model.sent(
            &CommandKind::CreateContext {
                name: "Ops".to_owned(),
                parent: None,
            },
            OperationId(4),
        );
        assert!(!model.listed(OperationId(3), Vec::new(), None, None));
        assert!(model.is_loaded());
        assert_eq!(
            model.rows().len(),
            3,
            "the stale answer did not replace the rows"
        );
    }

    #[test]
    fn a_create_and_a_delete_answer_update_the_tree_and_the_deleted_count() {
        let mut model = model_with(guide_tree());
        let create = CommandKind::CreateContext {
            name: "Ops".to_owned(),
            parent: None,
        };
        model.sent(&create, OperationId(10));
        assert!(model.changed(OperationId(10), Some(context(4, "Ops", None)), 0));
        assert!(model.find(4).is_some());

        model.sent(
            &CommandKind::DeleteContext {
                id: 2,
                delete_memories: true,
            },
            OperationId(11),
        );
        assert!(model.changed(OperationId(11), None, 3));
        assert!(model.find(2).is_none());
        assert_eq!(model.deleted_memories(), 3);
        assert!(
            !model.changed(OperationId(11), None, 0),
            "an answer is applied once"
        );
    }

    #[test]
    fn a_failed_write_keeps_its_message_and_a_stopped_one_does_not() {
        let mut model = model_with(guide_tree());
        model.sent(&CommandKind::RenewRepoId, OperationId(20));
        model.ended(OperationId(20), OperationState::Cancelled, None);
        assert_eq!(model.failure(), None);
        model.sent(&CommandKind::RenewRepoId, OperationId(21));
        model.ended(OperationId(21), OperationState::Failed, None);
        assert_eq!(model.failure(), Some(NO_MESSAGE));
    }

    /// The answer of a create of `name` under the root, as the engine gives it.
    fn created(model: &mut ContextsModel, write: u64, id: i64, name: &str) {
        let create = CommandKind::CreateContext {
            name: name.to_owned(),
            parent: None,
        };
        model.sent(&create, OperationId(write));
        assert!(model.changed(OperationId(write), Some(context(id, name, None)), 0));
    }

    #[test]
    fn a_created_context_is_selected_once_the_list_is_read_again() {
        let mut model = model_with(guide_tree());
        assert!(model.select(1));
        created(&mut model, 10, 4, "Ops");
        assert_eq!(model.selected(), Some(1), "the list is not read yet");

        model.sent(&ContextsModel::plan_list(), OperationId(11));
        let mut listed = guide_tree();
        listed.push(context(4, "Ops", None));
        assert!(model.listed(OperationId(11), listed, None, None));
        assert_eq!(model.selected(), Some(4));
        assert_eq!(
            model.selected_context().map(|c| c.name.as_str()),
            Some("Ops")
        );
    }

    #[test]
    fn a_created_sub_context_is_selected_when_listed_under_its_parent() {
        let mut model = model_with(guide_tree());
        assert!(model.select(1));
        let create = CommandKind::CreateContext {
            name: "Ledger".to_owned(),
            parent: Some(1),
        };
        model.sent(&create, OperationId(12));
        assert!(model.changed(OperationId(12), Some(context(5, "Ledger", Some(1))), 0));
        model.sent(&ContextsModel::plan_list(), OperationId(13));
        let mut listed = guide_tree();
        listed.push(context(5, "Ledger", Some(1)));
        assert!(model.listed(OperationId(13), listed, None, None));
        assert_eq!(model.selected(), Some(5));
    }

    #[test]
    fn a_rename_keeps_the_current_selection() {
        let mut model = model_with(guide_tree());
        assert!(model.select(1));
        let rename = CommandKind::UpdateContext {
            id: 2,
            name: Some("Invoices".to_owned()),
            parent: None,
        };
        model.sent(&rename, OperationId(14));
        assert!(model.changed(OperationId(14), Some(context(2, "Invoices", Some(1))), 0));
        model.sent(&ContextsModel::plan_list(), OperationId(15));
        let mut listed = guide_tree();
        listed[1].name = "Invoices".to_owned();
        assert!(model.listed(OperationId(15), listed, None, None));
        assert_eq!(model.selected(), Some(1), "the rename selects nothing");

        let mut none = model_with(guide_tree());
        let rename = CommandKind::UpdateContext {
            id: 3,
            name: Some("Allies".to_owned()),
            parent: None,
        };
        none.sent(&rename, OperationId(16));
        assert!(none.changed(OperationId(16), Some(context(3, "Allies", None)), 0));
        none.sent(&ContextsModel::plan_list(), OperationId(17));
        assert!(none.listed(OperationId(17), guide_tree(), None, None));
        assert_eq!(none.selected(), None);
    }

    #[test]
    fn a_created_context_missing_from_the_next_list_selects_nothing() {
        let mut model = model_with(guide_tree());
        assert!(model.select(1));
        created(&mut model, 18, 4, "Ops");
        model.sent(&ContextsModel::plan_list(), OperationId(19));
        assert!(model.listed(OperationId(19), guide_tree(), None, None));
        assert_eq!(
            model.selected(),
            Some(1),
            "the selection stays; the created context is not listed, so it is not selected"
        );

        model.sent(&ContextsModel::plan_list(), OperationId(20));
        let mut listed = guide_tree();
        listed.push(context(4, "Ops", None));
        assert!(model.listed(OperationId(20), listed, None, None));
        assert_eq!(
            model.selected(),
            Some(1),
            "the create's selection was used up by the first list"
        );
    }

    #[test]
    fn a_failed_create_changes_nothing_and_its_refusal_is_kept() {
        let mut model = model_with(guide_tree());
        assert!(model.select(1));
        let create = CommandKind::CreateContext {
            name: "Ops".to_owned(),
            parent: None,
        };
        model.sent(&create, OperationId(21));
        model.ended(
            OperationId(21),
            OperationState::Failed,
            Some("the database is locked".to_owned()),
        );
        assert_eq!(model.failure(), Some("the database is locked"));
        assert_eq!(model.selected(), Some(1));

        model.sent(&ContextsModel::plan_list(), OperationId(22));
        let mut listed = guide_tree();
        listed.push(context(4, "Ops", None));
        assert!(model.listed(OperationId(22), listed, None, None));
        assert_eq!(
            model.selected(),
            Some(1),
            "no context was created to select"
        );
    }

    #[test]
    fn a_refused_repository_keeps_git_s_reason_for_the_dialog() {
        let mut model = model_with(guide_tree());
        assert!(model.select(1));
        let add = model.add_repository(1, "/work/notes").unwrap();
        model.sent(&add, OperationId(23));
        model.ended(
            OperationId(23),
            OperationState::Failed,
            Some("/work/notes is not a Git repository.".to_owned()),
        );
        assert_eq!(
            model.failure(),
            Some("/work/notes is not a Git repository.")
        );
        assert_eq!(model.selected(), Some(1));
    }

    #[test]
    fn closing_the_dialog_drops_a_pending_selection() {
        let mut model = model_with(guide_tree());
        created(&mut model, 24, 4, "Ops");
        model.deselect();
        model.sent(&ContextsModel::plan_list(), OperationId(25));
        let mut listed = guide_tree();
        listed.push(context(4, "Ops", None));
        assert!(model.listed(OperationId(25), listed, None, None));
        assert_eq!(model.selected(), None);
    }

    /// `context` with its path given, as the engine derives it from the parents.
    fn at_path(id: i64, path: &str, parent: Option<i64>) -> Context {
        Context {
            path: path.to_owned(),
            ..context(id, path.rsplit(" / ").next().unwrap_or(path), parent)
        }
    }

    #[test]
    fn the_picker_lists_by_path_case_insensitively_with_the_top_level_first() {
        let contexts = vec![
            at_path(1, "Platform", None),
            at_path(2, "Platform / Billing", Some(1)),
            at_path(3, "Partners", None),
        ];
        let all = pick(&contexts, "", &HashSet::new(), true);
        let labels: Vec<&str> = all.iter().map(|choice| choice.label.as_str()).collect();
        assert_eq!(
            labels,
            [TOP_LEVEL, "Partners", "Platform", "Platform / Billing"]
        );
        assert_eq!(all[0].id, None);
        let narrowed = pick(&contexts, "BILL", &HashSet::new(), true);
        assert_eq!(
            narrowed,
            vec![Pick {
                id: Some(2),
                label: "Platform / Billing".to_owned()
            }]
        );
        assert!(
            pick(&contexts, "bill", &HashSet::new(), false)
                .iter()
                .all(|choice| choice.id.is_some())
        );
        assert_eq!(pick(&contexts, "top", &HashSet::new(), true)[0].id, None);
        assert!(pick(&contexts, "top", &HashSet::new(), false).is_empty());
    }

    #[test]
    fn the_picker_leaves_out_the_excluded_contexts() {
        let contexts = vec![at_path(1, "Platform", None), at_path(2, "Partners", None)];
        let excluded = HashSet::from([2]);
        let labels: Vec<String> = pick(&contexts, "", &excluded, false)
            .into_iter()
            .map(|choice| choice.label)
            .collect();
        assert_eq!(labels, ["Platform"]);
    }

    #[test]
    fn a_move_offers_the_top_level_and_every_context_but_the_moved_one_and_its_subtree() {
        let mut platform = at_path(1, "Platform", None);
        platform.repos = vec![repo("api")];
        let model = model_with(vec![
            platform,
            at_path(2, "Platform / Billing", Some(1)),
            at_path(3, "Platform / Billing / Jobs", Some(2)),
            at_path(4, "Partners", None),
        ]);
        let choices: Vec<Option<i64>> = model
            .move_choices(1, "")
            .into_iter()
            .map(|choice| choice.id)
            .collect();
        assert_eq!(
            choices,
            [None, Some(4)],
            "Platform cannot go under its own subtree"
        );
        let under_billing: Vec<Option<i64>> = model
            .move_choices(2, "")
            .into_iter()
            .map(|choice| choice.id)
            .collect();
        assert_eq!(
            under_billing,
            [None, Some(4), Some(1)],
            "by path: Partners before Platform"
        );
    }
}

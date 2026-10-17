//! The action registry: every action the window can run, with its stable id,
//! title, default shortcut, typed target and its "enabled now?" rule. Keys,
//! buttons, menus and the palette read it, and `plan` is the one path from a
//! run to the engine commands it sends. Nothing here touches GPUI, so the rules
//! are unit-tested on their own.

// `from_id` has no caller outside the tests yet.
#![allow(dead_code)]

use std::path::PathBuf;

use gm_schema::{CommandKind, DiffSide, HunkId, OperationId, StageTarget, StatusEntry};

/// Why an action is not enabled, as a short English sentence for the user.
pub type Reason = &'static str;

/// What a menu or the palette groups an action under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    WorkingCopy,
    Commit,
    Branches,
    Remote,
    Worktrees,
    Repository,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::WorkingCopy => "Working Copy",
            Category::Commit => "Commit",
            Category::Branches => "Branches",
            Category::Remote => "Remote",
            Category::Worktrees => "Worktrees",
            Category::Repository => "Repository",
        }
    }
}

/// The kind of target an action takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgKind {
    None,
    Branch,
    File,
    Chunk,
}

/// The target of one run. Its kind must match the action's `ArgKind`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Args {
    None,
    Branch(String),
    File(String),
    /// A hunk of one side of `path`. The id is checked by the engine, so a
    /// stale one is refused there.
    Chunk {
        path: String,
        hunk: HunkId,
        side: DiffSide,
    },
}

/// Static description of an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub id: &'static str,
    pub title: &'static str,
    /// Display string of the default key, if the action has one.
    pub shortcut: Option<&'static str>,
    pub args: ArgKind,
    pub category: Category,
}

/// What a successful run does. The window performs it; nothing else differs
/// between a key, a button and a menu item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Engine commands, in order, all with `Actor::Human`.
    Send(Vec<CommandKind>),
    /// Text for the clipboard.
    Copy(String),
    /// Replaces the composer text with the suggestion.
    UseSuggestion(String),
    /// Drops the suggestion; the composer keeps its text.
    DiscardSuggestion,
    /// Hides the approval prompt of the repository hook with this command hash.
    DenyHook(String),
    /// A path on disk, for the system file manager.
    Reveal(PathBuf),
    FocusCommitMessage,
}

/// What the window knows of the prepare-commit hooks, for the rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HookCtx<'a> {
    /// The run that is starting or running, if one is.
    pub running: Option<OperationId>,
    /// How many hooks apply to this branch and are not disabled; `None` until listed.
    pub applicable: Option<usize>,
    /// The message the last run offered and the user has not used or dropped.
    pub suggestion: Option<&'a str>,
    /// The first repository hook awaiting approval: its name and command hash.
    pub pending_approval: Option<(&'a str, &'a str)>,
}

/// The window state an action's rule reads. Built by the window for each run.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// A repository is open (the welcome screen has none).
    pub repo_open: bool,
    /// Why a repository change must wait right now, if it must: an open or a
    /// checkout in flight.
    pub busy: Option<Reason>,
    pub entries: &'a [StatusEntry],
    pub staged_count: usize,
    /// The commit message as typed.
    pub message: &'a str,
    /// The checked-out branch; `None` when HEAD is detached or unborn.
    pub current_branch: Option<&'a str>,
    /// The repository's working tree root; `None` when no repository is open.
    pub root: Option<&'a str>,
    pub hooks: HookCtx<'a>,
}

/// The specs, in the order of `Action`'s variants (a test keeps them in step).
const SPECS: [Spec; 27] = [
    spec(
        "file.stage",
        "Stage File",
        Some("Space"),
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "file.unstage",
        "Unstage File",
        Some("Space"),
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "chunk.stage",
        "Stage Chunk",
        Some("A"),
        ArgKind::Chunk,
        Category::WorkingCopy,
    ),
    spec(
        "chunk.unstage",
        "Unstage Chunk",
        Some("A"),
        ArgKind::Chunk,
        Category::WorkingCopy,
    ),
    spec(
        "chunk.discard",
        "Discard Chunk",
        None,
        ArgKind::Chunk,
        Category::WorkingCopy,
    ),
    spec(
        "commit.create",
        "Commit",
        Some("⌘↵"),
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "repo.refresh",
        "Refresh",
        Some("R"),
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "branch.checkout",
        "Checkout Branch",
        None,
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "branch.copy_name",
        "Copy Branch Name",
        None,
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "file.copy_path",
        "Copy File Path",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "commit.focus_message",
        "Focus Commit Message",
        None,
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "branch.new_child",
        "New Child Branch",
        None,
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "stack.restack",
        "Restack This Stack",
        None,
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "remote.push_with_lease",
        "Push With Lease",
        None,
        ArgKind::Branch,
        Category::Remote,
    ),
    spec(
        "branch.rename",
        "Rename Branch",
        None,
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "branch.delete",
        "Delete Branch",
        None,
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "worktree.create",
        "Create Worktree",
        None,
        ArgKind::Branch,
        Category::Worktrees,
    ),
    spec(
        "file.discard",
        "Discard Changes",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "file.reveal",
        "Reveal in File Manager",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "commit.suggest_message",
        "Suggest Commit Message",
        None,
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "commit.use_suggestion",
        "Replace Message With Suggestion",
        None,
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "commit.discard_suggestion",
        "Discard Suggestion",
        None,
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "hook.cancel",
        "Cancel Hook Run",
        None,
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "hook.approve",
        "Approve Repository Hook",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "hook.deny",
        "Deny Repository Hook",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "remote.fetch",
        "Fetch",
        None,
        ArgKind::None,
        Category::Remote,
    ),
    spec("remote.pull", "Pull", None, ArgKind::None, Category::Remote),
];

const fn spec(
    id: &'static str,
    title: &'static str,
    shortcut: Option<&'static str>,
    args: ArgKind,
    category: Category,
) -> Spec {
    Spec {
        id,
        title,
        shortcut,
        args,
        category,
    }
}

/// Every action the window knows. Placeholders stay in the list so that menus
/// and the palette can show them disabled until their slice lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    StageFile,
    UnstageFile,
    StageChunk,
    UnstageChunk,
    DiscardChunk,
    Commit,
    Refresh,
    CheckoutBranch,
    CopyBranchName,
    CopyFilePath,
    FocusCommitMessage,
    NewChildBranch,
    Restack,
    PushWithLease,
    RenameBranch,
    DeleteBranch,
    CreateWorktree,
    DiscardFile,
    RevealFile,
    SuggestMessage,
    UseSuggestion,
    DiscardSuggestion,
    CancelHook,
    ApproveHook,
    DenyHook,
    Fetch,
    Pull,
}

/// The reason of an action the engine cannot run yet, whatever its target.
pub(crate) const NOT_AVAILABLE: Reason = "Not available yet";

impl Action {
    pub const ALL: [Action; 27] = [
        Action::StageFile,
        Action::UnstageFile,
        Action::StageChunk,
        Action::UnstageChunk,
        Action::DiscardChunk,
        Action::Commit,
        Action::Refresh,
        Action::CheckoutBranch,
        Action::CopyBranchName,
        Action::CopyFilePath,
        Action::FocusCommitMessage,
        Action::NewChildBranch,
        Action::Restack,
        Action::PushWithLease,
        Action::RenameBranch,
        Action::DeleteBranch,
        Action::CreateWorktree,
        Action::DiscardFile,
        Action::RevealFile,
        Action::SuggestMessage,
        Action::UseSuggestion,
        Action::DiscardSuggestion,
        Action::CancelHook,
        Action::ApproveHook,
        Action::DenyHook,
        Action::Fetch,
        Action::Pull,
    ];

    #[must_use]
    pub fn spec(self) -> Spec {
        SPECS[self as usize]
    }

    #[must_use]
    pub fn id(self) -> &'static str {
        self.spec().id
    }

    /// Whether the engine can run the action at all yet. A placeholder is
    /// listed disabled with `NOT_AVAILABLE` wherever it appears.
    #[must_use]
    pub fn available(self) -> bool {
        !matches!(
            self,
            Action::NewChildBranch
                | Action::Restack
                | Action::PushWithLease
                | Action::RenameBranch
                | Action::DeleteBranch
                | Action::CreateWorktree
                | Action::DiscardFile
                | Action::Fetch
                | Action::Pull
        )
    }

    /// Whether the action throws away work. Menus show it in red and ask first.
    #[must_use]
    pub fn destructive(self) -> bool {
        matches!(
            self,
            Action::DiscardChunk | Action::DiscardFile | Action::DeleteBranch
        )
    }

    /// The action with this id, if any.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.id() == id)
    }

    /// Whether the action can run now with `args`; the reason when it cannot.
    pub fn enabled(self, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
        if !self.takes(args) {
            return Err("No target for this action");
        }
        if !self.available() {
            return Err(NOT_AVAILABLE);
        }
        match self {
            Action::StageFile => {
                idle(ctx)?;
                let entry = file_entry(args, ctx)?;
                if has_unstaged(entry) {
                    Ok(())
                } else {
                    Err("Nothing unstaged in this file")
                }
            }
            Action::UnstageFile => {
                idle(ctx)?;
                let entry = file_entry(args, ctx)?;
                if entry.staged.is_some() {
                    Ok(())
                } else {
                    Err("Nothing staged in this file")
                }
            }
            Action::StageChunk => {
                idle(ctx)?;
                match args {
                    Args::Chunk {
                        side: DiffSide::Unstaged,
                        ..
                    } => Ok(()),
                    _ => Err("This chunk is already staged"),
                }
            }
            Action::UnstageChunk => {
                idle(ctx)?;
                match args {
                    Args::Chunk {
                        side: DiffSide::Staged,
                        ..
                    } => Ok(()),
                    _ => Err("This chunk is not staged"),
                }
            }
            Action::DiscardChunk => {
                idle(ctx)?;
                match args {
                    Args::Chunk {
                        side: DiffSide::Unstaged,
                        ..
                    } => Ok(()),
                    _ => Err("Only unstaged chunks can be discarded"),
                }
            }
            Action::Commit => {
                idle(ctx)?;
                if ctx.message.trim().is_empty() {
                    Err("Nothing to commit: the message is empty.")
                } else if ctx.staged_count == 0 {
                    Err("Nothing is staged.")
                } else {
                    Ok(())
                }
            }
            Action::Refresh => {
                if ctx.repo_open {
                    Ok(())
                } else {
                    Err("No repository is open.")
                }
            }
            Action::CheckoutBranch => {
                idle(ctx)?;
                match args {
                    Args::Branch(name) if ctx.current_branch == Some(name.as_str()) => {
                        Err("This is the current branch")
                    }
                    _ => Ok(()),
                }
            }
            Action::RevealFile => {
                if ctx.root.is_none() {
                    return Err("No repository is open.");
                }
                file_entry(args, ctx).map(|_| ())
            }
            Action::CopyBranchName | Action::CopyFilePath | Action::FocusCommitMessage => Ok(()),
            Action::SuggestMessage
            | Action::CancelHook
            | Action::UseSuggestion
            | Action::DiscardSuggestion
            | Action::ApproveHook
            | Action::DenyHook => hook_rule(self, ctx),
            // The placeholders, refused above by `available`.
            _ => Err(NOT_AVAILABLE),
        }
    }

    /// Runs the rule, then says what the run does. A refused run gives the reason.
    pub fn plan(self, args: &Args, ctx: &Context<'_>) -> Result<Effect, Reason> {
        self.enabled(args, ctx)?;
        Ok(match (self, args) {
            (Action::StageFile, Args::File(path)) => Effect::Send(vec![CommandKind::Stage {
                target: StageTarget::File { path: path.clone() },
            }]),
            (Action::UnstageFile, Args::File(path)) => Effect::Send(vec![CommandKind::Unstage {
                target: StageTarget::File { path: path.clone() },
            }]),
            (Action::StageChunk, Args::Chunk { path, hunk, .. }) => {
                Effect::Send(vec![CommandKind::Stage {
                    target: StageTarget::Hunk {
                        path: path.clone(),
                        hunk: hunk.clone(),
                    },
                }])
            }
            (Action::UnstageChunk, Args::Chunk { path, hunk, .. }) => {
                Effect::Send(vec![CommandKind::Unstage {
                    target: StageTarget::Hunk {
                        path: path.clone(),
                        hunk: hunk.clone(),
                    },
                }])
            }
            (Action::DiscardChunk, Args::Chunk { path, hunk, .. }) => {
                Effect::Send(vec![CommandKind::DiscardHunk {
                    path: path.clone(),
                    hunk: hunk.clone(),
                }])
            }
            (Action::Commit, _) => Effect::Send(vec![CommandKind::Commit {
                message: ctx.message.to_owned(),
            }]),
            (Action::Refresh, _) => Effect::Send(vec![CommandKind::RefreshStatus]),
            (Action::CheckoutBranch, Args::Branch(name)) => {
                Effect::Send(vec![CommandKind::Checkout {
                    branch: name.clone(),
                }])
            }
            (Action::CopyBranchName, Args::Branch(name)) => Effect::Copy(name.clone()),
            (Action::CopyFilePath, Args::File(path)) => Effect::Copy(path.clone()),
            (Action::RevealFile, Args::File(path)) => {
                // `enabled` has checked that the root is there.
                Effect::Reveal(PathBuf::from(ctx.root.unwrap_or_default()).join(path))
            }
            (Action::FocusCommitMessage, _) => Effect::FocusCommitMessage,
            (Action::SuggestMessage, _) => Effect::Send(vec![CommandKind::RunPrepareCommit {
                message: ctx.message.to_owned(),
            }]),
            (Action::CancelHook, _) => match ctx.hooks.running {
                Some(run) => Effect::Send(vec![CommandKind::CancelHook { run }]),
                None => return Err("No hook is running."),
            },
            (Action::UseSuggestion, _) => match ctx.hooks.suggestion {
                Some(text) => Effect::UseSuggestion(text.to_owned()),
                None => return Err("There is no suggestion."),
            },
            (Action::DiscardSuggestion, _) => Effect::DiscardSuggestion,
            (Action::ApproveHook, _) => match ctx.hooks.pending_approval {
                Some((name, hash)) => Effect::Send(vec![CommandKind::ApproveHook {
                    name: name.to_owned(),
                    command_hash: hash.to_owned(),
                }]),
                None => return Err("No repository hook waits for approval."),
            },
            (Action::DenyHook, _) => match ctx.hooks.pending_approval {
                Some((_, hash)) => Effect::DenyHook(hash.to_owned()),
                None => return Err("No repository hook waits for approval."),
            },
            // Only reached when `enabled` said Ok, which no placeholder does.
            _ => return Err(NOT_AVAILABLE),
        })
    }

    /// Whether `args` is the kind of target this action takes.
    fn takes(self, args: &Args) -> bool {
        matches!(
            (self.spec().args, args),
            (ArgKind::None, Args::None)
                | (ArgKind::Branch, Args::Branch(_))
                | (ArgKind::File, Args::File(_))
                | (ArgKind::Chunk, Args::Chunk { .. })
        )
    }
}

/// The rules of the prepare-commit actions: what the window's hook state allows.
fn hook_rule(action: Action, ctx: &Context<'_>) -> Result<(), Reason> {
    match action {
        Action::SuggestMessage => {
            idle(ctx)?;
            if ctx.hooks.running.is_some() {
                return Err("A hook is already running.");
            }
            match ctx.hooks.applicable {
                None => Err("Hooks are still being read."),
                Some(0) => Err("No prepare-commit hook applies to this repository."),
                Some(_) => Ok(()),
            }
        }
        Action::CancelHook => ctx.hooks.running.map(|_| ()).ok_or("No hook is running."),
        Action::UseSuggestion | Action::DiscardSuggestion => ctx
            .hooks
            .suggestion
            .map(|_| ())
            .ok_or("There is no suggestion."),
        Action::ApproveHook | Action::DenyHook => ctx
            .hooks
            .pending_approval
            .map(|_| ())
            .ok_or("No repository hook waits for approval."),
        _ => Err(NOT_AVAILABLE),
    }
}

/// The action the Space key runs on a file: stage what is unstaged, otherwise
/// unstage. Its enabled rule is the one of that action.
#[must_use]
pub fn toggle_file_action(entry: &StatusEntry) -> Action {
    if has_unstaged(entry) {
        Action::StageFile
    } else {
        Action::UnstageFile
    }
}

/// Whether the working tree differs from the index for this entry. An
/// untracked file counts.
#[must_use]
pub fn has_unstaged(entry: &StatusEntry) -> bool {
    entry.unstaged.is_some() || entry.untracked || entry.conflicted
}

/// Refuses while an open or a checkout is in flight.
fn idle(ctx: &Context<'_>) -> Result<(), Reason> {
    if !ctx.repo_open {
        return Err("No repository is open.");
    }
    ctx.busy.map_or(Ok(()), Err)
}

fn file_entry<'a>(args: &Args, ctx: &Context<'a>) -> Result<&'a StatusEntry, Reason> {
    let Args::File(path) = args else {
        return Err("No target for this action");
    };
    ctx.entries
        .iter()
        .find(|entry| entry.path == *path)
        .ok_or("That file is not in the working copy")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use gm_schema::Change;

    use super::*;

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
        }
    }

    fn untracked(path: &str) -> StatusEntry {
        StatusEntry {
            untracked: true,
            ..entry(path, None, None)
        }
    }

    fn ctx<'a>(entries: &'a [StatusEntry], message: &'a str) -> Context<'a> {
        Context {
            repo_open: true,
            busy: None,
            entries,
            staged_count: entries.iter().filter(|e| e.staged.is_some()).count(),
            message,
            current_branch: Some("main"),
            root: Some("/repo"),
            hooks: crate::action_registry::HookCtx::default(),
        }
    }

    fn chunk(side: DiffSide) -> Args {
        Args::Chunk {
            path: "a.txt".into(),
            hunk: HunkId("h1".into()),
            side,
        }
    }

    #[test]
    fn ids_are_unique_and_every_action_has_a_title() {
        let ids: HashSet<&str> = Action::ALL.iter().map(|a| a.id()).collect();
        assert_eq!(ids.len(), Action::ALL.len());
        for action in Action::ALL {
            assert!(!action.spec().title.is_empty());
            assert_eq!(Action::from_id(action.id()), Some(action));
        }
        assert_eq!(Action::from_id("nope"), None);
    }

    #[test]
    fn the_spec_table_follows_the_order_of_the_variants() {
        for (index, action) in Action::ALL.into_iter().enumerate() {
            assert_eq!(action as usize, index);
        }
    }

    #[test]
    fn ids_are_dotted_lowercase_and_stable() {
        assert_eq!(Action::StageFile.id(), "file.stage");
        assert_eq!(Action::DiscardChunk.id(), "chunk.discard");
        assert_eq!(Action::Commit.id(), "commit.create");
        assert_eq!(Action::Refresh.id(), "repo.refresh");
        assert_eq!(Action::CheckoutBranch.id(), "branch.checkout");
    }

    #[test]
    fn placeholders_are_disabled_with_the_same_reason_whatever_the_target() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let context = ctx(&entries, "msg");
        let placeholders = [
            Action::NewChildBranch,
            Action::Restack,
            Action::PushWithLease,
            Action::RenameBranch,
            Action::DeleteBranch,
            Action::CreateWorktree,
            Action::DiscardFile,
            Action::Fetch,
            Action::Pull,
        ];
        for action in placeholders {
            let args = match action.spec().args {
                ArgKind::Branch => Args::Branch("main".into()),
                ArgKind::File => Args::File("a.txt".into()),
                _ => Args::None,
            };
            assert_eq!(action.enabled(&args, &context), Err("Not available yet"));
            assert_eq!(action.plan(&args, &context), Err("Not available yet"));
        }
    }

    #[test]
    fn available_is_false_exactly_for_the_placeholders_that_refuse_every_target() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let context = ctx(&entries, "msg");
        for action in Action::ALL {
            let args = match action.spec().args {
                ArgKind::Branch => Args::Branch("main".into()),
                ArgKind::File => Args::File("a.txt".into()),
                _ => Args::None,
            };
            let refused = action.enabled(&args, &context) == Err(NOT_AVAILABLE);
            assert_eq!(action.available(), !refused, "{action:?}");
        }
    }

    #[test]
    fn a_run_with_the_wrong_kind_of_target_is_refused() {
        let context = ctx(&[], "");
        assert_eq!(
            Action::Refresh.enabled(&Args::File("a".into()), &context),
            Err("No target for this action")
        );
        assert_eq!(
            Action::StageFile.enabled(&Args::None, &context),
            Err("No target for this action")
        );
    }

    #[test]
    fn stage_file_needs_an_unstaged_side_and_unstage_needs_a_staged_one() {
        let entries = [
            entry("staged.txt", Some(Change::Added), None),
            entry("both.txt", Some(Change::Modified), Some(Change::Modified)),
            untracked("new.txt"),
        ];
        let context = ctx(&entries, "");
        let file = |p: &str| Args::File(p.into());
        assert_eq!(
            Action::StageFile.enabled(&file("staged.txt"), &context),
            Err("Nothing unstaged in this file")
        );
        assert_eq!(
            Action::StageFile.enabled(&file("both.txt"), &context),
            Ok(())
        );
        assert_eq!(
            Action::StageFile.enabled(&file("new.txt"), &context),
            Ok(())
        );
        assert_eq!(
            Action::UnstageFile.enabled(&file("new.txt"), &context),
            Err("Nothing staged in this file")
        );
        assert_eq!(
            Action::UnstageFile.enabled(&file("both.txt"), &context),
            Ok(())
        );
        assert_eq!(
            Action::StageFile.enabled(&file("gone.txt"), &context),
            Err("That file is not in the working copy")
        );
    }

    #[test]
    fn stage_file_plans_the_engine_command_for_the_path() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let effect = Action::StageFile
            .plan(&Args::File("a.txt".into()), &ctx(&entries, ""))
            .unwrap();
        assert_eq!(
            effect,
            Effect::Send(vec![CommandKind::Stage {
                target: StageTarget::File {
                    path: "a.txt".into()
                }
            }])
        );
    }

    #[test]
    fn space_stages_what_is_unstaged_and_otherwise_unstages() {
        assert_eq!(
            toggle_file_action(&entry("a", Some(Change::Modified), Some(Change::Modified))),
            Action::StageFile
        );
        assert_eq!(
            toggle_file_action(&entry("a", Some(Change::Added), None)),
            Action::UnstageFile
        );
        assert_eq!(toggle_file_action(&untracked("a")), Action::StageFile);
    }

    #[test]
    fn commit_says_why_it_cannot_run() {
        let entries = [entry("a.txt", Some(Change::Added), None)];
        assert_eq!(
            Action::Commit.enabled(&Args::None, &ctx(&entries, "  ")),
            Err("Nothing to commit: the message is empty.")
        );
        assert_eq!(
            Action::Commit.enabled(&Args::None, &ctx(&[], "subject")),
            Err("Nothing is staged.")
        );
        assert_eq!(
            Action::Commit.enabled(&Args::None, &ctx(&entries, "subject")),
            Ok(())
        );
    }

    #[test]
    fn commit_sends_the_message_as_typed() {
        let entries = [entry("a.txt", Some(Change::Added), None)];
        let effect = Action::Commit
            .plan(&Args::None, &ctx(&entries, "Fix it\n\nBody"))
            .unwrap();
        assert_eq!(
            effect,
            Effect::Send(vec![CommandKind::Commit {
                message: "Fix it\n\nBody".into()
            }])
        );
    }

    #[test]
    fn a_busy_repository_refuses_changes_but_not_refresh_or_copy() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let busy = Context {
            busy: Some("Wait for the branch checkout to finish."),
            ..ctx(&entries, "msg")
        };
        let file = Args::File("a.txt".into());
        assert_eq!(
            Action::StageFile.enabled(&file, &busy),
            Err("Wait for the branch checkout to finish.")
        );
        assert_eq!(Action::Refresh.enabled(&Args::None, &busy), Ok(()));
        assert_eq!(
            Action::CopyFilePath.plan(&file, &busy),
            Ok(Effect::Copy("a.txt".into()))
        );
        assert_eq!(
            Action::FocusCommitMessage.plan(&Args::None, &busy),
            Ok(Effect::FocusCommitMessage)
        );
    }

    #[test]
    fn no_repository_refuses_refresh_and_changes() {
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::Refresh.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::Commit.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn stage_and_unstage_chunk_follow_the_side_they_are_shown_on() {
        let context = ctx(&[], "");
        assert_eq!(
            Action::StageChunk.enabled(&chunk(DiffSide::Unstaged), &context),
            Ok(())
        );
        assert_eq!(
            Action::StageChunk.enabled(&chunk(DiffSide::Staged), &context),
            Err("This chunk is already staged")
        );
        assert_eq!(
            Action::UnstageChunk.enabled(&chunk(DiffSide::Staged), &context),
            Ok(())
        );
        assert_eq!(
            Action::UnstageChunk.enabled(&chunk(DiffSide::Unstaged), &context),
            Err("This chunk is not staged")
        );
        assert_eq!(
            Action::StageChunk.plan(&chunk(DiffSide::Unstaged), &context),
            Ok(Effect::Send(vec![CommandKind::Stage {
                target: StageTarget::Hunk {
                    path: "a.txt".into(),
                    hunk: HunkId("h1".into()),
                }
            }]))
        );
    }

    #[test]
    fn discard_chunk_is_only_for_unstaged_chunks_and_sends_the_discard() {
        let context = ctx(&[], "");
        assert_eq!(
            Action::DiscardChunk.enabled(&chunk(DiffSide::Staged), &context),
            Err("Only unstaged chunks can be discarded")
        );
        assert_eq!(
            Action::DiscardChunk.plan(&chunk(DiffSide::Unstaged), &context),
            Ok(Effect::Send(vec![CommandKind::DiscardHunk {
                path: "a.txt".into(),
                hunk: HunkId("h1".into()),
            }]))
        );
    }

    #[test]
    fn reveal_needs_a_repository_and_a_listed_file() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let file = Args::File("a.txt".into());
        assert_eq!(
            Action::RevealFile.plan(&file, &ctx(&entries, "")),
            Ok(Effect::Reveal(PathBuf::from("/repo/a.txt")))
        );
        let closed = Context {
            repo_open: false,
            root: None,
            ..ctx(&entries, "")
        };
        assert_eq!(
            Action::RevealFile.enabled(&file, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::RevealFile.enabled(&Args::File("gone.txt".into()), &ctx(&entries, "")),
            Err("That file is not in the working copy")
        );
    }

    #[test]
    fn only_throwing_work_away_is_destructive() {
        let destructive: Vec<Action> = Action::ALL
            .into_iter()
            .filter(|action| action.destructive())
            .collect();
        assert_eq!(
            destructive,
            [
                Action::DiscardChunk,
                Action::DeleteBranch,
                Action::DiscardFile
            ]
        );
    }

    #[test]
    fn checkout_refuses_the_current_branch_and_sends_the_others() {
        let context = ctx(&[], "");
        let branch = |name: &str| Args::Branch(name.into());
        assert_eq!(
            Action::CheckoutBranch.enabled(&branch("main"), &context),
            Err("This is the current branch")
        );
        assert_eq!(
            Action::CheckoutBranch.plan(&branch("feature"), &context),
            Ok(Effect::Send(vec![CommandKind::Checkout {
                branch: "feature".into()
            }]))
        );
        assert_eq!(
            Action::CopyBranchName.plan(&branch("feature"), &context),
            Ok(Effect::Copy("feature".into()))
        );
    }

    fn with_hooks(hooks: HookCtx<'_>) -> Context<'_> {
        Context {
            hooks,
            ..ctx(&[], "draft")
        }
    }

    #[test]
    fn suggesting_waits_for_the_listing_and_needs_a_hook_that_applies() {
        let none = HookCtx::default();
        assert_eq!(
            Action::SuggestMessage.enabled(&Args::None, &with_hooks(none)),
            Err("Hooks are still being read.")
        );
        let nothing = HookCtx {
            applicable: Some(0),
            ..HookCtx::default()
        };
        assert_eq!(
            Action::SuggestMessage.enabled(&Args::None, &with_hooks(nothing)),
            Err("No prepare-commit hook applies to this repository.")
        );
        let one = HookCtx {
            applicable: Some(1),
            ..HookCtx::default()
        };
        assert_eq!(
            Action::SuggestMessage.plan(&Args::None, &with_hooks(one)),
            Ok(Effect::Send(vec![CommandKind::RunPrepareCommit {
                message: "draft".into(),
            }]))
        );
    }

    #[test]
    fn a_second_run_is_refused_and_cancel_names_the_running_one() {
        let running = HookCtx {
            running: Some(OperationId(5)),
            applicable: Some(1),
            ..HookCtx::default()
        };
        assert_eq!(
            Action::SuggestMessage.enabled(&Args::None, &with_hooks(running)),
            Err("A hook is already running.")
        );
        assert_eq!(
            Action::CancelHook.plan(&Args::None, &with_hooks(running)),
            Ok(Effect::Send(vec![CommandKind::CancelHook {
                run: OperationId(5),
            }]))
        );
        assert_eq!(
            Action::CancelHook.enabled(&Args::None, &with_hooks(HookCtx::default())),
            Err("No hook is running.")
        );
    }

    #[test]
    fn a_suggestion_is_used_or_dropped_and_needs_to_exist() {
        let offered = HookCtx {
            suggestion: Some("feat: x"),
            ..HookCtx::default()
        };
        assert_eq!(
            Action::UseSuggestion.plan(&Args::None, &with_hooks(offered)),
            Ok(Effect::UseSuggestion("feat: x".into()))
        );
        assert_eq!(
            Action::DiscardSuggestion.plan(&Args::None, &with_hooks(offered)),
            Ok(Effect::DiscardSuggestion)
        );
        assert_eq!(
            Action::UseSuggestion.enabled(&Args::None, &with_hooks(HookCtx::default())),
            Err("There is no suggestion.")
        );
    }

    #[test]
    fn approval_sends_the_listed_hash_and_deny_stays_local() {
        let waiting = HookCtx {
            pending_approval: Some(("team", "abc")),
            ..HookCtx::default()
        };
        assert_eq!(
            Action::ApproveHook.plan(&Args::None, &with_hooks(waiting)),
            Ok(Effect::Send(vec![CommandKind::ApproveHook {
                name: "team".into(),
                command_hash: "abc".into(),
            }]))
        );
        assert_eq!(
            Action::DenyHook.plan(&Args::None, &with_hooks(waiting)),
            Ok(Effect::DenyHook("abc".into()))
        );
        assert_eq!(
            Action::ApproveHook.enabled(&Args::None, &with_hooks(HookCtx::default())),
            Err("No repository hook waits for approval.")
        );
    }
}

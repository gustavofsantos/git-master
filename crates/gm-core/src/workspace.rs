//! Workspaces (docs/reference/cli.md, "Workspaces"; docs/guide/workspaces.md; D174, D177, D178,
//! D180, D183 (1)): a folder `<root>/<name>` holding a Git repository whose members are submodules,
//! each a linked worktree of a clone the user already has. The workspace is found from its own
//! repository's `gitmaster.workspace` key, and the members from its `.gitmodules`; there is no
//! store of its own (D177 (1), D186 (2)).
//!
//! The functions return data and write nothing to a terminal: `gitmaster workspace` prints the
//! results, and the engine answers its commands with them. The member worktrees go through
//! `GitPort`, as the worktree commands do; the superproject's Git calls run as subprocesses
//! (D177 (5)), with the global configuration given to `Workspaces::new`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use gm_schema::{
    BranchEntry, BranchHow, MemberPlan, MemberRequest, RemovalBlocker, RemovalBlockerKind,
    StatusEntry, WorkspaceProblem, WorkspaceRow, WorktreeEntry, WorktreeMode,
};
use serde::Serialize;

use crate::worktree_cli;
use crate::worktree_include::Mode;
use crate::{GitError, GitPort, workspace_pointer};

/// The Git key that marks a repository as a workspace: `true` (D186 (2)).
const MARKER_KEY: &str = "gitmaster.workspace";
/// The key workspaces made before D186 carry, with `durable` or `ephemeral` as its value. It still
/// marks a workspace; nothing rewrites it.
const OLD_KIND_KEY: &str = "gitmaster.workspace.kind";
/// The Git key that sets the folder workspaces are created under.
const ROOT_KEY: &str = "gitmaster.workspace.root";
/// The mode Git records for a submodule: a gitlink.
const GITLINK: &str = "160000";

/// Why a workspace command failed. `Usage` is a request the caller can correct (a wrong flag
/// value); `Run` is a failure of the work, with the reason for a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Usage(String),
    Run(String),
}

impl Error {
    /// The reason, without its kind.
    pub fn message(&self) -> &str {
        match self {
            Self::Usage(message) | Self::Run(message) => message,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl From<GitError> for Error {
    fn from(error: GitError) -> Self {
        Self::Run(error.0)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Run(error.to_string())
    }
}

impl From<worktree_cli::Failure> for Error {
    fn from(failure: worktree_cli::Failure) -> Self {
        match failure {
            worktree_cli::Failure::Usage(message) => Self::Usage(message),
            worktree_cli::Failure::Run(message) => Self::Run(message),
        }
    }
}

/// Whether `name` can name a workspace or a member: one folder name that does not start with `.`
/// or `-`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.starts_with(['.', '-'])
}

/// A branch name `git worktree add` would accept.
pub fn check_branch(branch: &str) -> Result<(), Error> {
    if branch.is_empty() || branch.starts_with('-') || !worktree_cli::valid_branch_name(branch) {
        return Err(Error::Run(format!("'{branch}' is not a valid branch name")));
    }
    Ok(())
}

/// Fails when `dir` exists: a workspace is created in a folder that is not there yet.
pub fn ensure_absent(dir: &Path) -> Result<(), Error> {
    if dir.exists() {
        return Err(Error::Run(format!("'{}' already exists", dir.display())));
    }
    Ok(())
}

/// The root the workspaces live under, from the first of `--root`, the configured value and
/// `~/workspaces`. A relative root is taken from `cwd`, and a leading `~` is the home folder.
pub fn root_from(
    flag: Option<&str>,
    configured: Option<&str>,
    home: Option<&Path>,
    cwd: &Path,
) -> Result<PathBuf, Error> {
    let chosen = match (flag, configured) {
        (Some(flag), _) => worktree_cli::expand(flag, home),
        (None, Some(value)) => worktree_cli::expand(value, home),
        (None, None) => home
            .ok_or_else(|| Error::Run("no home folder: give --root".into()))?
            .join("workspaces"),
    };
    Ok(cwd.join(chosen))
}

/// A member as the workspace's `.gitmodules` records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// The folder name, relative to the workspace.
    pub path: String,
    /// The path of the main checkout the member belongs to.
    pub url: String,
    /// The branch the member follows, from `submodule.<name>.branch`.
    pub branch: Option<String>,
}

/// One member in a status listing. A member whose folder is gone has `branch` `(missing)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemberState {
    pub member: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub dirty: bool,
    pub new_commits: usize,
    pub recorded: Option<String>,
    pub head: Option<String>,
    pub repo: String,
    /// The member's folder is gone. Not part of the JSON listing.
    #[serde(skip)]
    pub missing: bool,
}

impl MemberState {
    /// The branch column of the `tsv` format.
    pub fn branch_label(&self) -> &str {
        match (&self.branch, self.detached) {
            (Some(branch), _) => branch,
            (None, true) => "(detached)",
            (None, false) => "(missing)",
        }
    }
}

/// What a new workspace holds: its name and members, and the rules D178 gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateRequest {
    pub name: String,
    /// The root the folder `<root>/<name>` goes under.
    pub root: PathBuf,
    pub members: Vec<MemberRequest>,
    /// The `--base` rev that a new branch starts from, in every member.
    pub base: Option<String>,
    /// Only the members whose repository already has the branch (`--existing`); the others are
    /// skipped with a note.
    pub existing: bool,
}

/// One member to add to a workspace that exists (`gitmaster workspace add`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddRequest {
    /// A folder of the repository.
    pub repo: String,
    pub branch: String,
    pub base: Option<String>,
    /// The folder name of the member, the repository's own when `None`.
    pub member: Option<String>,
    /// The `--include` mode of `.worktreeinclude`, from the configuration when `None`.
    pub include: Option<String>,
}

/// Where a member's branch comes from (D178 (2)).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    /// The local branch, checked out.
    Existing,
    /// A new local branch tracking the remote branch `remote`.
    Track(String),
    /// A new local branch from `base`.
    From(String),
}

impl Source {
    fn mode(&self, branch: &str) -> WorktreeMode {
        match self {
            Self::Existing => WorktreeMode::Existing {
                branch: branch.to_owned(),
            },
            Self::Track(base) | Self::From(base) => WorktreeMode::New {
                branch: branch.to_owned(),
                base: base.clone(),
            },
        }
    }

    fn branch_how(&self) -> BranchHow {
        match self {
            Self::Existing => BranchHow::CheckOut,
            Self::Track(remote) => BranchHow::TrackRemote {
                remote: remote.clone(),
            },
            Self::From(base) => BranchHow::CreateFrom { base: base.clone() },
        }
    }

    /// What the report says the branch was, given whether a `--base` was passed. A base is
    /// ignored for a local branch and for a remote one (D178 (6)).
    fn how(&self, base_given: bool) -> String {
        let ignored = if base_given && !matches!(self, Self::From(_)) {
            "; the base is ignored"
        } else {
            ""
        };
        match self {
            Self::Existing => format!("checked out the existing branch{ignored}"),
            Self::Track(remote) => format!("created to track {remote}{ignored}"),
            Self::From(base) => format!("created from {base}"),
        }
    }
}

/// A member that can be added, checked and decided before anything is created (D178 (3)).
#[derive(Clone, Debug)]
struct Plan {
    /// The folder name of the member, relative to the workspace.
    member: String,
    /// The member's folder, `<workspace>/<member>`.
    dir: PathBuf,
    /// The path of the main checkout the member belongs to.
    main: PathBuf,
    /// The folder the repository's Git commands run in.
    root: PathBuf,
    branch: String,
    mode: WorktreeMode,
    branch_how: BranchHow,
    /// The report's words for the branch, after `added <dir> on <branch>:`.
    how: String,
    include: Mode,
}

/// The plan of a new workspace: each member that can be added, the problems that refuse it, and
/// the notes of the members left out. Nothing is written by planning. `create` takes the plan.
#[derive(Debug)]
pub struct CreatePlan {
    /// The folder the workspace would have.
    pub path: PathBuf,
    /// The members that can be added, in the order they were asked for.
    pub members: Vec<MemberPlan>,
    /// Why the workspace cannot be created as planned. Any problem refuses the whole creation.
    pub problems: Vec<WorkspaceProblem>,
    /// Lines for the standard error, in order: the members left out by `--existing`.
    pub notes: Vec<String>,
    plans: Vec<Plan>,
}

/// The result of creating a workspace. `added` has one entry per member; a member that failed
/// has a `failure`, and the workspace itself exists either way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub path: PathBuf,
    pub added: Vec<Added>,
}

/// One member as it was added, or why it was not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Added {
    pub member: String,
    /// The member's folder.
    pub dir: PathBuf,
    pub branch: String,
    /// Lines for the standard error, in order: notes about the member and, when it was added, the
    /// line that says so.
    pub notes: Vec<String>,
    /// Why the member was not added. The worktree is removed again when Git refused the
    /// registration; otherwise the member stays added and staged.
    pub failure: Option<String>,
}

/// What removing a workspace would refuse or lose (D31, D174 (3)). No blockers means it would
/// go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovalInspection {
    pub blockers: Vec<RemovalBlocker>,
}

impl RemovalInspection {
    /// Why the removal of the workspace `name` is refused, if it is: a blocker that cannot be
    /// overridden always refuses; the others refuse unless `force` is set.
    pub fn refusal(&self, name: &str, force: bool) -> Option<Error> {
        if let Some(blocker) = self.blockers.iter().find(|blocker| !blocker.overridable) {
            return Some(Error::Run(match (blocker.kind, &blocker.member) {
                (RemovalBlockerKind::Current, _) => {
                    "this directory is inside that workspace; run the command from another one"
                        .into()
                }
                (_, Some(member)) => format!("cannot remove {member}: {}", blocker.detail),
                (_, None) => blocker.detail.clone(),
            }));
        }
        let found: Vec<String> = self
            .blockers
            .iter()
            .map(|blocker| match &blocker.member {
                Some(member) => format!("{member}: {}", blocker.detail),
                None => blocker.detail.clone(),
            })
            .collect();
        if found.is_empty() || force {
            return None;
        }
        Some(Error::Run(format!(
            "not removing {name}: {}; use --force to discard them",
            found.join("; ")
        )))
    }
}

/// The result of removing a workspace: the lines for the standard error, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removed {
    pub path: PathBuf,
    pub notes: Vec<String>,
}

/// Why one member cannot be added: the reason, and the folder of the worktree that holds its
/// branch, when another worktree does.
struct Refusal {
    error: Error,
    path: Option<String>,
}

impl From<Error> for Refusal {
    fn from(error: Error) -> Self {
        Self { error, path: None }
    }
}

impl From<GitError> for Refusal {
    fn from(error: GitError) -> Self {
        Self::from(Error::from(error))
    }
}

/// What `plan_member` is asked: the branch and the options of one member.
struct Choice<'a> {
    branch: &'a str,
    base: Option<&'a str>,
    member: Option<&'a str>,
    include: Option<&'a str>,
}

/// The workspace operations over `git`, the port for the member worktrees, and the superproject's
/// own Git calls as subprocesses (D177 (5)). `global_config`, when set, is the file Git reads as
/// the global configuration for those calls; tests use it to give commits an identity.
pub struct Workspaces<'a> {
    git: &'a dyn GitPort,
    sub: Sub,
}

/// The Git calls of the superproject and of a member's folder, as subprocesses.
struct Sub {
    global_config: Option<PathBuf>,
}

impl Sub {
    /// The standard output of `git args` run in `dir`; an error holds git's message.
    fn text(&self, dir: &Path, args: &[&str]) -> Result<String, Error> {
        let output = self
            .command(dir, args)
            .output()
            .map_err(|e| Error::Run(format!("cannot run git: {e}")))?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let why = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(Error::Run(if why.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            why
        }))
    }

    /// The standard output of `git args` run in `dir`, or `None` when git fails or cannot run.
    fn text_if(&self, dir: &Path, args: &[&str]) -> Option<String> {
        let output = self.command(dir, args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn command(&self, dir: &Path, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command.args(args).current_dir(dir);
        if let Some(global) = &self.global_config {
            command.env("GIT_CONFIG_GLOBAL", global);
        }
        command
    }
}

impl<'a> Workspaces<'a> {
    pub fn new(git: &'a dyn GitPort, global_config: Option<PathBuf>) -> Self {
        Self {
            git,
            sub: Sub { global_config },
        }
    }

    /// The root for a command: `flag` when given, else the configured `gitmaster.workspace.root`
    /// as Git reads it from `cwd` (the global configuration outside a repository), else
    /// `~/workspaces`. `cwd` is where a relative root is taken from.
    pub fn root(
        &self,
        flag: Option<&str>,
        cwd: &Path,
        home: Option<&Path>,
    ) -> Result<PathBuf, Error> {
        let configured = self.configured_root(cwd);
        root_from(flag, configured.as_deref(), home, cwd)
    }

    /// The value of `gitmaster.workspace.root` that applies to `dir`, when one is set.
    pub fn configured_root(&self, dir: &Path) -> Option<String> {
        self.sub
            .text_if(dir, &["config", "--includes", "--get", ROOT_KEY])
            .or_else(|| {
                self.sub.text_if(
                    dir,
                    &["config", "--global", "--includes", "--get", ROOT_KEY],
                )
            })
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }

    /// Whether `dir` is a workspace: a folder with its own `.git` folder whose repository has
    /// `gitmaster.workspace = true` in its local configuration, or the key
    /// `gitmaster.workspace.kind` that workspaces made before D186 carry, whatever its value
    /// (D186 (2)).
    pub fn is_workspace(&self, dir: &Path) -> bool {
        if !dir.join(".git").is_dir() {
            return false;
        }
        let marked = self
            .sub
            .text_if(dir, &["config", "--local", "--bool", "--get", MARKER_KEY])
            .is_some_and(|value| value.trim() == "true");
        marked
            || self
                .sub
                .text_if(dir, &["config", "--local", "--get", OLD_KIND_KEY])
                .is_some()
    }

    /// The workspace `name` under `root`, when there is one.
    pub fn named(&self, root: &Path, name: &str) -> Option<PathBuf> {
        if !valid_name(name) {
            return None;
        }
        let dir = root.join(name);
        self.is_workspace(&dir).then_some(dir)
    }

    /// The workspace that contains `cwd`, from its root or from inside a member.
    pub fn containing(&self, cwd: &Path) -> Option<PathBuf> {
        let start = canonical(cwd);
        start
            .ancestors()
            .find(|dir| self.is_workspace(dir))
            .map(Path::to_path_buf)
    }

    /// The workspaces directly under `root`, by name.
    pub fn list(&self, root: &Path) -> Result<Vec<WorkspaceRow>, Error> {
        let Ok(entries) = std::fs::read_dir(root) else {
            return Ok(Vec::new());
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        let mut rows = Vec::new();
        for dir in dirs {
            if !self.is_workspace(&dir) {
                continue;
            }
            let name = dir
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            rows.push(WorkspaceRow {
                name,
                path: dir.to_string_lossy().into_owned(),
                members: self.members(&dir)?.len(),
            });
        }
        Ok(rows)
    }

    /// The members of the workspace at `ws`, in the order of its `.gitmodules`.
    pub fn members(&self, ws: &Path) -> Result<Vec<Member>, Error> {
        if !ws.join(".gitmodules").is_file() {
            return Ok(Vec::new());
        }
        let text = self
            .sub
            .text(ws, &["config", "--file", ".gitmodules", "--list", "-z"])?;
        let mut found: Vec<(String, Member)> = Vec::new();
        for record in text.split('\0').filter(|record| !record.is_empty()) {
            let Some((key, value)) = record.split_once('\n') else {
                continue;
            };
            let Some((name, var)) = key
                .strip_prefix("submodule.")
                .and_then(|rest| rest.rsplit_once('.'))
            else {
                continue;
            };
            let index = found
                .iter()
                .position(|(known, _)| known == name)
                .unwrap_or_else(|| {
                    found.push((
                        name.to_owned(),
                        Member {
                            path: String::new(),
                            url: String::new(),
                            branch: None,
                        },
                    ));
                    found.len() - 1
                });
            let member = &mut found[index].1;
            match var {
                "path" => value.clone_into(&mut member.path),
                "url" => value.clone_into(&mut member.url),
                "branch" => member.branch = Some(value.to_owned()),
                _ => {}
            }
        }
        Ok(found
            .into_iter()
            .map(|(_, member)| member)
            .filter(|member| !member.path.is_empty())
            .collect())
    }

    /// The status of every member of the workspace at `ws`, in the order of `.gitmodules`.
    pub fn member_states(&self, ws: &Path) -> Result<Vec<MemberState>, Error> {
        let mut rows = Vec::new();
        for member in self.members(ws)? {
            rows.push(self.member_state(ws, &member)?);
        }
        Ok(rows)
    }

    /// The status of one member.
    fn member_state(&self, ws: &Path, member: &Member) -> Result<MemberState, Error> {
        let recorded = self.recorded(ws, &member.path);
        let mut row = MemberState {
            member: member.path.clone(),
            branch: None,
            detached: false,
            dirty: false,
            new_commits: 0,
            recorded: recorded.clone(),
            head: None,
            repo: member.url.clone(),
            missing: false,
        };
        let dir = ws.join(&member.path);
        if !dir.exists() {
            row.branch = Some("(missing)".into());
            row.missing = true;
            return Ok(row);
        }
        let state = self.git.status(&dir)?;
        row.branch.clone_from(&state.branch.head);
        row.detached = state.branch.head.is_none();
        row.head.clone_from(&state.branch.oid);
        row.dirty = state.entries.iter().any(is_change);
        let range = match &recorded {
            Some(sha) => format!("{sha}..HEAD"),
            None => "HEAD".to_owned(),
        };
        row.new_commits = self
            .sub
            .text_if(&dir, &["rev-list", "--count", &range])
            .and_then(|count| count.trim().parse().ok())
            .unwrap_or(0);
        Ok(row)
    }

    /// The commit the workspace's committed `HEAD` records for the member at `path`, when it has one.
    fn recorded(&self, ws: &Path, path: &str) -> Option<String> {
        let text = self
            .sub
            .text_if(ws, &["ls-tree", "-z", "HEAD", "--", path])?;
        let record = text.split('\0').next()?;
        let (meta, _) = record.split_once('\t')?;
        let mut fields = meta.split_whitespace();
        let (mode, _kind, sha) = (fields.next()?, fields.next()?, fields.next()?);
        (mode == GITLINK).then(|| sha.to_owned())
    }

    /// The paths the workspace's index records as gitlinks, that is its members.
    fn gitlink_paths(&self, ws: &Path) -> Result<Vec<String>, Error> {
        let text = self.sub.text(ws, &["ls-files", "-s", "-z"])?;
        Ok(text
            .split('\0')
            .filter_map(|record| {
                let (meta, path) = record.split_once('\t')?;
                let mode = meta.split_whitespace().next()?;
                (mode == GITLINK).then(|| path.to_owned())
            })
            .collect())
    }

    /// Plans the workspace of `request`: the branch of each member, and every problem that
    /// refuses it (D178 (1) and (3)). Nothing is written. A repository that cannot be read is a
    /// problem of its own, and so is a branch that is not valid; a Git failure that is not about
    /// one repository fails the plan.
    pub fn plan(&self, request: &CreateRequest) -> Result<CreatePlan, Error> {
        if !valid_name(&request.name) {
            return Err(Error::Run(format!(
                "'{}' is not a workspace name",
                request.name
            )));
        }
        let dir = request.root.join(&request.name);
        ensure_absent(&dir)?;
        let mut plans: Vec<Plan> = Vec::new();
        let mut problems: Vec<WorkspaceProblem> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for member in &request.members {
            let (root, entries) =
                match worktree_cli::worktrees_in(Path::new(&member.repo), self.git) {
                    Ok(found) => found,
                    Err(failure) => {
                        problems.push(WorkspaceProblem {
                            repo: Some(member.repo.clone()),
                            message: Error::from(failure).message().to_owned(),
                            path: None,
                        });
                        continue;
                    }
                };
            let main = entries[0].path.clone();
            let known = self.git.branches(&root)?;
            if request.existing && !has_branch(&known, &member.branch) {
                notes.push(format!(
                    "gitmaster: skipped {}: no branch '{}'",
                    project_of(&main),
                    member.branch
                ));
                continue;
            }
            let choice = Choice {
                branch: &member.branch,
                base: request.base.as_deref(),
                member: None,
                include: None,
            };
            match self.plan_member(&dir, &root, &entries, &known, &choice) {
                Ok(plan) => plans.push(plan),
                Err(refusal) => problems.push(WorkspaceProblem {
                    repo: Some(main),
                    message: refusal.error.message().to_owned(),
                    path: refusal.path,
                }),
            }
        }
        for (index, plan) in plans.iter().enumerate() {
            if let Some(other) = plans[..index]
                .iter()
                .find(|other| other.member == plan.member)
            {
                problems.push(WorkspaceProblem {
                    repo: None,
                    message: format!(
                        "{} and {} would both be the member '{}'",
                        other.main.display(),
                        plan.main.display(),
                        plan.member
                    ),
                    path: None,
                });
            }
        }
        let members = plans
            .iter()
            .map(|plan| MemberPlan {
                repo: plan.main.to_string_lossy().into_owned(),
                member: plan.member.clone(),
                branch: plan.branch.clone(),
                how: plan.branch_how.clone(),
            })
            .collect();
        Ok(CreatePlan {
            path: dir,
            members,
            problems,
            notes,
            plans,
        })
    }

    /// Creates the workspace of `plan`: its folder, its repository on `main`, its marker, and each
    /// member (D174, D177, D186 (2)). Refuses a plan with problems before anything is written. A member that
    /// fails is reported in the result and the workspace is kept, as the members before it are.
    pub fn create(&self, plan: CreatePlan) -> Result<Created, Error> {
        if !plan.problems.is_empty() {
            return Err(Error::Run(
                "no workspace was created: the plan has problems".into(),
            ));
        }
        let CreatePlan { path, plans, .. } = plan;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir(&path)?;
        self.sub.text(&path, &["init", "-q", "-b", "main"])?;
        self.sub
            .text(&path, &["config", "--local", MARKER_KEY, "true"])?;
        let added = plans
            .iter()
            .map(|plan| self.add_member(&path, plan))
            .collect();
        Ok(Created { path, added })
    }

    /// Adds one member to the workspace at `ws` (`gitmaster workspace add`). The result says
    /// whether it was added; the caller decides what a failure means.
    pub fn add(&self, ws: &Path, request: &AddRequest) -> Result<Added, Error> {
        let (root, entries) = worktree_cli::worktrees_in(Path::new(&request.repo), self.git)?;
        let known = self.git.branches(&root)?;
        let choice = Choice {
            branch: &request.branch,
            base: request.base.as_deref(),
            member: request.member.as_deref(),
            include: request.include.as_deref(),
        };
        let plan = self
            .plan_member(ws, &root, &entries, &known, &choice)
            .map_err(|refusal| refusal.error)?;
        Ok(self.add_member(ws, &plan))
    }

    /// Checks the member `choice` would add to the workspace at `ws`, which need not exist yet,
    /// and decides its branch. Nothing is created.
    fn plan_member(
        &self,
        ws: &Path,
        root: &Path,
        entries: &[WorktreeEntry],
        known: &[BranchEntry],
        choice: &Choice,
    ) -> Result<Plan, Refusal> {
        let branch = choice.branch;
        check_branch(branch)?;
        let main = PathBuf::from(&entries[0].path);
        let member = choice.member.map_or_else(
            || {
                main.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            },
            str::to_owned,
        );
        if !valid_name(&member) {
            return Err(Error::Run(format!("'{member}' is not a folder name for a member")).into());
        }
        if let Some(entry) = worktree_cli::holder(entries, branch) {
            return Err(Refusal {
                error: Error::Run(format!(
                    "branch '{branch}' is checked out at {}; a branch can be checked out in one worktree only",
                    entry.path
                )),
                path: Some(entry.path.clone()),
            });
        }
        let existing = self.members(ws)?;
        if existing.iter().any(|kept| kept.path == member) {
            return Err(Error::Run(format!("'{member}' is already a member")).into());
        }
        if let Some(kept) = existing.iter().find(|kept| same_folder(&kept.url, &main)) {
            return Err(Error::Run(format!(
                "{} is already a member, as '{}'",
                main.display(),
                kept.path
            ))
            .into());
        }
        let dir = ws.join(&member);
        ensure_absent(&dir)?;

        let include =
            worktree_cli::include_mode(self.git, root, choice.include).map_err(Error::from)?;
        let source = self.branch_source(root, &main, known, branch, choice.base)?;
        Ok(Plan {
            member,
            dir,
            main,
            root: root.to_path_buf(),
            branch: branch.to_owned(),
            mode: source.mode(branch),
            branch_how: source.branch_how(),
            how: source.how(choice.base.is_some()),
            include,
        })
    }

    /// Where a member's branch comes from, as D178 (2) chooses it: the local branch `branch` when
    /// there is one; else a new local branch that tracks the remote one, whatever the `base`; else
    /// a new branch from `base`, or from the repository's base branch, or from `HEAD` when there is
    /// none.
    fn branch_source(
        &self,
        root: &Path,
        main: &Path,
        known: &[BranchEntry],
        branch: &str,
        base: Option<&str>,
    ) -> Result<Source, Error> {
        if known.iter().any(|b| !b.is_remote && b.name == branch) {
            return Ok(Source::Existing);
        }
        if let Some(remote) = worktree_cli::remote_branch(known, branch) {
            return Ok(Source::Track(remote));
        }
        let from = match base {
            Some(base) => {
                self.check_commit(main, base)?;
                base.to_owned()
            }
            None => self
                .repo_base(root, main, known)?
                .unwrap_or_else(|| "HEAD".to_owned()),
        };
        Ok(Source::From(from))
    }

    /// The base branch of a repository, as `merge_base` picks it from `origin/HEAD` and the local
    /// branches (D56, D178 (2)). `None` when there is none.
    fn repo_base(
        &self,
        root: &Path,
        main: &Path,
        known: &[BranchEntry],
    ) -> Result<Option<String>, Error> {
        let origin = self.git.origin_head(root)?;
        let origin = origin
            .as_deref()
            .and_then(crate::branch::origin_head_branch);
        let locals: Vec<&str> = known
            .iter()
            .filter(|b| !b.is_remote)
            .map(|b| b.name.as_str())
            .collect();
        let current = self.git.status(main)?.branch.head;
        Ok(crate::branch::merge_base(
            origin,
            &locals,
            current.as_deref(),
        ))
    }

    /// Fails unless `rev` names a commit in the repository whose main checkout is `main`.
    fn check_commit(&self, main: &Path, rev: &str) -> Result<(), Error> {
        let spec = format!("{rev}^{{commit}}");
        if self
            .sub
            .text_if(main, &["rev-parse", "--verify", "--quiet", &spec])
            .is_none()
        {
            return Err(Error::Run(format!(
                "'{rev}' is not a commit in {}",
                main.display()
            )));
        }
        Ok(())
    }

    /// Makes the member of `plan` in the workspace at `ws`: its worktree, the registration of the
    /// worktree as a submodule and the commit of the workspace's repository (D177 (2) and (3)).
    /// When the commit fails the member stays added and staged, and the failure says so.
    fn add_member(&self, ws: &Path, plan: &Plan) -> Added {
        let dir = &plan.dir;
        let dir_text = dir.to_string_lossy().into_owned();
        let mut added = Added {
            member: plan.member.clone(),
            dir: dir.clone(),
            branch: plan.branch.clone(),
            notes: Vec::new(),
            failure: None,
        };
        if let Err(error) = self.git.create_worktree(&plan.root, &dir_text, &plan.mode) {
            added.failure = Some(error.0);
            return added;
        }
        let mut shared = Vec::new();
        worktree_cli::share(self.git, &plan.main, dir, plan.include, false, &mut shared);
        added
            .notes
            .extend(String::from_utf8_lossy(&shared).lines().map(str::to_owned));

        // Registers the existing worktree: Git adds it in place, without a clone (D174).
        let registered = self.sub.text(
            ws,
            &[
                "submodule",
                "add",
                "-b",
                &plan.branch,
                &plan.main.to_string_lossy(),
                &plan.member,
            ],
        );
        if let Err(error) = registered {
            let _ = self.git.remove_worktree(&plan.root, &dir_text, 1);
            added.failure = Some(error.message().to_owned());
            return added;
        }
        // The member's repository lists the workspace from now on (D180 (2)). A failure does not
        // undo the member, which is registered already; the note says what is missing.
        if let Err(error) = self.pointer_add(&plan.main, ws) {
            added.notes.push(format!(
                "gitmaster: {}: the workspace is not recorded in its repository: {error}",
                plan.main.display()
            ));
        }

        let message = format!("Add {} on {}", plan.member, plan.branch);
        if let Err(error) = self.sub.text(
            ws,
            &[
                "commit",
                "-q",
                "-m",
                &message,
                "--",
                ".gitmodules",
                &plan.member,
            ],
        ) {
            added.failure = Some(format!(
                "{} is added and staged in {}, but not committed: {}; commit it with git -C {} commit",
                plan.member,
                ws.display(),
                error.message(),
                ws.display()
            ));
            return added;
        }
        added.notes.push(format!(
            "gitmaster: added {} on {}: {}",
            dir.display(),
            plan.branch,
            plan.how
        ));
        added
    }

    /// Lists the workspace `ws` in the pointer file of the repository whose main checkout is `main`
    /// (D180 (2)). The file lives in the common directory, so no working tree is written.
    fn pointer_add(&self, main: &Path, ws: &Path) -> std::io::Result<()> {
        let common = self
            .git
            .common_dir(main)
            .map_err(|error| std::io::Error::other(error.0))?;
        // Canonical, as `remove` names it and as the lookup compares it.
        workspace_pointer::add(&common, &canonical(ws))
    }

    /// Takes the workspace `ws` out of the pointer file of the repository of `main` (D180 (2)).
    fn pointer_remove(&self, main: &Path, ws: &Path) -> std::io::Result<()> {
        let common = self
            .git
            .common_dir(main)
            .map_err(|error| std::io::Error::other(error.0))?;
        workspace_pointer::remove(&common, ws)
    }

    /// What removing the workspace at `ws` would refuse or lose, read from the current state.
    /// `here` is the directory the command runs in: a workspace that contains it is a blocker,
    /// unless `closing` says the window showing it is going away (D185). Reads only.
    pub fn inspect_removal(
        &self,
        ws: &Path,
        here: &Path,
        closing: bool,
    ) -> Result<RemovalInspection, Error> {
        if !self.is_workspace(ws) {
            return Err(Error::Run(format!("'{}' is not a workspace", ws.display())));
        }
        let mut blockers = Vec::new();
        if !closing && canonical(here).starts_with(canonical(ws)) {
            blockers.push(RemovalBlocker {
                kind: RemovalBlockerKind::Current,
                member: None,
                detail: "the workspace contains the current directory".into(),
                overridable: false,
            });
            return Ok(RemovalInspection { blockers });
        }
        let members = self.members(ws)?;
        for member in &members {
            if ws.join(&member.path).exists() && !Path::new(&member.url).is_dir() {
                blockers.push(RemovalBlocker {
                    kind: RemovalBlockerKind::MemberRepositoryMissing,
                    member: Some(member.path.clone()),
                    detail: format!("its repository {} is not there", member.url),
                    overridable: false,
                });
            }
        }
        if !blockers.is_empty() {
            return Ok(RemovalInspection { blockers });
        }

        // The gitlinks of the members are ignored: every member commit moves one, and the commits
        // are on the member's branch. Any other change of the workspace's repository blocks.
        let gitlinks = self.gitlink_paths(ws)?;
        for change in self
            .git
            .status(ws)?
            .entries
            .iter()
            .filter(|entry| is_change(entry) && !gitlinks.contains(&entry.path))
        {
            blockers.push(RemovalBlocker {
                kind: RemovalBlockerKind::WorkspaceChanges,
                member: None,
                detail: format!("the workspace has changes of its own: {}", change.path),
                overridable: true,
            });
        }
        for member in &members {
            let dir = ws.join(&member.path);
            if !dir.exists() {
                continue;
            }
            let state = self.git.status(&dir)?;
            if state.entries.iter().any(is_change) {
                blockers.push(RemovalBlocker {
                    kind: RemovalBlockerKind::MemberDirty,
                    member: Some(member.path.clone()),
                    detail: "it has uncommitted or untracked files".into(),
                    overridable: true,
                });
            }
            if state.branch.head.is_none() && !self.held(&dir, state.branch.oid.as_deref()) {
                blockers.push(RemovalBlocker {
                    kind: RemovalBlockerKind::MemberDetached,
                    member: Some(member.path.clone()),
                    detail: "it is on a detached HEAD whose commits no branch has".into(),
                    overridable: true,
                });
            }
        }
        Ok(RemovalInspection { blockers })
    }

    /// Whether a local or a remote branch of the repository at `dir` contains `oid`, as D31 counts
    /// them (D178 (4)). An unknown commit counts as held.
    fn held(&self, dir: &Path, oid: Option<&str>) -> bool {
        match oid {
            Some(oid) => self
                .sub
                .text_if(
                    dir,
                    &[
                        "for-each-ref",
                        "--contains",
                        oid,
                        "--count=1",
                        "--format=%(refname)",
                        "refs/heads",
                        "refs/remotes",
                    ],
                )
                .is_some_and(|text| !text.trim().is_empty()),
            None => true,
        }
    }

    /// Removes the workspace at `ws` (D174 (3), D177 (4)): every member's worktree from its
    /// repository, then the folder. The inspection is taken again first, so a refusal names what
    /// is there now. The branches stay.
    pub fn remove(
        &self,
        ws: &Path,
        here: &Path,
        force: bool,
        closing: bool,
    ) -> Result<Removed, Error> {
        let name = ws
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        if let Some(refusal) = self
            .inspect_removal(ws, here, closing)?
            .refusal(&name, force)
        {
            return Err(refusal);
        }
        let members = self.members(ws)?;
        // Canonical, as the pointer files hold it (D180 (2)).
        let key = canonical(ws);
        let mut notes = Vec::new();
        self.remove_members(ws, &members, &key, force, &mut notes)?;
        std::fs::remove_dir_all(ws)?;
        notes.push(format!("removed workspace {name} (the branches are kept)"));
        Ok(Removed {
            path: ws.to_path_buf(),
            notes,
        })
    }

    /// Removes the worktree of each member from its repository, and takes the workspace `key`
    /// out of each repository's pointer file (D180 (2)). The notes collect what went wrong without
    /// stopping the others.
    fn remove_members(
        &self,
        ws: &Path,
        members: &[Member],
        key: &Path,
        force: bool,
        notes: &mut Vec<String>,
    ) -> Result<(), Error> {
        for member in members {
            let dir = ws.join(&member.path);
            let repo = Path::new(&member.url);
            if dir.exists() {
                // From the main checkout, so the folder being removed is never the one Git runs in.
                self.git
                    .remove_worktree(repo, &dir.to_string_lossy(), u8::from(force))?;
            } else if let Err(error) = self.sub.text(repo, &["worktree", "prune"]) {
                notes.push(format!("gitmaster: {}: {}", member.url, error.message()));
            }
            if let Err(error) = self.pointer_remove(repo, key) {
                notes.push(format!(
                    "gitmaster: {}: the workspace is still recorded there: {error}",
                    member.url
                ));
            }
        }
        Ok(())
    }

    /// Checks out the branch the workspace's `.gitmodules` names for the member `member` (its path
    /// relative to the workspace), inside the member (D183). Refused when the member has no branch
    /// or its folder is gone, when its detached HEAD is on no branch (its commits would be left
    /// behind), and when another worktree holds the branch; otherwise Git's own refusal is the
    /// error.
    pub fn check_out_member_branch(&self, ws: &Path, member: &str) -> Result<(), Error> {
        let members = self.members(ws)?;
        let Some(found) = members.iter().find(|candidate| candidate.path == member) else {
            return Err(Error::Run(format!(
                "'{member}' is not a member of this workspace"
            )));
        };
        let Some(branch) = found.branch.as_deref() else {
            return Err(Error::Run(format!(
                "{member} has no branch set in .gitmodules"
            )));
        };
        let dir = ws.join(member);
        if !dir.exists() {
            return Err(Error::Run(format!(
                "{member} is missing: its folder is not there"
            )));
        }
        // A detached HEAD that no branch holds would be left behind by the checkout (D183):
        // refuse, as removal does, and say how many commits.
        let state = self.git.status(&dir)?;
        if state.branch.head.is_none() && !self.held(&dir, state.branch.oid.as_deref()) {
            let count = self.sub.text(
                &dir,
                &[
                    "rev-list",
                    "--count",
                    "HEAD",
                    "--not",
                    "--branches",
                    "--remotes",
                ],
            )?;
            let count: usize = count.trim().parse().unwrap_or(1);
            return Err(Error::Run(if count == 1 {
                format!(
                    "{member}: 1 commit on the detached HEAD is on no branch; put it on a branch first."
                )
            } else {
                format!(
                    "{member}: {count} commits on the detached HEAD are on no branch; put them on a branch first."
                )
            }));
        }
        let entries = self.git.worktrees(Path::new(&found.url))?;
        if let Some(entry) = worktree_cli::holder(&entries, branch)
            && !same_folder(&entry.path, &dir)
        {
            return Err(Error::Run(format!(
                "branch '{branch}' is checked out at {}; a branch can be checked out in one worktree only",
                entry.path
            )));
        }
        let dir_text = dir.to_string_lossy().into_owned();
        self.sub.text(ws, &["-C", &dir_text, "checkout", branch])?;
        Ok(())
    }
}

/// The name of the folder of the main checkout `main`, the project a message names.
fn project_of(main: &str) -> String {
    Path::new(main)
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// `path` as the file system names it, or as written when it cannot be named.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether a status entry is a change: staged, unstaged, untracked or conflicted.
fn is_change(entry: &StatusEntry) -> bool {
    entry.staged.is_some() || entry.unstaged.is_some() || entry.untracked || entry.conflicted
}

/// Whether two paths name the same folder; a path that does not exist compares as written.
fn same_folder(a: &str, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => Path::new(a) == b,
    }
}

/// Whether `branch` is a local branch or a remote one in `known`.
fn has_branch(known: &[BranchEntry], branch: &str) -> bool {
    known.iter().any(|b| !b.is_remote && b.name == branch)
        || worktree_cli::remote_branch(known, branch).is_some()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gm_schema::{BranchHow, RemovalBlocker, RemovalBlockerKind};

    use super::{Error, RemovalInspection, Source, root_from, valid_name};

    /// The path of a successful `root_from`; the message of a failed one.
    fn chosen(result: Result<std::path::PathBuf, Error>) -> std::path::PathBuf {
        match result {
            Ok(path) => path,
            Err(error) => panic!("{error}"),
        }
    }

    #[test]
    fn a_name_is_one_folder_name() {
        assert!(valid_name("checkout-v2"));
        assert!(valid_name("a b"));
        assert!(!valid_name(""));
        assert!(!valid_name("a/b"));
        assert!(!valid_name(".x"));
        assert!(!valid_name("-x"));
    }

    #[test]
    fn the_flag_beats_the_configured_root_and_the_default_is_home_workspaces() {
        let cwd = Path::new("/work");
        let home = Path::new("/home/me");
        let flag = chosen(root_from(Some("/flag"), Some("/config"), Some(home), cwd));
        assert_eq!(flag, Path::new("/flag"));
        let config = chosen(root_from(None, Some("/config"), Some(home), cwd));
        assert_eq!(config, Path::new("/config"));
        let default = chosen(root_from(None, None, Some(home), cwd));
        assert_eq!(default, Path::new("/home/me/workspaces"));
    }

    #[test]
    fn a_relative_root_is_taken_from_the_current_directory_and_a_tilde_from_home() {
        let cwd = Path::new("/work");
        let home = Path::new("/home/me");
        let relative = chosen(root_from(Some("ws"), None, Some(home), cwd));
        assert_eq!(relative, Path::new("/work/ws"));
        let tilde = chosen(root_from(Some("~/ws"), None, Some(home), cwd));
        assert_eq!(tilde, Path::new("/home/me/ws"));
    }

    #[test]
    fn without_a_home_and_without_a_root_there_is_no_default() {
        let error = root_from(None, None, None, Path::new("/work"));
        assert!(matches!(error, Err(Error::Run(_))));
    }

    #[test]
    fn a_branch_that_exists_is_checked_out_and_says_so_about_a_base() {
        assert_eq!(
            Source::Existing.how(false),
            "checked out the existing branch"
        );
        assert_eq!(
            Source::Existing.how(true),
            "checked out the existing branch; the base is ignored"
        );
        assert_eq!(Source::Existing.branch_how(), BranchHow::CheckOut);
    }

    #[test]
    fn a_remote_branch_is_tracked_even_with_a_base_and_a_base_starts_a_new_one() {
        let track = Source::Track("origin/feat/x".into());
        assert_eq!(track.how(false), "created to track origin/feat/x");
        assert_eq!(
            track.how(true),
            "created to track origin/feat/x; the base is ignored"
        );
        let from = Source::From("main".into());
        assert_eq!(from.how(true), "created from main");
        assert_eq!(
            from.branch_how(),
            BranchHow::CreateFrom {
                base: "main".into()
            }
        );
    }

    fn blocker(
        kind: RemovalBlockerKind,
        member: Option<&str>,
        detail: &str,
        overridable: bool,
    ) -> RemovalBlocker {
        RemovalBlocker {
            kind,
            member: member.map(str::to_owned),
            detail: detail.to_owned(),
            overridable,
        }
    }

    #[test]
    fn no_blocker_lets_the_removal_go() {
        let inspection = RemovalInspection {
            blockers: Vec::new(),
        };
        assert_eq!(inspection.refusal("ws", false), None);
    }

    #[test]
    fn overridable_blockers_refuse_unless_forced_and_name_each_member() {
        let inspection = RemovalInspection {
            blockers: vec![
                blocker(
                    RemovalBlockerKind::WorkspaceChanges,
                    None,
                    "the workspace has changes of its own: a.txt",
                    true,
                ),
                blocker(
                    RemovalBlockerKind::MemberDirty,
                    Some("billing"),
                    "it has uncommitted or untracked files",
                    true,
                ),
            ],
        };
        assert_eq!(
            inspection.refusal("ws", false),
            Some(Error::Run(
                "not removing ws: the workspace has changes of its own: a.txt; billing: it has uncommitted or untracked files; use --force to discard them".into()
            ))
        );
        assert_eq!(inspection.refusal("ws", true), None);
    }

    #[test]
    fn a_blocker_that_cannot_be_overridden_refuses_even_when_forced() {
        let inside = RemovalInspection {
            blockers: vec![blocker(
                RemovalBlockerKind::Current,
                None,
                "the workspace contains the current directory",
                false,
            )],
        };
        assert_eq!(
            inside.refusal("ws", true),
            Some(Error::Run(
                "this directory is inside that workspace; run the command from another one".into()
            ))
        );
        let missing = RemovalInspection {
            blockers: vec![blocker(
                RemovalBlockerKind::MemberRepositoryMissing,
                Some("billing"),
                "its repository /gone is not there",
                false,
            )],
        };
        assert_eq!(
            missing.refusal("ws", true),
            Some(Error::Run(
                "cannot remove billing: its repository /gone is not there".into()
            ))
        );
    }
}

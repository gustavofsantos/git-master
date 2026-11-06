//! `gitmaster workspace …` (docs/reference/cli.md, "Workspaces"; D174, D177).
//!
//! One-shot commands that create and look after workspaces: a folder `<root>/<name>` holding a Git
//! repository whose members are submodules, each a linked worktree of a clone the user already
//! has. The workspace is found from its own repository's `gitmaster.workspace.kind` key, and the
//! members from its `.gitmodules`; there is no store of its own (D177 (1)). The member worktrees go
//! through `GitPort`, as the worktree commands do; the superproject's Git calls run as subprocesses
//! here (D177 (5)). The output is read by other programs: the result on standard output, messages
//! on standard error, and the exit status `0`, `1` or `2`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use gm_schema::{BranchEntry, StatusEntry, WorktreeEntry, WorktreeMode};
use serde::Serialize;

use crate::git::SubprocessGit;
use crate::worktree_cli::{self, Args, Env, Failure};
use crate::worktree_include::Mode;
use crate::{GitPort, branch};

const USAGE: &str = "usage: gitmaster workspace create <name> [--root <dir>] [--ephemeral] [--branch <branch> --repos-from <file|-> [--base <rev>] [--existing]]
       gitmaster workspace add <repo> <branch> [<base>] [--workspace <name|dir>] [--root <dir>] [--as <member>] [--include symlink|reflink|none]
       gitmaster workspace list [--format paths|names|tsv|json] [--root <dir>]
       gitmaster workspace path <name> [--root <dir>]
       gitmaster workspace status [--workspace <name|dir>] [--root <dir>] [--format tsv|json]
       gitmaster workspace remove <name> [--root <dir>] [--force]
";

/// The Git key that marks a repository as a workspace, `durable` or `ephemeral` (D177 (1)).
const KIND_KEY: &str = "gitmaster.workspace.kind";
/// The Git key that sets the folder workspaces are created under.
const ROOT_KEY: &str = "gitmaster.workspace.root";
/// The mode Git records for a submodule: a gitlink.
const GITLINK: &str = "160000";

/// Runs `gitmaster workspace <words…>`. Returns `0` on success, `1` when the work fails and `2` for
/// a wrong command line.
pub fn command(
    words: &[String],
    stdin: &mut dyn Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    run(
        words,
        &Env::from_process(),
        &SubprocessGit::default(),
        stdin,
        out,
        err,
    )
}

/// `command` with the environment and Git passed in.
pub fn run(
    words: &[String],
    env: &Env,
    git: &dyn GitPort,
    stdin: &mut dyn Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    run_with(words, env, git, None, stdin, out, err)
}

/// `run`, with the global Git configuration of the superproject's calls set to `global_config`
/// when given, as `SubprocessGit` does for its calls. Tests use it to give the commits of a
/// workspace an identity without changing the process environment.
pub fn run_with(
    words: &[String],
    env: &Env,
    git: &dyn GitPort,
    global_config: Option<&Path>,
    stdin: &mut dyn Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let ctx = Ctx {
        env,
        git,
        sub: Sub {
            global_config: global_config.map(Path::to_path_buf),
        },
    };
    let Some((name, rest)) = words.split_first() else {
        let _ = write!(err, "{USAGE}");
        return 2;
    };
    let result = match name.as_str() {
        "create" => create(rest, &ctx, stdin, out, err),
        "add" => add(rest, &ctx, out, err),
        "list" => list(rest, &ctx, out),
        "path" => path(rest, &ctx, out),
        "status" => status(rest, &ctx, out),
        "remove" => remove(rest, &ctx, out, err),
        _ => Err(Failure::Usage(format!("unknown subcommand '{name}'"))),
    };
    match result {
        Ok(()) => 0,
        Err(Failure::Usage(message)) => {
            let _ = writeln!(err, "gitmaster: {message}");
            let _ = write!(err, "{USAGE}");
            2
        }
        Err(Failure::Run(message)) => {
            let _ = writeln!(err, "gitmaster: {message}");
            1
        }
    }
}

/// What the commands share: the process, Git for the member worktrees, and the superproject's Git.
struct Ctx<'a> {
    env: &'a Env,
    git: &'a dyn GitPort,
    sub: Sub,
}

/// The Git calls of the superproject and of a member's folder, as subprocesses. `global_config`,
/// when set, is the file Git reads as the global configuration.
struct Sub {
    global_config: Option<PathBuf>,
}

impl Sub {
    /// The standard output of `git args` run in `dir`; an error holds git's message.
    fn text(&self, dir: &Path, args: &[&str]) -> Result<String, Failure> {
        let output = self
            .command(dir, args)
            .output()
            .map_err(|e| Failure::Run(format!("cannot run git: {e}")))?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let why = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(Failure::Run(if why.is_empty() {
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

/// The message of a failure, for a line that wraps it.
fn detail(failure: Failure) -> String {
    match failure {
        Failure::Run(message) | Failure::Usage(message) => message,
    }
}

/// Whether `name` can name a workspace or a member: one folder name that does not start with `.`
/// or `-`.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.starts_with(['.', '-'])
}

/// The root the workspaces live under, from the first of `--root`, the configured value and
/// `~/workspaces`. A relative root is taken from `cwd`, and a leading `~` is the home folder.
fn root_from(
    flag: Option<&str>,
    configured: Option<&str>,
    home: Option<&Path>,
    cwd: &Path,
) -> Result<PathBuf, Failure> {
    let chosen = match (flag, configured) {
        (Some(flag), _) => worktree_cli::expand(flag, home),
        (None, Some(value)) => worktree_cli::expand(value, home),
        (None, None) => home
            .ok_or_else(|| Failure::Run("no home folder: give --root".into()))?
            .join("workspaces"),
    };
    Ok(cwd.join(chosen))
}

/// The root for a command: `--root`, else `gitmaster.workspace.root` as Git reads it from the
/// current directory (the global configuration outside a repository), else `~/workspaces`.
fn root_for(ctx: &Ctx, flag: Option<&str>) -> Result<PathBuf, Failure> {
    let configured = ctx
        .sub
        .text_if(&ctx.env.cwd, &["config", "--includes", "--get", ROOT_KEY])
        .or_else(|| {
            ctx.sub.text_if(
                &ctx.env.cwd,
                &["config", "--global", "--includes", "--get", ROOT_KEY],
            )
        })
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    root_from(
        flag,
        configured.as_deref(),
        ctx.env.home.as_deref(),
        &ctx.env.cwd,
    )
}

/// The kind of the workspace at `dir`, or `None` when `dir` is not one: a folder with its own
/// `.git` folder whose configuration sets `gitmaster.workspace.kind`.
fn kind_of(ctx: &Ctx, dir: &Path) -> Option<String> {
    if !dir.join(".git").is_dir() {
        return None;
    }
    let value = ctx
        .sub
        .text_if(dir, &["config", "--local", "--get", KIND_KEY])?;
    let kind = value.trim();
    matches!(kind, "durable" | "ephemeral").then(|| kind.to_owned())
}

/// The workspace `name` under `root`, when there is one.
fn named(ctx: &Ctx, root: &Path, name: &str) -> Option<PathBuf> {
    if !valid_name(name) {
        return None;
    }
    let dir = root.join(name);
    kind_of(ctx, &dir).map(|_| dir)
}

/// The workspace that contains the current directory, from its root or from inside a member.
fn containing(ctx: &Ctx) -> Option<PathBuf> {
    let start = std::fs::canonicalize(&ctx.env.cwd).ok()?;
    start
        .ancestors()
        .find(|dir| kind_of(ctx, dir).is_some())
        .map(Path::to_path_buf)
}

/// The workspace a command works on: `--workspace` (a folder when the value has a `/`, else a name
/// under the root that `--root` or the configured root gives), else the one that contains the
/// current directory.
fn target(ctx: &Ctx, spec: Option<&str>, root: Option<&str>) -> Result<PathBuf, Failure> {
    match spec {
        Some(spec) if spec.contains('/') => {
            let dir = ctx
                .env
                .cwd
                .join(worktree_cli::expand(spec, ctx.env.home.as_deref()));
            kind_of(ctx, &dir)
                .map(|_| dir)
                .ok_or_else(|| Failure::Run(format!("'{spec}' is not a workspace")))
        }
        Some(name) => {
            let root = root_for(ctx, root)?;
            named(ctx, &root, name)
                .ok_or_else(|| Failure::Run(format!("no workspace named '{name}'")))
        }
        None => containing(ctx).ok_or_else(|| {
            Failure::Run("not inside a workspace; give --workspace <name|dir>".into())
        }),
    }
}

/// A branch name `git worktree add` would accept.
fn check_branch(branch: &str) -> Result<(), Failure> {
    if branch.is_empty() || branch.starts_with('-') || !worktree_cli::valid_branch_name(branch) {
        return Err(Failure::Run(format!(
            "'{branch}' is not a valid branch name"
        )));
    }
    Ok(())
}

/// A member as the workspace's `.gitmodules` records it.
struct Member {
    /// The folder name, relative to the workspace.
    path: String,
    /// The path of the main checkout the member belongs to.
    url: String,
    /// The branch the member follows, from `submodule.<name>.branch`.
    branch: Option<String>,
}

/// The members of the workspace at `ws`, in the order of its `.gitmodules`.
fn members(ctx: &Ctx, ws: &Path) -> Result<Vec<Member>, Failure> {
    if !ws.join(".gitmodules").is_file() {
        return Ok(Vec::new());
    }
    let text = ctx
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

/// The commit the workspace's committed `HEAD` records for the member at `path`, when it has one.
fn recorded(ctx: &Ctx, ws: &Path, path: &str) -> Option<String> {
    let text = ctx
        .sub
        .text_if(ws, &["ls-tree", "-z", "HEAD", "--", path])?;
    let record = text.split('\0').next()?;
    let (meta, _) = record.split_once('\t')?;
    let mut fields = meta.split_whitespace();
    let (mode, _kind, sha) = (fields.next()?, fields.next()?, fields.next()?);
    (mode == GITLINK).then(|| sha.to_owned())
}

/// The paths the workspace's index records as gitlinks, that is its members.
fn gitlink_paths(ctx: &Ctx, ws: &Path) -> Result<Vec<String>, Failure> {
    let text = ctx.sub.text(ws, &["ls-files", "-s", "-z"])?;
    Ok(text
        .split('\0')
        .filter_map(|record| {
            let (meta, path) = record.split_once('\t')?;
            let mode = meta.split_whitespace().next()?;
            (mode == GITLINK).then(|| path.to_owned())
        })
        .collect())
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

/// What `add` chooses for a member: the branch, the base it is made from, the folder name and the
/// sharing mode of `.worktreeinclude`.
struct Choice<'a> {
    branch: &'a str,
    base: Option<&'a str>,
    member: Option<&'a str>,
    include: Option<&'a str>,
}

/// A member that can be added, checked and decided before anything is created (D178 (3)).
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
    /// What the report says the branch was, after `added <dir> on <branch>:`.
    how: String,
    include: Mode,
}

/// Checks the member `choice` would add to the workspace at `ws`, which need not exist yet, and
/// decides its branch. Nothing is created. `known` are the repository's branches.
fn plan_member(
    ctx: &Ctx,
    ws: &Path,
    root: &Path,
    entries: &[WorktreeEntry],
    known: &[BranchEntry],
    choice: &Choice,
) -> Result<Plan, Failure> {
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
        return Err(Failure::Run(format!(
            "'{member}' is not a folder name for a member"
        )));
    }
    if let Some(entry) = worktree_cli::holder(entries, branch) {
        return Err(Failure::Run(format!(
            "branch '{branch}' is checked out at {}; a branch can be checked out in one worktree only",
            entry.path
        )));
    }
    let existing = members(ctx, ws)?;
    if existing.iter().any(|kept| kept.path == member) {
        return Err(Failure::Run(format!("'{member}' is already a member")));
    }
    if let Some(kept) = existing.iter().find(|kept| same_folder(&kept.url, &main)) {
        return Err(Failure::Run(format!(
            "{} is already a member, as '{}'",
            main.display(),
            kept.path
        )));
    }
    let dir = ws.join(&member);
    if dir.exists() {
        return Err(Failure::Run(format!("'{}' already exists", dir.display())));
    }

    let include = worktree_cli::include_mode(ctx.git, root, choice.include)?;
    let (mode, how) = branch_choice(ctx, root, &main, known, branch, choice.base)?;
    Ok(Plan {
        member,
        dir,
        main,
        root: root.to_path_buf(),
        branch: branch.to_owned(),
        mode,
        how,
        include,
    })
}

/// The branch a member is made on, and the words the report uses for it, as D178 (2) chooses it:
/// the local branch `branch` when there is one; else a new local branch that tracks the remote
/// one, whatever the `base`; else a new branch from `base`, or from the repository's base branch,
/// or from `HEAD` when there is none.
fn branch_choice(
    ctx: &Ctx,
    root: &Path,
    main: &Path,
    known: &[BranchEntry],
    branch: &str,
    base: Option<&str>,
) -> Result<(WorktreeMode, String), Failure> {
    if known.iter().any(|b| !b.is_remote && b.name == branch) {
        let how = if base.is_some() {
            "checked out the existing branch; the base is ignored"
        } else {
            "checked out the existing branch"
        };
        let mode = WorktreeMode::Existing {
            branch: branch.to_owned(),
        };
        return Ok((mode, how.to_owned()));
    }
    // A branch a remote already has is tracked, never forked from a base under the same name.
    if let Some(remote) = worktree_cli::remote_branch(known, branch) {
        let how = if base.is_some() {
            format!("created to track {remote}; the base is ignored")
        } else {
            format!("created to track {remote}")
        };
        let mode = WorktreeMode::New {
            branch: branch.to_owned(),
            base: remote,
        };
        return Ok((mode, how));
    }
    let from = match base {
        Some(base) => {
            check_commit(ctx, main, base)?;
            base.to_owned()
        }
        None => repo_base(ctx, root, main, known)?.unwrap_or_else(|| "HEAD".to_owned()),
    };
    let how = format!("created from {from}");
    let mode = WorktreeMode::New {
        branch: branch.to_owned(),
        base: from,
    };
    Ok((mode, how))
}

/// The base branch of a repository, as `merge_base` picks it from `origin/HEAD` and the local
/// branches (D56, D178 (2)). `None` when there is none, and the caller then uses `HEAD`.
fn repo_base(
    ctx: &Ctx,
    root: &Path,
    main: &Path,
    known: &[BranchEntry],
) -> Result<Option<String>, Failure> {
    let origin = ctx.git.origin_head(root)?;
    let origin = origin.as_deref().and_then(branch::origin_head_branch);
    let locals: Vec<&str> = known
        .iter()
        .filter(|b| !b.is_remote)
        .map(|b| b.name.as_str())
        .collect();
    let current = ctx.git.status(main)?.branch.head;
    Ok(branch::merge_base(origin, &locals, current.as_deref()))
}

/// Fails unless `rev` names a commit in the repository whose main checkout is `main`.
fn check_commit(ctx: &Ctx, main: &Path, rev: &str) -> Result<(), Failure> {
    let spec = format!("{rev}^{{commit}}");
    if ctx
        .sub
        .text_if(main, &["rev-parse", "--verify", "--quiet", &spec])
        .is_none()
    {
        return Err(Failure::Run(format!(
            "'{rev}' is not a commit in {}",
            main.display()
        )));
    }
    Ok(())
}

/// Makes the member of `plan` in the workspace at `ws`: its worktree, the registration of the
/// worktree as a submodule and the commit of the workspace's repository (D177 (2) and (3)). Reports
/// it on `err`. Returns the member's folder. When the commit fails the member stays added and
/// staged, and the error says so.
fn add_member(ctx: &Ctx, ws: &Path, plan: &Plan, err: &mut dyn Write) -> Result<PathBuf, Failure> {
    let dir = &plan.dir;
    let branch = plan.branch.as_str();
    let member = plan.member.as_str();
    ctx.git
        .create_worktree(&plan.root, &dir.to_string_lossy(), &plan.mode)?;
    worktree_cli::share(ctx.git, &plan.main, dir, plan.include, false, err);

    // Registers the existing worktree: Git adds it in place, without a clone (D174).
    let registered = ctx.sub.text(
        ws,
        &[
            "submodule",
            "add",
            "-b",
            branch,
            &plan.main.to_string_lossy(),
            member,
        ],
    );
    if let Err(error) = registered {
        let _ = ctx
            .git
            .remove_worktree(&plan.root, &dir.to_string_lossy(), 1);
        return Err(error);
    }

    let message = format!("Add {member} on {branch}");
    ctx.sub
        .text(
            ws,
            &["commit", "-q", "-m", &message, "--", ".gitmodules", member],
        )
        .map_err(|error| {
            Failure::Run(format!(
                "{member} is added and staged in {}, but not committed: {}; commit it with git -C {} commit",
                ws.display(),
                detail(error),
                ws.display()
            ))
        })?;
    writeln!(
        err,
        "gitmaster: added {} on {branch}: {}",
        dir.display(),
        plan.how
    )?;
    Ok(dir.clone())
}

/// Whether `branch` is a local branch or a remote one in `known`.
fn has_branch(known: &[BranchEntry], branch: &str) -> bool {
    known.iter().any(|b| !b.is_remote && b.name == branch)
        || worktree_cli::remote_branch(known, branch).is_some()
}

/// Plans the member of each repository of `repos` for `create --branch` (D178 (1) and (3)): every
/// repository, or with `--existing` the ones that have the branch. Returns the plans. When one
/// cannot be added, or when two would have one folder name, prints every problem and returns an
/// error, so nothing is created. Nothing is created here either.
fn plan_repos(
    ctx: &Ctx,
    ws: &Path,
    args: &Args,
    branch: &str,
    repos: &[worktree_cli::Repo],
    err: &mut dyn Write,
) -> Result<Vec<Plan>, Failure> {
    let existing = args.switch("existing");
    let base = args.option("base");
    let mut plans: Vec<Plan> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for repo in repos {
        let known = ctx.git.branches(&repo.root)?;
        if existing && !has_branch(&known, branch) {
            writeln!(
                err,
                "gitmaster: skipped {}: no branch '{branch}'",
                repo.project
            )?;
            continue;
        }
        let choice = Choice {
            branch,
            base,
            member: None,
            include: None,
        };
        match plan_member(ctx, ws, &repo.root, &repo.entries, &known, &choice) {
            Ok(plan) => plans.push(plan),
            Err(error) => problems.push(format!("{}: {}", repo.project, detail(error))),
        }
    }
    for (index, plan) in plans.iter().enumerate() {
        if let Some(other) = plans[..index]
            .iter()
            .find(|other| other.member == plan.member)
        {
            problems.push(format!(
                "{} and {} would both be the member '{}'",
                other.main.display(),
                plan.main.display(),
                plan.member
            ));
        }
    }
    if !problems.is_empty() {
        for problem in &problems {
            writeln!(err, "gitmaster: {problem}")?;
        }
        return Err(Failure::Run(
            "no workspace was created: fix the repositories above, or leave them out of the list"
                .into(),
        ));
    }
    if plans.is_empty() {
        return Err(Failure::Run(
            "no repository would be added; no workspace was created".into(),
        ));
    }
    Ok(plans)
}

fn create(
    words: &[String],
    ctx: &Ctx,
    stdin: &mut dyn Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), Failure> {
    let args = Args::parse(
        words,
        &["root", "branch", "repos-from", "base"],
        &["ephemeral", "existing"],
    )?;
    args.exactly(1, "a workspace name")?;
    let name = args.positional[0].as_str();
    if !valid_name(name) {
        return Err(Failure::Run(format!("'{name}' is not a workspace name")));
    }
    if args.option("branch").is_some() != args.option("repos-from").is_some() {
        return Err(Failure::Usage(
            "--branch and --repos-from go together".into(),
        ));
    }
    if args.option("branch").is_none() && (args.option("base").is_some() || args.switch("existing"))
    {
        return Err(Failure::Usage("--base and --existing need --branch".into()));
    }
    if let Some(branch) = args.option("branch") {
        check_branch(branch)?;
    }
    let root = root_for(ctx, args.option("root"))?;
    let dir = root.join(name);
    if dir.exists() {
        return Err(Failure::Run(format!("'{}' already exists", dir.display())));
    }
    // The list is read and planned before anything is created, so a bad list leaves nothing behind.
    let plans = match args.option("branch") {
        Some(branch) => {
            let (repos, _, skipped) = worktree_cli::listed(&args, ctx.env, ctx.git, stdin, err)?;
            for folder in &skipped {
                writeln!(
                    err,
                    "gitmaster: skipped {}: not a repository",
                    folder.display()
                )?;
            }
            Some(plan_repos(ctx, &dir, &args, branch, &repos, err)?)
        }
        None => None,
    };

    std::fs::create_dir_all(&root)?;
    std::fs::create_dir(&dir)?;
    ctx.sub.text(&dir, &["init", "-q", "-b", "main"])?;
    let kind = if args.switch("ephemeral") {
        "ephemeral"
    } else {
        "durable"
    };
    ctx.sub.text(&dir, &["config", "--local", KIND_KEY, kind])?;
    writeln!(out, "{}", dir.display())?;

    let Some(plans) = plans else {
        return Ok(());
    };
    let mut failed = false;
    for plan in &plans {
        if let Err(error) = add_member(ctx, &dir, plan, err) {
            failed = true;
            writeln!(err, "gitmaster: {}: {}", plan.member, detail(error))?;
        }
    }
    if failed {
        return Err(Failure::Run(
            "some repositories could not be added; the workspace was created".into(),
        ));
    }
    Ok(())
}

fn add(
    words: &[String],
    ctx: &Ctx,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), Failure> {
    let args = Args::parse(words, &["workspace", "as", "include", "root"], &[])?;
    if args.positional.len() < 2 || args.positional.len() > 3 {
        return Err(Failure::Usage(
            "expected a repository, a branch and an optional base".into(),
        ));
    }
    let branch = args.positional[1].as_str();
    let choice = Choice {
        branch,
        base: args.positional.get(2).map(String::as_str),
        member: args.option("as"),
        include: args.option("include"),
    };
    let ws = target(ctx, args.option("workspace"), args.option("root"))?;
    let repo = worktree_cli::resolve_repo(&args.positional[0], ctx.env)?;
    let (root, entries) = worktree_cli::worktrees_in(&repo, ctx.git)?;
    let known = ctx.git.branches(&root)?;
    let plan = plan_member(ctx, &ws, &root, &entries, &known, &choice)?;
    let dir = add_member(ctx, &ws, &plan, err)?;
    writeln!(out, "{}", dir.display())?;
    Ok(())
}

/// One workspace in a listing.
#[derive(Serialize)]
struct Row {
    name: String,
    kind: String,
    path: String,
    members: usize,
}

/// The workspaces directly under `root`, by name.
fn workspaces(ctx: &Ctx, root: &Path) -> Result<Vec<Row>, Failure> {
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
        let Some(kind) = kind_of(ctx, &dir) else {
            continue;
        };
        let name = dir
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        rows.push(Row {
            name,
            kind,
            path: dir.to_string_lossy().into_owned(),
            members: members(ctx, &dir)?.len(),
        });
    }
    Ok(rows)
}

fn list(words: &[String], ctx: &Ctx, out: &mut dyn Write) -> Result<(), Failure> {
    let args = Args::parse(words, &["format", "root"], &[])?;
    args.exactly(0, "no arguments")?;
    let format = args.option("format").unwrap_or("paths");
    if !["paths", "names", "tsv", "json"].contains(&format) {
        return Err(Failure::Usage(format!("unknown format '{format}'")));
    }
    let root = root_for(ctx, args.option("root"))?;
    let rows = workspaces(ctx, &root)?;
    match format {
        "paths" => {
            for row in &rows {
                writeln!(out, "{}", row.path)?;
            }
        }
        "names" => {
            for row in &rows {
                writeln!(out, "{}", row.name)?;
            }
        }
        "tsv" => {
            for row in &rows {
                writeln!(out, "{}\t{}\t{}", row.name, row.kind, row.path)?;
            }
        }
        _ => {
            let json = serde_json::to_string(&rows).map_err(|e| Failure::Run(e.to_string()))?;
            writeln!(out, "{json}")?;
        }
    }
    Ok(())
}

fn path(words: &[String], ctx: &Ctx, out: &mut dyn Write) -> Result<(), Failure> {
    let args = Args::parse(words, &["root"], &[])?;
    args.exactly(1, "a workspace name")?;
    let name = args.positional[0].as_str();
    let root = root_for(ctx, args.option("root"))?;
    let dir = named(ctx, &root, name)
        .ok_or_else(|| Failure::Run(format!("no workspace named '{name}'")))?;
    writeln!(out, "{}", dir.display())?;
    Ok(())
}

/// One member in a status listing. A member whose folder is gone has `branch` `(missing)`.
#[derive(Serialize)]
struct MemberRow {
    member: String,
    branch: Option<String>,
    detached: bool,
    dirty: bool,
    new_commits: usize,
    recorded: Option<String>,
    head: Option<String>,
    repo: String,
    #[serde(skip)]
    missing: bool,
}

impl MemberRow {
    /// The branch column of the `tsv` format.
    fn branch_label(&self) -> &str {
        match (&self.branch, self.detached) {
            (Some(branch), _) => branch,
            (None, true) => "(detached)",
            (None, false) => "(missing)",
        }
    }
}

/// The status of one member of the workspace at `ws`.
fn member_row(ctx: &Ctx, ws: &Path, member: &Member) -> Result<MemberRow, Failure> {
    let recorded = recorded(ctx, ws, &member.path);
    let mut row = MemberRow {
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
    let state = ctx.git.status(&dir)?;
    row.branch.clone_from(&state.branch.head);
    row.detached = state.branch.head.is_none();
    row.head.clone_from(&state.branch.oid);
    row.dirty = state.entries.iter().any(is_change);
    let range = match &recorded {
        Some(sha) => format!("{sha}..HEAD"),
        None => "HEAD".to_owned(),
    };
    row.new_commits = ctx
        .sub
        .text_if(&dir, &["rev-list", "--count", &range])
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0);
    Ok(row)
}

fn status(words: &[String], ctx: &Ctx, out: &mut dyn Write) -> Result<(), Failure> {
    let args = Args::parse(words, &["workspace", "format", "root"], &[])?;
    args.exactly(0, "no arguments")?;
    let format = args.option("format").unwrap_or("tsv");
    if !["tsv", "json"].contains(&format) {
        return Err(Failure::Usage(format!("unknown format '{format}'")));
    }
    let ws = target(ctx, args.option("workspace"), args.option("root"))?;
    let mut rows = Vec::new();
    for member in members(ctx, &ws)? {
        rows.push(member_row(ctx, &ws, &member)?);
    }
    if format == "json" {
        let json = serde_json::to_string(&rows).map_err(|e| Failure::Run(e.to_string()))?;
        writeln!(out, "{json}")?;
    } else {
        for row in &rows {
            writeln!(
                out,
                "{}\t{}\t{}\t{}\t{}",
                row.member,
                row.branch_label(),
                if row.dirty { "dirty" } else { "clean" },
                row.new_commits,
                row.repo
            )?;
        }
    }
    let missing = rows.iter().filter(|row| row.missing).count();
    if missing > 0 {
        return Err(Failure::Run(format!("{missing} member(s) missing")));
    }
    Ok(())
}

fn remove(
    words: &[String],
    ctx: &Ctx,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), Failure> {
    let args = Args::parse(words, &["root"], &["force"])?;
    args.exactly(1, "a workspace name")?;
    let name = args.positional[0].as_str();
    let root = root_for(ctx, args.option("root"))?;
    let ws = named(ctx, &root, name)
        .ok_or_else(|| Failure::Run(format!("no workspace named '{name}'")))?;
    // Never removed from inside, even with --force: the command would delete its own directory.
    let here = std::fs::canonicalize(&ctx.env.cwd).unwrap_or_else(|_| ctx.env.cwd.clone());
    let there = std::fs::canonicalize(&ws).unwrap_or_else(|_| ws.clone());
    if here.starts_with(&there) {
        return Err(Failure::Run(
            "this directory is inside that workspace; run the command from another one".into(),
        ));
    }
    let force = args.switch("force");
    let members = members(ctx, &ws)?;
    for member in &members {
        if ws.join(&member.path).exists() && !Path::new(&member.url).is_dir() {
            return Err(Failure::Run(format!(
                "cannot remove {}: its repository {} is not there",
                member.path, member.url
            )));
        }
    }

    // The gitlinks of the members are ignored: every member commit moves one, and the commits are
    // on the member's branch. Any other change of the workspace's repository blocks.
    let gitlinks = gitlink_paths(ctx, &ws)?;
    let mut found: Vec<String> = Vec::new();
    for change in ctx
        .git
        .status(&ws)?
        .entries
        .iter()
        .filter(|entry| is_change(entry) && !gitlinks.contains(&entry.path))
    {
        found.push(format!(
            "the workspace has changes of its own: {}",
            change.path
        ));
    }
    for member in &members {
        let dir = ws.join(&member.path);
        if !dir.exists() {
            continue;
        }
        let state = ctx.git.status(&dir)?;
        if state.entries.iter().any(is_change) {
            found.push(format!(
                "{}: it has uncommitted or untracked files",
                member.path
            ));
        }
        if state.branch.head.is_none() {
            // Held by a local or a remote branch, as D31 counts them (D178 (4)).
            let held = match &state.branch.oid {
                Some(oid) => ctx
                    .sub
                    .text_if(
                        &dir,
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
            };
            if !held {
                found.push(format!(
                    "{}: it is on a detached HEAD whose commits no branch has",
                    member.path
                ));
            }
        }
    }
    if !found.is_empty() && !force {
        return Err(Failure::Run(format!(
            "not removing {name}: {}; use --force to discard them",
            found.join("; ")
        )));
    }

    for member in &members {
        let dir = ws.join(&member.path);
        let repo = Path::new(&member.url);
        if dir.exists() {
            // From the main checkout, so the folder being removed is never the one git runs in.
            ctx.git
                .remove_worktree(repo, &dir.to_string_lossy(), u8::from(force))?;
        } else if let Err(error) = ctx.sub.text(repo, &["worktree", "prune"]) {
            writeln!(err, "gitmaster: {}: {}", member.url, detail(error))?;
        }
    }
    std::fs::remove_dir_all(&ws)?;
    writeln!(out, "{}", ws.display())?;
    writeln!(err, "removed workspace {name} (the branches are kept)")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{root_from, valid_name};
    use crate::worktree_cli::Failure;

    /// The path of a successful `root_from`; the message of a failed one.
    fn chosen(result: Result<std::path::PathBuf, Failure>) -> std::path::PathBuf {
        match result {
            Ok(path) => path,
            Err(Failure::Run(message) | Failure::Usage(message)) => panic!("{message}"),
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
        assert!(matches!(error, Err(Failure::Run(_))));
    }
}

//! `gitmaster workspace …` (docs/reference/cli.md, "Workspaces"; D174, D177).
//!
//! One-shot commands that create and look after workspaces. The work is in `workspace`, which
//! returns data; this module reads the arguments and prints the results. The output is read by
//! other programs: the result on standard output, messages on standard error, and the exit status
//! `0`, `1` or `2`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use gm_schema::MemberRequest;

use crate::GitPort;
use crate::workspace::{self, AddRequest, CreateRequest, Workspaces};
use crate::worktree_cli::{self, Args, Env, Failure};

const USAGE: &str = "usage: gitmaster workspace create <name> [--root <dir>] [--branch <branch> --repos-from <file|-> [--base <rev>] [--existing]]
       gitmaster workspace add <repo> <branch> [<base>] [--workspace <name|dir>] [--root <dir>] [--as <member>] [--include symlink|reflink|none]
       gitmaster workspace list [--format paths|names|tsv|json] [--root <dir>]
       gitmaster workspace path <name> [--root <dir>]
       gitmaster workspace status [--workspace <name|dir>] [--root <dir>] [--format tsv|json]
       gitmaster workspace remove <name> [--root <dir>] [--force]
";

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
        &crate::git::SubprocessGit::default(),
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
        ws: Workspaces::new(git, global_config.map(Path::to_path_buf)),
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

impl From<workspace::Error> for Failure {
    fn from(error: workspace::Error) -> Self {
        match error {
            workspace::Error::Usage(message) => Self::Usage(message),
            workspace::Error::Run(message) => Self::Run(message),
        }
    }
}

/// What the commands share: the process, Git for the member worktrees, and the workspace
/// operations over it.
struct Ctx<'a> {
    env: &'a Env,
    git: &'a dyn GitPort,
    ws: Workspaces<'a>,
}

/// The root for a command: `--root`, else the configured root, else `~/workspaces`.
fn root_for(ctx: &Ctx, flag: Option<&str>) -> Result<PathBuf, Failure> {
    Ok(ctx.ws.root(flag, &ctx.env.cwd, ctx.env.home.as_deref())?)
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
            ctx.ws
                .is_workspace(&dir)
                .then_some(dir)
                .ok_or_else(|| Failure::Run(format!("'{spec}' is not a workspace")))
        }
        Some(name) => {
            let root = root_for(ctx, root)?;
            ctx.ws
                .named(&root, name)
                .ok_or_else(|| Failure::Run(format!("no workspace named '{name}'")))
        }
        None => ctx.ws.containing(&ctx.env.cwd).ok_or_else(|| {
            Failure::Run("not inside a workspace; give --workspace <name|dir>".into())
        }),
    }
}

/// The name of the repository a problem is about: its main checkout's folder.
fn project_of(repo: &str) -> String {
    Path::new(repo)
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
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
        &["existing"],
    )?;
    args.exactly(1, "a workspace name")?;
    let name = args.positional[0].as_str();
    if !workspace::valid_name(name) {
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
        workspace::check_branch(branch)?;
    }
    let root = root_for(ctx, args.option("root"))?;
    workspace::ensure_absent(&root.join(name))?;
    // The list is read and planned before anything is created, so a bad list leaves nothing behind.
    let mut members = Vec::new();
    if let Some(branch) = args.option("branch") {
        let (repos, _, skipped) = worktree_cli::listed(&args, ctx.env, ctx.git, stdin, err)?;
        for folder in &skipped {
            writeln!(
                err,
                "gitmaster: skipped {}: not a repository",
                folder.display()
            )?;
        }
        members = repos
            .iter()
            .map(|repo| MemberRequest {
                repo: repo.root.to_string_lossy().into_owned(),
                branch: branch.to_owned(),
            })
            .collect();
    }
    let request = CreateRequest {
        name: name.to_owned(),
        root,
        members,
        base: args.option("base").map(str::to_owned),
        existing: args.switch("existing"),
    };
    let plan = ctx.ws.plan(&request)?;
    for note in &plan.notes {
        writeln!(err, "{note}")?;
    }
    if !plan.problems.is_empty() {
        for problem in &plan.problems {
            let line = match &problem.repo {
                Some(repo) => format!("gitmaster: {}: {}", project_of(repo), problem.message),
                None => format!("gitmaster: {}", problem.message),
            };
            writeln!(err, "{line}")?;
        }
        return Err(Failure::Run(
            "no workspace was created: fix the repositories above, or leave them out of the list"
                .into(),
        ));
    }
    if args.option("branch").is_some() && plan.members.is_empty() {
        return Err(Failure::Run(
            "no repository would be added; no workspace was created".into(),
        ));
    }

    let created = ctx.ws.create(plan)?;
    writeln!(out, "{}", created.path.display())?;
    let mut failed = false;
    for added in &created.added {
        for note in &added.notes {
            writeln!(err, "{note}")?;
        }
        if let Some(failure) = &added.failure {
            failed = true;
            writeln!(err, "gitmaster: {}: {failure}", added.member)?;
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
    let ws = target(ctx, args.option("workspace"), args.option("root"))?;
    let repo = worktree_cli::resolve_repo(&args.positional[0], ctx.env)?;
    let request = AddRequest {
        repo: repo.to_string_lossy().into_owned(),
        branch: branch.to_owned(),
        base: args.positional.get(2).cloned(),
        member: args.option("as").map(str::to_owned),
        include: args.option("include").map(str::to_owned),
    };
    let added = ctx.ws.add(&ws, &request)?;
    for note in &added.notes {
        writeln!(err, "{note}")?;
    }
    if let Some(failure) = added.failure {
        return Err(Failure::Run(failure));
    }
    writeln!(out, "{}", added.dir.display())?;
    Ok(())
}

fn list(words: &[String], ctx: &Ctx, out: &mut dyn Write) -> Result<(), Failure> {
    let args = Args::parse(words, &["format", "root"], &[])?;
    args.exactly(0, "no arguments")?;
    let format = args.option("format").unwrap_or("paths");
    if !["paths", "names", "tsv", "json"].contains(&format) {
        return Err(Failure::Usage(format!("unknown format '{format}'")));
    }
    let root = root_for(ctx, args.option("root"))?;
    let rows = ctx.ws.list(&root)?;
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
                writeln!(out, "{}\t{}\t{}", row.name, row.members, row.path)?;
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
    let dir = ctx
        .ws
        .named(&root, name)
        .ok_or_else(|| Failure::Run(format!("no workspace named '{name}'")))?;
    writeln!(out, "{}", dir.display())?;
    Ok(())
}

fn status(words: &[String], ctx: &Ctx, out: &mut dyn Write) -> Result<(), Failure> {
    let args = Args::parse(words, &["workspace", "format", "root"], &[])?;
    args.exactly(0, "no arguments")?;
    let format = args.option("format").unwrap_or("tsv");
    if !["tsv", "json"].contains(&format) {
        return Err(Failure::Usage(format!("unknown format '{format}'")));
    }
    let ws = target(ctx, args.option("workspace"), args.option("root"))?;
    let rows = ctx.ws.member_states(&ws)?;
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
    let ws = ctx
        .ws
        .named(&root, name)
        .ok_or_else(|| Failure::Run(format!("no workspace named '{name}'")))?;
    let removed = ctx
        .ws
        .remove(&ws, &ctx.env.cwd, args.switch("force"), false)?;
    writeln!(out, "{}", removed.path.display())?;
    for note in &removed.notes {
        writeln!(err, "{note}")?;
    }
    Ok(())
}

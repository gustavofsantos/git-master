//! `gitmaster worktree …` (docs/reference/cli.md, "Worktrees").
//!
//! One-shot commands for shell scripts and tmux bindings: list worktrees by path or by branch,
//! find the worktree of a branch, and create one (reusing it when it exists), sharing the files
//! `.worktreeinclude` names. The output is meant to be read by other programs: a path or a branch
//! per line on standard output, messages on standard error. Everything that depends on the process
//! (the directory, the home, the environment) comes in through `Env`, and Git through `GitPort`,
//! so the commands are tested without touching either.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use gm_schema::{Blocker, WorktreeEntry, WorktreeMode};

use crate::git::SubprocessGit;
use crate::worktree_include::{self, Action, Mode};
use crate::{GitError, GitPort};

const USAGE: &str = "usage: gitmaster worktree list [--format paths|branches|tsv|json] [--linked]
       gitmaster worktree branches [--missing]
       gitmaster worktree path <branch>
       gitmaster worktree add <branch> [<base>] [--root <dir>] [--include symlink|reflink|none]
       gitmaster worktree remove <branch> [--force]
       gitmaster worktree include [<branch>] [--include symlink|reflink] [--dry-run]

Every command takes --repo <path> to work on another repository. list and branches take --repo
more than once, or --repos-from <file|-> to read one repository per line (- is standard input).
";

/// The git config key that sets the folder worktrees are created under.
const ROOT_KEY: &str = "gitmaster.worktree.root";
/// The git config key that sets how `.worktreeinclude` paths are shared.
const INCLUDE_KEY: &str = "gitmaster.worktree.include";

/// What the commands need from the process.
#[derive(Clone, Debug)]
pub struct Env {
    /// The directory the command runs in.
    pub cwd: PathBuf,
    /// The home directory, for a leading `~` in a folder setting.
    pub home: Option<PathBuf>,
    /// `WORKTREES_HOME`: the folder worktrees go under when nothing else says.
    pub worktrees_home: Option<PathBuf>,
}

impl Env {
    /// The environment of the running process.
    #[must_use]
    pub fn from_process() -> Self {
        let var = |name| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            cwd: std::env::current_dir().unwrap_or_default(),
            home: var("HOME"),
            worktrees_home: var("WORKTREES_HOME"),
        }
    }
}

/// Runs `gitmaster worktree <words…>`. Returns `0` on success, `1` when the work fails and `2` for
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
    let Some((name, rest)) = words.split_first() else {
        let _ = write!(err, "{USAGE}");
        return 2;
    };
    let result = match name.as_str() {
        "list" => list(rest, env, git, stdin, out, err),
        "branches" => branches(rest, env, git, stdin, out, err),
        "path" => path(rest, env, git, out),
        "add" => add(rest, env, git, out, err),
        "remove" => remove(rest, env, git, out, err),
        "include" => include(rest, env, git, err),
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

enum Failure {
    Usage(String),
    Run(String),
}

impl From<GitError> for Failure {
    fn from(error: GitError) -> Self {
        Self::Run(error.0)
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::Run(error.to_string())
    }
}

/// The words of a command line sorted into positionals, valued options and switches.
struct Args {
    positional: Vec<String>,
    options: Vec<(String, String)>,
    switches: Vec<String>,
}

impl Args {
    /// `valued` options take a value (`--root DIR` or `--root=DIR`); `switches` take none.
    fn parse(words: &[String], valued: &[&str], switches: &[&str]) -> Result<Self, Failure> {
        let mut args = Self {
            positional: Vec::new(),
            options: Vec::new(),
            switches: Vec::new(),
        };
        let mut words = words.iter();
        while let Some(word) = words.next() {
            let Some(flag) = word.strip_prefix("--") else {
                args.positional.push(word.clone());
                continue;
            };
            let (flag, inline) = match flag.split_once('=') {
                Some((flag, value)) => (flag, Some(value.to_owned())),
                None => (flag, None),
            };
            if valued.contains(&flag) {
                let value = match inline {
                    Some(value) => value,
                    None => words
                        .next()
                        .cloned()
                        .ok_or_else(|| Failure::Usage(format!("--{flag} needs a value")))?,
                };
                args.options.push((flag.to_owned(), value));
            } else if switches.contains(&flag) && inline.is_none() {
                args.switches.push(flag.to_owned());
            } else {
                return Err(Failure::Usage(format!("unknown option '--{flag}'")));
            }
        }
        Ok(args)
    }

    fn option(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .rev()
            .find(|(flag, _)| flag == name)
            .map(|(_, value)| value.as_str())
    }

    /// Every value given for `name`, in order.
    fn values(&self, name: &str) -> Vec<&str> {
        self.options
            .iter()
            .filter(|(flag, _)| flag == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    fn switch(&self, name: &str) -> bool {
        self.switches.iter().any(|flag| flag == name)
    }

    /// Exactly `count` positionals, else a usage error.
    fn exactly(&self, count: usize, what: &str) -> Result<(), Failure> {
        if self.positional.len() == count {
            Ok(())
        } else {
            Err(Failure::Usage(format!("expected {what}")))
        }
    }
}

/// The top folder of the working tree that contains `dir`.
fn toplevel(dir: &Path) -> Result<PathBuf, Failure> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .map_err(|e| Failure::Run(format!("cannot run git: {e}")))?;
    if !output.status.success() {
        return Err(Failure::Run("not inside a git repository".into()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(PathBuf::from(text.trim_end_matches('\n')))
}

/// The worktrees of the repository around `dir` whose folder still exists, main checkout first,
/// and the folder Git commands run in.
fn worktrees_in(dir: &Path, git: &dyn GitPort) -> Result<(PathBuf, Vec<WorktreeEntry>), Failure> {
    let root = toplevel(dir)?;
    let entries = git.worktrees(&root)?;
    if entries.first().is_some_and(|main| main.bare) {
        return Err(Failure::Run("bare repositories are not supported".into()));
    }
    let usable = entries
        .into_iter()
        .filter(|entry| entry.prunable.is_none())
        .collect();
    Ok((root, usable))
}

/// The folder `spec` names, taken from the current directory when it is relative.
fn resolve_repo(spec: &str, env: &Env) -> Result<PathBuf, Failure> {
    let path = env.cwd.join(expand(spec, env.home.as_deref()));
    if path.is_dir() {
        Ok(path)
    } else {
        Err(Failure::Run(format!("'{spec}' is not a folder")))
    }
}

/// A repository a command works on.
struct Repo {
    /// The main checkout's folder name.
    project: String,
    /// The folder Git commands run in.
    root: PathBuf,
    entries: Vec<WorktreeEntry>,
}

/// The repositories of a read command: those named by `--repo`, those listed in the file of
/// `--repos-from` (one folder per line, `-` for standard input), else the one around the current
/// directory. The flag is `true` when the caller gave a list or more than one `--repo`, which adds
/// the project to the output. A repository named with `--repo` that is not one is an error; a
/// folder in a list that is not one is skipped, since the caller's list may hold other folders.
/// Each repository is read once, however many of its worktrees are listed.
fn repositories(
    args: &Args,
    env: &Env,
    git: &dyn GitPort,
    stdin: &mut dyn Read,
    err: &mut dyn Write,
) -> Result<(Vec<Repo>, bool), Failure> {
    let specs = args.values("repo");
    let from = args.option("repos-from");
    let multi = from.is_some() || specs.len() > 1;
    let mut dirs: Vec<(PathBuf, bool)> = Vec::new();
    for spec in &specs {
        dirs.push((resolve_repo(spec, env)?, true));
    }
    if let Some(source) = from {
        let text = if source == "-" {
            let mut text = String::new();
            stdin.read_to_string(&mut text)?;
            text
        } else {
            std::fs::read_to_string(env.cwd.join(expand(source, env.home.as_deref())))
                .map_err(|e| Failure::Run(format!("cannot read '{source}': {e}")))?
        };
        let listed: Vec<PathBuf> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| env.cwd.join(expand(line, env.home.as_deref())))
            .collect();
        if listed.is_empty() {
            let _ = writeln!(err, "gitmaster: the list of repositories is empty");
        }
        dirs.extend(listed.into_iter().map(|dir| (dir, false)));
    }
    // A list the caller gave stays the whole answer, even when it is empty.
    if dirs.is_empty() && from.is_none() {
        dirs.push((env.cwd.clone(), true));
    }
    let mut repos: Vec<Repo> = Vec::new();
    for (dir, explicit) in dirs {
        let (root, entries) = match worktrees_in(&dir, git) {
            Ok(found) => found,
            Err(error) if explicit => return Err(error),
            Err(_) => continue,
        };
        if repos
            .iter()
            .any(|repo| repo.entries[0].path == entries[0].path)
        {
            continue;
        }
        let project = Path::new(&entries[0].path)
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        repos.push(Repo {
            project,
            root,
            entries,
        });
    }
    Ok((repos, multi))
}

/// The one repository a command that changes or looks up something works on: `--repo`, else the
/// one around the current directory.
fn one_repo(args: &Args, env: &Env) -> Result<PathBuf, Failure> {
    if args.option("repos-from").is_some() || args.values("repo").len() > 1 {
        return Err(Failure::Usage(
            "this command works on one repository: give --repo once".into(),
        ));
    }
    match args.values("repo").first() {
        Some(spec) => resolve_repo(spec, env),
        None => Ok(env.cwd.clone()),
    }
}

fn list(
    words: &[String],
    env: &Env,
    git: &dyn GitPort,
    stdin: &mut dyn Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), Failure> {
    let args = Args::parse(words, &["format", "repo", "repos-from"], &["linked"])?;
    args.exactly(0, "no arguments")?;
    let format = args.option("format").unwrap_or("paths");
    if !["paths", "branches", "tsv", "json"].contains(&format) {
        return Err(Failure::Usage(format!("unknown format '{format}'")));
    }
    let (repos, multi) = repositories(&args, env, git, stdin, err)?;
    let mut objects = Vec::new();
    for repo in &repos {
        for entry in repo
            .entries
            .iter()
            .filter(|entry| !(args.switch("linked") && entry.is_main))
        {
            let branch = entry.branch.as_deref();
            let project = if multi {
                format!("{}\t", repo.project)
            } else {
                String::new()
            };
            match format {
                "paths" => writeln!(out, "{}", entry.path)?,
                "branches" => {
                    if let Some(branch) = branch {
                        writeln!(out, "{project}{branch}")?;
                    }
                }
                "tsv" => writeln!(
                    out,
                    "{project}{}\t{}",
                    branch.unwrap_or("(detached)"),
                    entry.path
                )?,
                _ => {
                    let mut object =
                        serde_json::to_value(entry).map_err(|e| Failure::Run(e.to_string()))?;
                    if multi && let Some(fields) = object.as_object_mut() {
                        fields.insert("project".into(), repo.project.clone().into());
                        fields.insert("repo".into(), repo.entries[0].path.clone().into());
                    }
                    objects.push(object);
                }
            }
        }
    }
    if format == "json" {
        let json = serde_json::to_string(&objects).map_err(|e| Failure::Run(e.to_string()))?;
        writeln!(out, "{json}")?;
    }
    Ok(())
}

/// Every branch a worktree could be made for: the local branches, then the remote ones that have
/// no local branch of the same name, each once. With several repositories a line is the project,
/// the branch and the main checkout, separated by tabs.
fn branches(
    words: &[String],
    env: &Env,
    git: &dyn GitPort,
    stdin: &mut dyn Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), Failure> {
    let args = Args::parse(words, &["repo", "repos-from"], &["missing"])?;
    args.exactly(0, "no arguments")?;
    let (repos, multi) = repositories(&args, env, git, stdin, err)?;
    for repo in &repos {
        let mut names: Vec<String> = Vec::new();
        for branch in git.branches(&repo.root)? {
            let name = if branch.is_remote {
                // `origin/feature/x` is the branch `feature/x`.
                branch
                    .name
                    .split_once('/')
                    .map_or(branch.name.clone(), |(_, rest)| rest.to_owned())
            } else {
                branch.name
            };
            if !names.contains(&name) {
                names.push(name);
            }
        }
        for name in names {
            let has_worktree = repo
                .entries
                .iter()
                .any(|entry| entry.branch.as_deref() == Some(name.as_str()));
            if args.switch("missing") && has_worktree {
                continue;
            }
            if multi {
                writeln!(out, "{}\t{name}\t{}", repo.project, repo.entries[0].path)?;
            } else {
                writeln!(out, "{name}")?;
            }
        }
    }
    Ok(())
}

fn path(
    words: &[String],
    env: &Env,
    git: &dyn GitPort,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let args = Args::parse(words, &["repo"], &[])?;
    args.exactly(1, "a branch")?;
    let branch = &args.positional[0];
    let (_, entries) = worktrees_in(&one_repo(&args, env)?, git)?;
    match holder(&entries, branch) {
        Some(entry) => {
            writeln!(out, "{}", entry.path)?;
            Ok(())
        }
        None => Err(Failure::Run(format!(
            "no worktree has branch '{branch}' checked out"
        ))),
    }
}

fn holder<'a>(entries: &'a [WorktreeEntry], branch: &str) -> Option<&'a WorktreeEntry> {
    entries
        .iter()
        .find(|entry| entry.branch.as_deref() == Some(branch))
}

/// The last value of the git config `key`.
fn config(git: &dyn GitPort, root: &Path, key: &str) -> Result<Option<String>, Failure> {
    Ok(git
        .config_values(root, key)?
        .into_iter()
        .rfind(|value| !value.is_empty()))
}

/// `path` with a leading `~` replaced by the home directory.
fn expand(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

/// Where the worktree of `branch` goes.
///
/// The folder comes from `--root`, else `gitmaster.worktree.root`, else `WORKTREES_HOME`. Under
/// it the worktree is `<project>/<branch>`, with `/` in the branch written as `-`. Without any of
/// them it is a sibling of the main checkout, `<project>-<branch>` (D31).
fn worktree_dir(
    env: &Env,
    git: &dyn GitPort,
    root: &Path,
    main: &Path,
    branch: &str,
    flag: Option<&str>,
) -> Result<PathBuf, Failure> {
    let project = main
        .file_name()
        .map_or_else(|| "repo".into(), |name| name.to_string_lossy().into_owned());
    let leaf = branch.replace('/', "-");
    let base = if let Some(flag) = flag {
        Some(env.cwd.join(expand(flag, env.home.as_deref())))
    } else if let Some(value) = config(git, root, ROOT_KEY)? {
        Some(main.join(expand(&value, env.home.as_deref())))
    } else {
        env.worktrees_home.clone()
    };
    Ok(match base {
        Some(base) => base.join(&project).join(leaf),
        None => main
            .parent()
            .unwrap_or(main)
            .join(format!("{project}-{leaf}")),
    })
}

/// The mode from `--include`, else `gitmaster.worktree.include`, else symbolic links.
fn include_mode(git: &dyn GitPort, root: &Path, flag: Option<&str>) -> Result<Mode, Failure> {
    let (name, source) = match flag {
        Some(flag) => (Some(flag.to_owned()), "--include"),
        None => (config(git, root, INCLUDE_KEY)?, INCLUDE_KEY),
    };
    match name {
        None => Ok(Mode::Symlink),
        Some(name) => Mode::parse(&name).ok_or_else(|| {
            Failure::Usage(format!(
                "{source}: '{name}' is not symlink, reflink or none"
            ))
        }),
    }
}

fn add(
    words: &[String],
    env: &Env,
    git: &dyn GitPort,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), Failure> {
    let args = Args::parse(words, &["root", "include", "repo"], &[])?;
    if args.positional.is_empty() || args.positional.len() > 2 {
        return Err(Failure::Usage(
            "expected a branch and an optional base".into(),
        ));
    }
    let branch = args.positional[0].as_str();
    let base = args.positional.get(1).map(String::as_str);
    if branch.is_empty() || branch.starts_with('-') || !valid_branch_name(branch) {
        return Err(Failure::Run(format!(
            "'{branch}' is not a valid branch name"
        )));
    }
    let (root, entries) = worktrees_in(&one_repo(&args, env)?, git)?;
    let mode = include_mode(git, &root, args.option("include"))?;

    // A branch is checked out in one worktree only: reuse it wherever it is.
    if let Some(entry) = holder(&entries, branch) {
        writeln!(out, "{}", entry.path)?;
        return Ok(());
    }

    let main = PathBuf::from(&entries[0].path);
    let dir = worktree_dir(env, git, &root, &main, branch, args.option("root"))?;
    if dir.exists() {
        return Err(Failure::Run(format!("'{}' already exists", dir.display())));
    }
    let known = git.branches(&root)?;
    let local = known.iter().any(|b| !b.is_remote && b.name == branch);
    let kind = if local {
        if base.is_some() {
            writeln!(
                err,
                "gitmaster: branch '{branch}' exists; ignoring the base"
            )?;
        }
        WorktreeMode::Existing {
            branch: branch.to_owned(),
        }
    } else if let Some(remote) = base
        .is_none()
        .then(|| remote_branch(&known, branch))
        .flatten()
    {
        // Git sets the new branch to track a remote-tracking start point.
        WorktreeMode::New {
            branch: branch.to_owned(),
            base: remote,
        }
    } else {
        WorktreeMode::New {
            branch: branch.to_owned(),
            base: base.unwrap_or("HEAD").to_owned(),
        }
    };
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    git.create_worktree(&root, &dir.to_string_lossy(), &kind)?;

    share(git, &main, &dir, mode, false, err);
    writeln!(out, "{}", dir.display())?;
    Ok(())
}

/// Whether `git check-ref-format --branch` accepts `name`.
fn valid_branch_name(name: &str) -> bool {
    Command::new("git")
        .args(["check-ref-format", "--branch", name])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// The remote-tracking branch for `branch`, `origin/…` first.
fn remote_branch(known: &[gm_schema::BranchEntry], branch: &str) -> Option<String> {
    let suffix = format!("/{branch}");
    let mut found: Vec<&str> = known
        .iter()
        .filter(|b| b.is_remote)
        .filter_map(|b| {
            let remote = b.name.strip_suffix(&suffix)?;
            (!remote.contains('/')).then_some(b.name.as_str())
        })
        .collect();
    found.sort_by_key(|name| !name.starts_with("origin/"));
    found.first().map(|name| (*name).to_owned())
}

/// Shares the `.worktreeinclude` paths of `main` into `target` and reports each on `err`. A
/// problem here never fails the command: the worktree exists.
fn share(
    git: &dyn GitPort,
    main: &Path,
    target: &Path,
    mode: Mode,
    dry_run: bool,
    err: &mut dyn Write,
) {
    if mode == Mode::None {
        return;
    }
    let file = main.join(worktree_include::FILE_NAME);
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    let result = worktree_include::Matcher::new(main, &text)
        .and_then(|matcher| {
            worktree_include::ignored_entries(main)
                .map(|ignored| worktree_include::select(main, &matcher, &ignored))
        })
        .map(|entries| worktree_include::apply(main, target, &entries, mode, dry_run));
    let applied = match result {
        Ok(applied) => applied,
        Err(error) => {
            let _ = writeln!(
                err,
                "gitmaster: {}: {}",
                worktree_include::FILE_NAME,
                error.0
            );
            return;
        }
    };
    for item in &applied {
        let slash = if item.entry.is_dir { "/" } else { "" };
        let path = &item.entry.path;
        let _ = match (&item.action, dry_run) {
            (Action::Symlinked, false) => writeln!(err, "linked {path}{slash}"),
            (Action::Symlinked, true) => writeln!(err, "would link {path}{slash}"),
            (Action::Reflinked, false) => writeln!(err, "cloned {path}{slash}"),
            (Action::Reflinked, true) => writeln!(err, "would clone {path}{slash}"),
            (Action::SymlinkedInstead, _) => {
                writeln!(err, "linked {path}{slash} (this file system cannot clone)")
            }
            (Action::Skipped(why), _) => writeln!(err, "skipped {path}{slash}: {why}"),
            (Action::Failed(why), _) => writeln!(err, "gitmaster: {path}{slash}: {why}"),
        };
    }
    if !dry_run {
        let dirs = worktree_include::linked_dirs(&applied);
        let written = git
            .common_dir(main)
            .map_err(|e| std::io::Error::other(e.0))
            .and_then(|common| worktree_include::ensure_excludes(&common, &dirs));
        if let Err(error) = written {
            let _ = writeln!(err, "gitmaster: cannot update info/exclude: {error}");
        }
    }
}

/// Removes the worktree that has `<branch>` checked out; the branch is kept. The rules are the
/// app's (D31): the main checkout and the worktree the command runs in are never removed, and a
/// worktree that is locked, has changes or has commits no other branch has needs `--force`.
fn remove(
    words: &[String],
    env: &Env,
    git: &dyn GitPort,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), Failure> {
    let args = Args::parse(words, &["repo"], &["force"])?;
    args.exactly(1, "a branch")?;
    let branch = &args.positional[0];
    // `worktrees` leaves out a worktree whose folder is gone: that one is pruned, not removed.
    let (_, entries) = worktrees_in(&one_repo(&args, env)?, git)?;
    let entry = holder(&entries, branch)
        .ok_or_else(|| Failure::Run(format!("no worktree has branch '{branch}' checked out")))?;
    let path = PathBuf::from(&entry.path);
    let here = std::fs::canonicalize(&env.cwd).unwrap_or_else(|_| env.cwd.clone());
    let there = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());

    let status = git.status(&path)?;
    let changes = status
        .entries
        .iter()
        .any(|c| c.staged.is_some() || c.unstaged.is_some() || c.untracked);
    let unpushed = git.unpushed_commits(&path, Some(branch))?;
    let found = crate::worktree::blockers([
        (entry.is_main, Blocker::Main),
        (here.starts_with(&there), Blocker::Current),
        (false, Blocker::Missing),
        (entry.locked.is_some(), Blocker::Locked),
        (changes, Blocker::Dirty),
        (unpushed > 0, Blocker::Unpushed),
    ]);
    if found.contains(&Blocker::Main) {
        return Err(Failure::Run(format!(
            "'{branch}' is checked out in the main checkout"
        )));
    }
    if found.contains(&Blocker::Current) {
        return Err(Failure::Run(
            "this directory is inside that worktree; run the command from another one".into(),
        ));
    }
    if !found.is_empty() && !args.switch("force") {
        let why: Vec<String> = found
            .iter()
            .map(|b| match b {
                Blocker::Locked => "it is locked".to_owned(),
                Blocker::Dirty => "it has uncommitted or untracked files".to_owned(),
                _ => format!("it has {unpushed} commit(s) that no other branch or remote has"),
            })
            .collect();
        return Err(Failure::Run(format!(
            "not removing {}: {}; use --force to discard them",
            path.display(),
            why.join(", ")
        )));
    }
    let force = if found.contains(&Blocker::Locked) {
        2
    } else {
        u8::from(!found.is_empty())
    };
    // From the main checkout, so the folder being removed is never the one git runs in.
    let main = PathBuf::from(&entries[0].path);
    git.remove_worktree(&main, &entry.path, force)?;
    // An empty `<root>/<project>` folder is left behind by the layout; the sibling layout shares
    // the main checkout's parent, which is not ours to tidy.
    if let Some(parent) = path.parent()
        && Some(parent) != main.parent()
    {
        let _ = std::fs::remove_dir(parent);
    }
    writeln!(out, "{}", path.display())?;
    writeln!(
        err,
        "removed worktree {} (branch '{branch}' kept)",
        path.display()
    )?;
    Ok(())
}

fn include(
    words: &[String],
    env: &Env,
    git: &dyn GitPort,
    err: &mut dyn Write,
) -> Result<(), Failure> {
    let args = Args::parse(words, &["include", "repo"], &["dry-run"])?;
    if args.positional.len() > 1 {
        return Err(Failure::Usage("expected at most one branch".into()));
    }
    let (root, entries) = worktrees_in(&one_repo(&args, env)?, git)?;
    let main = PathBuf::from(&entries[0].path);
    let target = match args.positional.first() {
        Some(branch) => holder(&entries, branch)
            .map(|entry| PathBuf::from(&entry.path))
            .ok_or_else(|| {
                Failure::Run(format!("no worktree has branch '{branch}' checked out"))
            })?,
        None => root,
    };
    if target == main {
        return Err(Failure::Run("that is the main checkout".into()));
    }
    let mode = include_mode(git, &target, args.option("include"))?;
    if mode == Mode::None {
        return Err(Failure::Usage("--include none shares nothing".into()));
    }
    share(git, &main, &target, mode, args.switch("dry-run"), err);
    Ok(())
}

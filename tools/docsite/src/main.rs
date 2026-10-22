//! `docsite`: checks the documentation in `docs/` and builds it into a static website.
//!
//!   docsite check  [--docs DIR] [--strict]
//!   docsite status [--docs DIR]
//!   docsite build  [--docs DIR] [--out DIR] [--base-url URL] [--version LABEL]
//!
//! `docs/README.md` is the authoring guide: the rules `check` enforces and the status markers
//! `status` counts.

mod check;
mod frontmatter;
mod highlight;
mod load;
mod markdown;
mod model;
mod site;
mod status;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use model::{Diagnostics, Docs, Severity};

const USAGE: &str = "usage:
  docsite check  [--docs DIR] [--strict]    verify the pages against the authoring rules
  docsite status [--docs DIR]               show which parts of the docs the app does not do yet
  docsite build  [--docs DIR] [--out DIR] [--base-url URL] [--version LABEL]
                                            write the static site (default out: <repo>/build/site)

--docs defaults to the docs/ directory above the current one (the one with sections.toml).";

#[derive(Default)]
struct Args {
    docs: Option<PathBuf>,
    out: Option<PathBuf>,
    base_url: Option<String>,
    version: Option<String>,
    strict: bool,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args::default();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--docs" => parsed.docs = Some(PathBuf::from(value("--docs")?)),
            "--out" => parsed.out = Some(PathBuf::from(value("--out")?)),
            "--base-url" => parsed.base_url = Some(value("--base-url")?),
            "--version" => parsed.version = Some(value("--version")?),
            "--strict" => parsed.strict = true,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(parsed)
}

/// The `docs/` directory: the one given, or the nearest above the current directory.
fn find_docs(given: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(dir) = given {
        return Ok(dir);
    }
    let mut dir = std::env::current_dir().map_err(|e| e.to_string())?;
    loop {
        let candidate = dir.join("docs");
        if candidate.join("sections.toml").is_file() {
            return Ok(candidate);
        }
        if !dir.pop() {
            return Err(
                "cannot find docs/sections.toml; run from the repository or pass --docs".to_owned(),
            );
        }
    }
}

fn read_docs(
    docs_dir: &Path,
    diags: &mut Diagnostics,
) -> Result<(Docs, Vec<markdown::Analysis>), String> {
    let docs = load::load(docs_dir, diags)?;
    let analyses = check::check(&docs, diags);
    Ok((docs, analyses))
}

fn print_diagnostics(diags: &Diagnostics) {
    for diagnostic in &diags.0 {
        eprintln!("{diagnostic}");
    }
    eprintln!(
        "{} error(s), {} warning(s)",
        diags.errors(),
        diags.warnings()
    );
}

fn run() -> Result<ExitCode, String> {
    let mut raw = std::env::args().skip(1);
    let Some(command) = raw.next() else {
        eprintln!("{USAGE}");
        return Ok(ExitCode::from(2));
    };
    if matches!(command.as_str(), "-h" | "--help" | "help") {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let args = parse(raw).map_err(|e| format!("{e}\n{USAGE}"))?;
    let docs_dir = find_docs(args.docs.clone())?;
    let mut diags = Diagnostics::default();
    let (docs, analyses) = read_docs(&docs_dir, &mut diags)?;

    match command.as_str() {
        "check" => {
            print_diagnostics(&diags);
            let failed = diags.errors() > 0 || (args.strict && diags.warnings() > 0);
            Ok(if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        "status" => {
            print!("{}", status::report(&docs, &analyses));
            if diags.errors() > 0 {
                print_diagnostics(&diags);
                return Ok(ExitCode::FAILURE);
            }
            Ok(ExitCode::SUCCESS)
        }
        "build" => {
            if diags.0.iter().any(|d| d.severity == Severity::Error) {
                print_diagnostics(&diags);
                eprintln!("not building: fix the errors above");
                return Ok(ExitCode::FAILURE);
            }
            if diags.warnings() > 0 {
                print_diagnostics(&diags);
            }
            let out = args.out.unwrap_or_else(|| {
                docs_dir.parent().map_or_else(
                    || PathBuf::from("build/site"),
                    |root| root.join("build/site"),
                )
            });
            let options = site::Options {
                out,
                base_url: args.base_url,
                version: args
                    .version
                    .unwrap_or_else(|| "development build".to_owned()),
            };
            site::build(&docs, &analyses, &options)?;
            println!(
                "wrote {} pages to {}",
                docs.pages.len() + 2,
                options.out.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown command `{other}`\n{USAGE}")),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("docsite: {message}");
            ExitCode::from(2)
        }
    }
}

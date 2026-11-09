//! Reviewer definitions (D189, D212 (1)). A reviewer is a Markdown file, `<name>.md`, with a
//! front matter of flat `key: value` lines and the instructions as the body. The repository's
//! copy lives in `.git/gitmaster/reviewers/` and the user's in `reviewers/` of the configuration
//! directory; the repository's wins by name. Presets are built in and are templates: nothing
//! here reads them from disk.
//!
//! The format is ours and small on purpose (no TOML or YAML crate). Nothing is written into a
//! working tree.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use gm_schema::{Reviewer, ReviewerDirection, ReviewerSignal, ReviewerSource, ReviewerSubject};

use crate::glob;

/// The longest name, in characters.
const NAME_LIMIT: usize = 40;
/// The longest value of a front matter key, in characters.
const VALUE_LIMIT: usize = 200;
/// The longest instructions, in characters.
const INSTRUCTIONS_LIMIT: usize = 8000;
/// The largest file `load` reads; a bigger one is reported and skipped.
const FILE_LIMIT: u64 = 64 * 1024;
/// The most patterns one of the `paths` or `branches` lines holds (D214 (1)).
const PATTERN_COUNT_LIMIT: usize = 20;
/// The front matter keys, in the order `render` writes them.
const KEYS: [&str; 7] = [
    "description",
    "signal",
    "direction",
    "subject",
    "model",
    "paths",
    "branches",
];

/// The reviewers that were read, by name, and the files that could not be read as reviewers,
/// as `(file, reason)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedReviewers {
    pub reviewers: Vec<Reviewer>,
    pub errors: Vec<(String, String)>,
}

/// The directory of the repository's reviewers under its common Git directory.
#[must_use]
pub fn repo_dir(common_dir: &Path) -> PathBuf {
    common_dir.join("gitmaster").join("reviewers")
}

/// The directory of the user's reviewers under the configuration directory.
#[must_use]
pub fn user_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("reviewers")
}

/// Checks a reviewer name: 1 to 40 characters of `a-z`, `0-9` and `-`, not starting or ending
/// with `-`.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > NAME_LIMIT {
        return Err(format!(
            "a reviewer name is 1 to {NAME_LIMIT} characters, not {}",
            name.len()
        ));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(format!(
            "`{name}` is not a reviewer name: use a-z, 0-9 and '-' only"
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(format!("`{name}` cannot start or end with '-'"));
    }
    Ok(())
}

/// Reads a reviewer from its file text. `name` is the file stem; `source` says where the text
/// came from. Every error names the line it is about, or the field that is missing.
pub fn parse(name: &str, text: &str, source: ReviewerSource) -> Result<Reviewer, String> {
    validate_name(name)?;
    let mut lines = text.lines().map(|line| line.trim_end_matches('\r'));
    if lines.next() != Some("---") {
        return Err("line 1: the file must start with a `---` line".to_owned());
    }
    let mut fields: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
    let mut closing = None;
    for (index, line) in lines.by_ref().enumerate() {
        let number = index + 2;
        if line == "---" {
            closing = Some(number);
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("line {number}: expected `key: value`"));
        };
        let (key, value) = (key.trim(), value.trim());
        if !KEYS.contains(&key) {
            return Err(format!("line {number}: unknown key `{key}`"));
        }
        if fields.insert(key, (number, value)).is_some() {
            return Err(format!("line {number}: duplicate key `{key}`"));
        }
        if value.chars().count() > VALUE_LIMIT {
            return Err(format!(
                "line {number}: `{key}` is over {VALUE_LIMIT} characters"
            ));
        }
    }
    let Some(closing) = closing else {
        return Err("the front matter is not closed: add a `---` line after it".to_owned());
    };
    let body: Vec<&str> = lines.collect();
    let instructions = body.join("\n");
    let instructions = instructions.trim();
    if instructions.is_empty() {
        return Err(format!(
            "line {}: the instructions are empty: write them after the front matter",
            closing + 1
        ));
    }
    if instructions.chars().count() > INSTRUCTIONS_LIMIT {
        return Err(format!(
            "line {}: the instructions are over {INSTRUCTIONS_LIMIT} characters",
            closing + 1
        ));
    }

    let description = required(&fields, "description")?;
    let signal = match required(&fields, "signal")? {
        "score" => ReviewerSignal::Score,
        "level" => ReviewerSignal::Level,
        "pass" => ReviewerSignal::Pass,
        _ => {
            return Err(line_error(
                &fields,
                "signal",
                "must be score, level or pass",
            ));
        }
    };
    let direction = direction_of(&fields)?;
    let subject = subject_of(&fields)?;
    let model = optional(&fields, "model")
        .filter(|model| !model.is_empty())
        .map(str::to_owned);
    let paths = patterns(&fields, "paths", glob::check_path_pattern)?;
    let branches = patterns(&fields, "branches", glob::check_pattern)?;

    Ok(Reviewer {
        name: name.to_owned(),
        description: description.to_owned(),
        instructions: instructions.to_owned(),
        signal,
        direction,
        subject,
        model,
        source,
        paths,
        branches,
    })
}

/// The file text of a reviewer. `parse` reads it back as the same reviewer.
#[must_use]
pub fn render(reviewer: &Reviewer) -> String {
    let mut text = format!(
        "---\ndescription: {}\nsignal: {}\ndirection: {}\nsubject: {}\n",
        reviewer.description,
        signal_text(reviewer.signal),
        direction_text(reviewer.direction),
        subject_text(reviewer.subject),
    );
    if let Some(model) = &reviewer.model {
        text.push_str("model: ");
        text.push_str(model);
        text.push('\n');
    }
    for (key, list) in [("paths", &reviewer.paths), ("branches", &reviewer.branches)] {
        if !list.is_empty() {
            text.push_str(key);
            text.push_str(": ");
            text.push_str(&list.join(", "));
            text.push('\n');
        }
    }
    text.push_str("---\n\n");
    text.push_str(&reviewer.instructions);
    text.push('\n');
    text
}

/// The five built-in reviewers. They are templates: a user copies one to a file and edits it.
#[must_use]
pub fn presets() -> Vec<Reviewer> {
    vec![
        preset(
            "code-review",
            "Looks for correctness bugs and missing edge cases in the branch",
            ReviewerSignal::Score,
            ReviewerDirection::HigherIsBetter,
            &[
                "Review the changes on this branch as a careful senior engineer would.",
                "Look for bugs: wrong logic, off-by-one errors, unhandled errors, races and missing edge cases.",
                "Report only what you can point to in the changed code, with its file, line and the reason.",
                "Score from 0 to 100: 100 means nothing worrying remains, lower means more or worse problems.",
            ],
        ),
        preset(
            "security",
            "Looks for security weaknesses the branch introduces",
            ReviewerSignal::Level,
            ReviewerDirection::LowerIsBetter,
            &[
                "Review the changes on this branch for security weaknesses that they introduce.",
                "Check input handling, injection (shell, SQL, path, template), secrets written into code or logs, authorization checks and unsafe deserialization.",
                "Ignore problems that were there before the branch and that the branch does not touch.",
                "Answer with a level: low when nothing needs attention, medium when a weakness needs a fix before merge, high when the change exposes data or allows an attack.",
            ],
        ),
        preset(
            "slop",
            "Flags generated filler: needless wrappers, comments that restate code, tests that assert nothing",
            ReviewerSignal::Score,
            ReviewerDirection::LowerIsBetter,
            &[
                "Judge whether the changes on this branch read like careful human work or like generated filler.",
                "Flag needless wrappers, dead code, comments that restate the code, names that say nothing and tests that assert nothing.",
                "Score from 0 to 100, where a higher score means more filler, so lower is better.",
            ],
        ),
        preset(
            "intent-drift",
            "Checks that the changes do what the commits and the description say",
            ReviewerSignal::Level,
            ReviewerDirection::LowerIsBetter,
            &[
                "Compare what this branch changes with what its commit messages say it does.",
                "Flag work that the stated intent does not ask for, and stated intent that the code does not deliver.",
                "Answer with a level: low when the changes match the intent, medium when some change goes beyond or falls short of it, high when the branch does something other than what it says.",
            ],
        ),
        preset(
            "risk",
            "Scores how risky the branch is to ship: blast radius, reversibility and test coverage",
            ReviewerSignal::Score,
            ReviewerDirection::LowerIsBetter,
            &[
                "Estimate how risky this branch is to merge and ship.",
                "Consider how much code depends on the changed parts, whether a change is easy to undo, whether data or schema change, and whether the changed code has tests.",
                "Score from 0 to 100, where a higher score means riskier, so lower is better.",
            ],
        ),
    ]
}

/// Reads the reviewers in `repo_dir` and `user_dir`; either may be absent. Each directory is
/// read directly (not recursively): every `*.md` file whose stem is a valid name. Symlinks are
/// skipped; a file over 64 KiB or one that does not parse is reported in `errors` and does not
/// stop the others. A repository reviewer wins over a user one of the same name. The result is
/// sorted by name.
#[must_use]
pub fn load(repo_dir: Option<&Path>, user_dir: Option<&Path>) -> LoadedReviewers {
    let mut by_name: BTreeMap<String, Reviewer> = BTreeMap::new();
    let mut errors = Vec::new();
    for (dir, source) in [
        (repo_dir, ReviewerSource::Repository),
        (user_dir, ReviewerSource::User),
    ] {
        if let Some(dir) = dir {
            read_dir_into(dir, source, &mut by_name, &mut errors);
        }
    }
    errors.sort();
    LoadedReviewers {
        reviewers: by_name.into_values().collect(),
        errors,
    }
}

/// Writes `reviewer` as `<name>.md` in `dir`, creating `dir`. The text is checked first: it
/// must parse back as the same reviewer. The file is replaced in one rename, so a reader sees
/// the old one or the new one.
pub fn save(dir: &Path, reviewer: &Reviewer) -> Result<(), String> {
    validate_name(&reviewer.name)?;
    let text = render(reviewer);
    let back = parse(&reviewer.name, &text, reviewer.source)
        .map_err(|e| format!("cannot save {}: {e}", reviewer.name))?;
    if back != *reviewer {
        return Err(format!(
            "cannot save {}: a field would not be read back as it was written",
            reviewer.name
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let mut file = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| format!("cannot write in {}: {e}", dir.display()))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|e| format!("cannot write in {}: {e}", dir.display()))?;
    let target = dir.join(format!("{}.md", reviewer.name));
    file.persist(&target)
        .map_err(|e| format!("cannot save {}: {}", target.display(), e.error))?;
    Ok(())
}

/// Removes `<name>.md` from `dir`.
pub fn delete(dir: &Path, name: &str) -> Result<(), String> {
    validate_name(name)?;
    let target = dir.join(format!("{name}.md"));
    std::fs::remove_file(&target).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            format!("there is no reviewer {name}")
        } else {
            format!("cannot delete {}: {e}", target.display())
        }
    })
}

/// Adds the reviewers of one directory to `by_name`, keeping any already there.
fn read_dir_into(
    dir: &Path,
    source: ReviewerSource,
    by_name: &mut BTreeMap<String, Reviewer>,
    errors: &mut Vec<(String, String)>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            errors.push((
                dir.display().to_string(),
                format!("cannot read the directory: {e}"),
            ));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension() != Some(OsStr::new("md")) {
            continue;
        }
        let file = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            errors.push((file, "cannot read the file".to_owned()));
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }
        if meta.len() > FILE_LIMIT {
            errors.push((file, "the file is over 64 KiB".to_owned()));
            continue;
        }
        let Some(name) = path.file_stem().and_then(OsStr::to_str) else {
            errors.push((file, "the file name is not UTF-8".to_owned()));
            continue;
        };
        let read = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read the file: {e}"))
            .and_then(|text| parse(name, &text, source));
        match read {
            Ok(reviewer) => {
                by_name.entry(name.to_owned()).or_insert(reviewer);
            }
            Err(reason) => errors.push((file, reason)),
        }
    }
}

/// The value of a field that must be present and not empty.
fn required<'a>(fields: &BTreeMap<&str, (usize, &'a str)>, key: &str) -> Result<&'a str, String> {
    match fields.get(key) {
        Some((_, value)) if !value.is_empty() => Ok(value),
        _ => Err(format!("`{key}` is required in the front matter")),
    }
}

/// The value of a field that may be absent or empty.
fn optional<'a>(fields: &BTreeMap<&str, (usize, &'a str)>, key: &str) -> Option<&'a str> {
    fields.get(key).map(|(_, value)| *value)
}

/// The comma-separated patterns of a field, each checked by `check`. A pattern cannot contain a
/// comma, and an absent or empty field has none.
fn patterns(
    fields: &BTreeMap<&str, (usize, &str)>,
    key: &str,
    check: fn(&str) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    let Some(&(line, value)) = fields.get(key) else {
        return Ok(Vec::new());
    };
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let items: Vec<&str> = value.split(',').map(str::trim).collect();
    if items.len() > PATTERN_COUNT_LIMIT {
        return Err(format!(
            "line {line}: `{key}` has {} patterns, the most is {PATTERN_COUNT_LIMIT}",
            items.len()
        ));
    }
    let mut patterns = Vec::with_capacity(items.len());
    for item in items {
        if item.is_empty() {
            return Err(format!("line {line}: `{key}` has an empty pattern"));
        }
        check(item).map_err(|reason| format!("line {line}: `{key}` pattern `{item}`: {reason}"))?;
        patterns.push(item.to_owned());
    }
    Ok(patterns)
}

/// A reason for a field, naming the field's line.
fn line_error(fields: &BTreeMap<&str, (usize, &str)>, key: &str, reason: &str) -> String {
    let line = fields.get(key).map_or(0, |(line, _)| *line);
    format!("line {line}: `{key}` {reason}")
}

/// The `direction` field: higher is better when absent.
fn direction_of(fields: &BTreeMap<&str, (usize, &str)>) -> Result<ReviewerDirection, String> {
    match optional(fields, "direction") {
        None | Some("higher_is_better") => Ok(ReviewerDirection::HigherIsBetter),
        Some("lower_is_better") => Ok(ReviewerDirection::LowerIsBetter),
        Some(_) => Err(line_error(
            fields,
            "direction",
            "must be higher_is_better or lower_is_better",
        )),
    }
}

/// The `subject` field: a branch when absent.
fn subject_of(fields: &BTreeMap<&str, (usize, &str)>) -> Result<ReviewerSubject, String> {
    match optional(fields, "subject") {
        None | Some("branch") => Ok(ReviewerSubject::Branch),
        Some("staged") => Ok(ReviewerSubject::Staged),
        Some("commit") => Ok(ReviewerSubject::Commit),
        Some(_) => Err(line_error(
            fields,
            "subject",
            "must be branch, staged or commit",
        )),
    }
}

fn signal_text(signal: ReviewerSignal) -> &'static str {
    match signal {
        ReviewerSignal::Score => "score",
        ReviewerSignal::Level => "level",
        ReviewerSignal::Pass => "pass",
    }
}

fn direction_text(direction: ReviewerDirection) -> &'static str {
    match direction {
        ReviewerDirection::HigherIsBetter => "higher_is_better",
        ReviewerDirection::LowerIsBetter => "lower_is_better",
    }
}

fn subject_text(subject: ReviewerSubject) -> &'static str {
    match subject {
        ReviewerSubject::Branch => "branch",
        ReviewerSubject::Staged => "staged",
        ReviewerSubject::Commit => "commit",
    }
}

fn preset(
    name: &str,
    description: &str,
    signal: ReviewerSignal,
    direction: ReviewerDirection,
    instructions: &[&str],
) -> Reviewer {
    Reviewer {
        name: name.to_owned(),
        description: description.to_owned(),
        instructions: instructions.join("\n"),
        signal,
        direction,
        subject: ReviewerSubject::Branch,
        model: Some("haiku".to_owned()),
        source: ReviewerSource::Preset,
        paths: Vec::new(),
        branches: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const SAMPLE: &str = "---\ndescription: Looks for leaks\nsignal: score\n\
        direction: lower_is_better\nsubject: staged\nmodel: haiku\n---\n\nFind leaks.\nBe brief.\n";

    fn sample() -> Reviewer {
        parse("leaks", SAMPLE, ReviewerSource::User).expect("sample parses")
    }

    fn error_of(name: &str, text: &str) -> String {
        parse(name, text, ReviewerSource::User).expect_err("refused")
    }

    #[test]
    fn a_file_reads_into_a_reviewer() {
        let reviewer = sample();
        assert_eq!(reviewer.name, "leaks");
        assert_eq!(reviewer.description, "Looks for leaks");
        assert_eq!(reviewer.instructions, "Find leaks.\nBe brief.");
        assert_eq!(reviewer.signal, ReviewerSignal::Score);
        assert_eq!(reviewer.direction, ReviewerDirection::LowerIsBetter);
        assert_eq!(reviewer.subject, ReviewerSubject::Staged);
        assert_eq!(reviewer.model.as_deref(), Some("haiku"));
        assert_eq!(reviewer.source, ReviewerSource::User);
    }

    #[test]
    fn the_defaults_are_higher_is_better_and_branch() {
        let text = "---\ndescription: d\nsignal: level\n---\nDo it.\n";
        let reviewer = parse("plain", text, ReviewerSource::Repository).unwrap();
        assert_eq!(reviewer.direction, ReviewerDirection::HigherIsBetter);
        assert_eq!(reviewer.subject, ReviewerSubject::Branch);
        assert_eq!(reviewer.model, None);
    }

    #[test]
    fn a_crlf_file_reads_like_an_lf_one() {
        let text = SAMPLE.replace('\n', "\r\n");
        assert_eq!(
            parse("leaks", &text, ReviewerSource::User).unwrap(),
            sample()
        );
    }

    #[test]
    fn an_empty_model_is_no_model() {
        let text = "---\ndescription: d\nsignal: pass\nmodel:\n---\nDo it.\n";
        let reviewer = parse("gate", text, ReviewerSource::User).unwrap();
        assert_eq!(reviewer.model, None);
    }

    #[test]
    fn paths_and_branches_are_comma_separated_and_checked() {
        let text = "---\ndescription: d\nsignal: pass\n\
            paths: services/billing/**, api/*.rs\nbranches: feat/billing-*\n---\nCheck it.\n";
        let reviewer = parse("billing", text, ReviewerSource::Repository).unwrap();
        assert_eq!(reviewer.paths, ["services/billing/**", "api/*.rs"]);
        assert_eq!(reviewer.branches, ["feat/billing-*"]);
        assert_eq!(
            parse("billing", &render(&reviewer), ReviewerSource::Repository).unwrap(),
            reviewer
        );
    }

    #[test]
    fn a_reviewer_without_conditions_writes_no_such_lines() {
        let reviewer = sample();
        assert!(reviewer.paths.is_empty() && reviewer.branches.is_empty());
        let text = render(&reviewer);
        assert!(
            !text.contains("paths:") && !text.contains("branches:"),
            "{text}"
        );
        assert!(
            presets()
                .iter()
                .all(|p| p.paths.is_empty() && p.branches.is_empty())
        );
    }

    #[test]
    fn a_bad_condition_names_its_line_and_pattern() {
        let head = "---\ndescription: d\nsignal: pass\n";
        let absolute = format!("{head}paths: /etc/*\n---\nx\n");
        assert!(
            error_of("a", &absolute)
                .starts_with("line 4: `paths` pattern `/etc/*`: `/etc/*` is absolute"),
            "{}",
            error_of("a", &absolute)
        );
        let dots = format!("{head}paths: a/../b\n---\nx\n");
        assert!(error_of("a", &dots).starts_with("line 4: `paths` pattern `a/../b`"));
        let empty_item = format!("{head}branches: feat/*,,fix/*\n---\nx\n");
        assert!(error_of("a", &empty_item).starts_with("line 4: `branches` has an empty pattern"));
        let many = format!("{head}branches: {}\n---\nx\n", vec!["b"; 21].join(","));
        assert!(error_of("a", &many).starts_with("line 4: `branches` has 21 patterns"));
        let twenty = format!("{head}branches: {}\n---\nx\n", vec!["b"; 20].join(","));
        assert!(parse("a", &twenty, ReviewerSource::User).is_ok());
    }

    #[test]
    fn the_name_rules_are_enforced() {
        let forty = "a".repeat(40);
        let forty_one = "a".repeat(41);
        for good in ["a", "code-review", "x1", "a-b-c", forty.as_str()] {
            assert!(validate_name(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "Code",
            "a_b",
            "a b",
            "-a",
            "a-",
            "é",
            forty_one.as_str(),
        ] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
        assert!(parse("Bad", SAMPLE, ReviewerSource::User).is_err());
    }

    #[test]
    fn every_front_matter_error_names_its_line() {
        assert!(error_of("a", "x\n---\n").contains("line 1"));
        assert!(error_of("a", "---\ndescription: d\nsignal: score\n").contains("not closed"));
        assert!(
            error_of("a", "---\ndescription d\nsignal: score\n---\nx\n")
                .starts_with("line 2: expected `key: value`")
        );
        assert!(
            error_of(
                "a",
                "---\ndescription: d\nsignal: score\ncolour: red\n---\nx\n"
            )
            .starts_with("line 4: unknown key `colour`")
        );
        assert!(
            error_of(
                "a",
                "---\ndescription: d\nsignal: score\nsignal: level\n---\nx\n"
            )
            .starts_with("line 4: duplicate key `signal`")
        );
        assert!(
            error_of("a", "---\ndescription: d\nsignal: spin\n---\nx\n")
                .starts_with("line 3: `signal` must be score, level or pass")
        );
        assert!(
            error_of(
                "a",
                "---\ndescription: d\nsignal: score\ndirection: up\n---\nx\n"
            )
            .starts_with("line 4: `direction` must be")
        );
        assert!(
            error_of(
                "a",
                "---\ndescription: d\nsignal: score\nsubject: pr\n---\nx\n"
            )
            .starts_with("line 4: `subject` must be")
        );
        assert!(
            error_of("a", "---\nsignal: score\n---\nx\n").contains("`description` is required")
        );
        assert!(error_of("a", "---\ndescription: d\n---\nx\n").contains("`signal` is required"));
        assert!(
            error_of("a", "---\ndescription:\nsignal: score\n---\nx\n")
                .contains("`description` is required")
        );
        assert!(
            error_of("a", "---\ndescription: d\nsignal: score\n---\n \n")
                .starts_with("line 5: the instructions are empty")
        );
    }

    #[test]
    fn the_length_limits_are_enforced() {
        let long_description = format!(
            "---\ndescription: {}\nsignal: pass\n---\nx\n",
            "d".repeat(201)
        );
        assert!(error_of("a", &long_description).starts_with("line 2: `description` is over 200"));
        let exact = format!(
            "---\ndescription: {}\nsignal: pass\n---\nx\n",
            "d".repeat(200)
        );
        assert!(parse("a", &exact, ReviewerSource::User).is_ok());
        let long_body = format!(
            "---\ndescription: d\nsignal: pass\n---\n{}\n",
            "x".repeat(8001)
        );
        assert!(error_of("a", &long_body).starts_with("line 5: the instructions are over 8000"));
    }

    #[test]
    fn a_rendered_reviewer_reads_back_as_itself() {
        let reviewer = sample();
        let text = render(&reviewer);
        assert_eq!(
            parse(&reviewer.name, &text, reviewer.source).unwrap(),
            reviewer
        );
        assert!(text.starts_with("---\ndescription: Looks for leaks\n"));
    }

    #[test]
    fn every_preset_is_valid_and_reads_back() {
        let presets = presets();
        let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["code-review", "security", "slop", "intent-drift", "risk"]
        );
        for preset in &presets {
            assert_eq!(preset.source, ReviewerSource::Preset);
            assert_eq!(preset.model.as_deref(), Some("haiku"));
            assert_eq!(preset.subject, ReviewerSubject::Branch);
            assert!(preset.instructions.lines().count() >= 3, "{}", preset.name);
            let back = parse(&preset.name, &render(preset), ReviewerSource::Preset).unwrap();
            assert_eq!(&back, preset);
        }
    }

    #[test]
    fn save_writes_a_file_that_loads_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("reviewers");
        let mut reviewer = sample();
        reviewer.source = ReviewerSource::User;
        save(&target, &reviewer).expect("save");
        let loaded = load(None, Some(&target));
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        assert_eq!(loaded.reviewers, vec![reviewer.clone()]);
        reviewer.description = "Changed".to_owned();
        save(&target, &reviewer).expect("replace");
        assert_eq!(
            load(None, Some(&target)).reviewers[0].description,
            "Changed"
        );
        let leftovers: Vec<_> = std::fs::read_dir(&target)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, ["leaks.md"], "no temporary file is left");
    }

    #[test]
    fn save_refuses_what_would_not_read_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut reviewer = sample();
        reviewer.description = "two\nlines".to_owned();
        assert!(save(dir.path(), &reviewer).is_err());
        assert!(!dir.path().join("leaks.md").exists());
        reviewer.description = "fine".to_owned();
        reviewer.name = "Bad Name".to_owned();
        assert!(save(dir.path(), &reviewer).is_err());
    }

    #[test]
    fn delete_removes_the_file_and_names_a_missing_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        save(dir.path(), &sample()).expect("save");
        delete(dir.path(), "leaks").expect("delete");
        assert_eq!(
            delete(dir.path(), "leaks").expect_err("gone"),
            "there is no reviewer leaks"
        );
    }

    #[test]
    fn the_directories_are_under_the_common_dir_and_the_config_dir() {
        assert_eq!(
            repo_dir(Path::new("/r/.git")),
            Path::new("/r/.git/gitmaster/reviewers")
        );
        assert_eq!(
            user_dir(Path::new("/home/u/.config/gitmaster")),
            Path::new("/home/u/.config/gitmaster/reviewers")
        );
    }

    fn write(dir: &Path, file: &str, text: &str) {
        std::fs::create_dir_all(dir).expect("dir");
        std::fs::write(dir.join(file), text).expect("write");
    }

    fn reviewer_text(description: &str) -> String {
        format!("---\ndescription: {description}\nsignal: pass\n---\nCheck it.\n")
    }

    #[test]
    fn load_prefers_the_repository_and_sorts_by_name() {
        let root = tempfile::tempdir().expect("tempdir");
        let repo = root.path().join("repo");
        let user = root.path().join("user");
        write(&repo, "zeta.md", &reviewer_text("repo zeta"));
        write(&repo, "alpha.md", &reviewer_text("repo alpha"));
        write(&user, "alpha.md", &reviewer_text("user alpha"));
        write(&user, "beta.md", &reviewer_text("user beta"));
        let loaded = load(Some(&repo), Some(&user));
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let names: Vec<(&str, &str, ReviewerSource)> = loaded
            .reviewers
            .iter()
            .map(|r| (r.name.as_str(), r.description.as_str(), r.source))
            .collect();
        assert_eq!(
            names,
            [
                ("alpha", "repo alpha", ReviewerSource::Repository),
                ("beta", "user beta", ReviewerSource::User),
                ("zeta", "repo zeta", ReviewerSource::Repository),
            ]
        );
    }

    #[test]
    fn a_bad_file_is_reported_and_the_others_still_load() {
        let root = tempfile::tempdir().expect("tempdir");
        let repo = root.path().join("repo");
        write(&repo, "good.md", &reviewer_text("fine"));
        write(&repo, "broken.md", "no front matter at all\n");
        write(&repo, "Bad_Name.md", &reviewer_text("bad name"));
        write(&repo, "notes.txt", "not a reviewer, ignored\n");
        write(&repo, "huge.md", &"x".repeat(65 * 1024));
        let loaded = load(Some(&repo), None);
        let names: Vec<&str> = loaded.reviewers.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["good"]);
        let files: Vec<&str> = loaded.errors.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(files, ["Bad_Name.md", "broken.md", "huge.md"]);
        assert!(loaded.errors[1].1.contains("line 1"));
        assert_eq!(loaded.errors[2].1, "the file is over 64 KiB");
    }

    #[test]
    fn a_missing_directory_is_no_reviewers_and_no_error() {
        let root = tempfile::tempdir().expect("tempdir");
        let loaded = load(Some(&root.path().join("nope")), None);
        assert_eq!(loaded, LoadedReviewers::default());
    }

    #[test]
    fn a_shadowed_bad_user_file_is_still_reported() {
        let root = tempfile::tempdir().expect("tempdir");
        let repo = root.path().join("repo");
        let user = root.path().join("user");
        write(&repo, "same.md", &reviewer_text("repo"));
        write(&user, "same.md", "broken\n");
        let loaded = load(Some(&repo), Some(&user));
        assert_eq!(loaded.reviewers.len(), 1);
        assert_eq!(loaded.reviewers[0].source, ReviewerSource::Repository);
        assert_eq!(loaded.errors.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_skipped() {
        let root = tempfile::tempdir().expect("tempdir");
        let repo = root.path().join("repo");
        write(
            &root.path().join("elsewhere"),
            "linked.md",
            &reviewer_text("linked"),
        );
        std::fs::create_dir_all(&repo).expect("dir");
        std::os::unix::fs::symlink(
            root.path().join("elsewhere").join("linked.md"),
            repo.join("linked.md"),
        )
        .expect("symlink");
        write(&repo, "real.md", &reviewer_text("real"));
        let loaded = load(Some(&repo), None);
        let names: Vec<&str> = loaded.reviewers.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["real"]);
        assert!(loaded.errors.is_empty());
    }

    proptest! {
        #[test]
        fn any_valid_reviewer_reads_back_from_its_file(
            name in "[a-z0-9][a-z0-9-]{0,38}[a-z0-9]",
            description in "[a-zA-Z][a-zA-Z0-9 .,:()-]{0,60}[a-zA-Z0-9]",
            instructions in "[a-zA-Z][a-zA-Z0-9 .,:\n-]{0,300}[a-zA-Z.]",
            signal in prop_oneof![
                Just(ReviewerSignal::Score),
                Just(ReviewerSignal::Level),
                Just(ReviewerSignal::Pass),
            ],
            direction in prop_oneof![
                Just(ReviewerDirection::HigherIsBetter),
                Just(ReviewerDirection::LowerIsBetter),
            ],
            subject in prop_oneof![
                Just(ReviewerSubject::Branch),
                Just(ReviewerSubject::Staged),
                Just(ReviewerSubject::Commit),
            ],
            model in proptest::option::of("[a-z0-9.-]{1,20}"),
            paths in proptest::collection::vec("[a-z*?]{1,8}(/[a-z*?]{1,8}){0,2}", 0..4),
            branches in proptest::collection::vec("[a-z*?/-]{1,12}", 0..4),
        ) {
            let reviewer = Reviewer {
                name,
                description,
                instructions,
                signal,
                direction,
                subject,
                model,
                source: ReviewerSource::Repository,
                paths,
                branches,
            };
            let back = parse(&reviewer.name, &render(&reviewer), reviewer.source);
            prop_assert_eq!(back, Ok(reviewer));
        }
    }
}

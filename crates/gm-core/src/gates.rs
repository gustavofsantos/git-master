//! Gates (D189 (5), D212 (2), D214, D215 (2)): the file they are read from, the layers they come
//! from, and the pure check that says whether an operation may go on. The engine reads the
//! branch, its commits and its verdicts, and calls [`evaluate`]; nothing here runs a model or
//! asks Git anything.
//!
//! The file is `gates.json` directly inside a directory: `.git/gitmaster/` for the repository
//! and the configuration directory for the user (D214 (2)). Nothing is written into a working
//! tree.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::Path;

use gm_schema::{
    GateDef, GateKind, GateLayer, GateOp, GateOutcome, GatesStatus, LayeredGate, Level,
    MissingReason, MissingReviewer, ReviewSeverity, ReviewStatus, Reviewer, ReviewerSignal,
    Verdict, When,
};
use serde_json::{Map, Value};

use crate::{glob, reviewers};

/// The name of the gate file inside a layer's directory.
pub const FILE_NAME: &str = "gates.json";
/// The largest gate file `load` reads; a bigger one is reported and skipped.
const FILE_LIMIT: u64 = 256 * 1024;
/// The most gates one file holds.
const GATE_LIMIT: usize = 50;
/// The most patterns one `when` field holds (D214 (1)).
const PATTERN_LIMIT: usize = 20;
/// The most reviewers one `fresh_verdict` gate names.
const REVIEWER_LIMIT: usize = 10;
/// The kinds a gate can have, as the file writes them.
const KINDS: [&str; 5] = [
    "fresh_verdict",
    "findings",
    "signal",
    "up_to_date",
    "human_approval",
];

/// The gates that were read, merged by id, and the problems found in the files.
///
/// `errors` is never empty for a file that could not be read: that file contributed no gates at
/// all. A caller that checks gates must fail closed on any error (see [`load`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedGates {
    pub gates: Vec<LayeredGate>,
    pub errors: Vec<String>,
}

/// A finding of a verdict, as a gate sees it: its severity and the status a person set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindingFact {
    pub severity: ReviewSeverity,
    pub status: ReviewStatus,
}

/// What a gate check knows about the branch: the operation's head, the change, the reviewers
/// and verdicts recorded for it, and whether a person approved it.
#[derive(Clone, Debug)]
pub struct GateFacts<'a> {
    pub branch: &'a str,
    /// The tip the operation is about. A verdict counts only when its `head` is this commit.
    pub head: &'a str,
    /// The paths the change touches, from the merge base with its base to the tip. `None` when
    /// they are unknown: a gate or reviewer with `paths` then applies (D214 (1), fail closed).
    pub changed_paths: Option<&'a [String]>,
    /// Every defined reviewer, after the layers are merged.
    pub reviewers: &'a [Reviewer],
    /// The recorded verdicts for this branch. The newest one of a reviewer that matches `head`
    /// counts; a verdict for another head is stale.
    pub verdicts: &'a [Verdict],
    /// The findings of the branch's verdicts.
    pub findings: &'a [FindingFact],
    /// How many commits the branch is behind its parent in the stack model, when it has one.
    pub behind_parent: Option<u32>,
    /// True when a person approved `head` (D215 (2)).
    pub approved: bool,
}

/// Reads a gate file: `{"gates": [ ... ]}`. Every problem is reported, not only the first, and
/// a file with any problem gives no gates.
///
/// Unknown fields are refused by hand, because a flattened enum cannot deny them. A gate's
/// `signal` bounds are checked for their JSON type only: whether a bound fits its reviewer's
/// signal is decided when the gate is evaluated (a score bound on a level reviewer is
/// `gate misconfigured`).
pub fn parse(text: &str, layer: GateLayer) -> Result<Vec<LayeredGate>, Vec<String>> {
    let value: Value = serde_json::from_str(text).map_err(|e| vec![format!("not JSON: {e}")])?;
    let Some(object) = value.as_object() else {
        return Err(vec![
            "the file must be a JSON object: {\"gates\": [ ... ]}".to_owned(),
        ]);
    };
    let mut errors: Vec<String> = object
        .keys()
        .filter(|key| key.as_str() != "gates")
        .map(|key| format!("unknown key `{key}` at the top level"))
        .collect();
    let Some(list) = object.get("gates") else {
        errors.push("missing `gates`: the file is {\"gates\": [ ... ]}".to_owned());
        return Err(errors);
    };
    let Some(list) = list.as_array() else {
        errors.push("`gates` must be a list".to_owned());
        return Err(errors);
    };
    if list.len() > GATE_LIMIT {
        errors.push(format!(
            "the file has {} gates, the most is {GATE_LIMIT}",
            list.len()
        ));
    }
    let mut gates = Vec::with_capacity(list.len());
    let mut ids = BTreeSet::new();
    for (index, item) in list.iter().enumerate() {
        match parse_gate(index, item) {
            Ok(gate) => {
                if !ids.insert(gate.id.clone()) {
                    errors.push(format!("gate `{}`: duplicate id", gate.id));
                }
                gates.push(LayeredGate { gate, layer });
            }
            Err(found) => errors.extend(found),
        }
    }
    if errors.is_empty() {
        Ok(gates)
    } else {
        Err(errors)
    }
}

/// Reads the repository's gates from `repo_dir` and the user's from `user_dir`; either may be
/// absent, and a missing file is no gates. A repository gate wins over a user gate with the
/// same id, as a whole (D214 (2)). The result is sorted by id.
///
/// A file that cannot be read, is a symlink, is over 256 KiB or does not parse contributes no
/// gates, and its errors are prefixed with its path. **The caller must fail closed on any
/// error**: a gate check that cannot see every gate it should cannot say the operation is ready.
#[must_use]
pub fn load(repo_dir: Option<&Path>, user_dir: Option<&Path>) -> LoadedGates {
    let mut by_id: BTreeMap<String, LayeredGate> = BTreeMap::new();
    let mut errors = Vec::new();
    for (dir, layer) in [
        (repo_dir, GateLayer::Repository),
        (user_dir, GateLayer::User),
    ] {
        let Some(dir) = dir else {
            continue;
        };
        match read_file(&dir.join(FILE_NAME), layer) {
            Ok(gates) => {
                for gate in gates {
                    by_id.entry(gate.gate.id.clone()).or_insert(gate);
                }
            }
            Err(found) => errors.extend(found),
        }
    }
    LoadedGates {
        gates: by_id.into_values().collect(),
        errors,
    }
}

/// Writes `gates` as `dir/gates.json`, pretty and with a trailing newline. The text is parsed
/// back first and must read as the same gates, so a file the engine cannot read is never
/// written. The file is replaced in one rename, so a reader sees the old one or the new one.
pub fn save(dir: &Path, gates: &[GateDef]) -> Result<(), String> {
    let document = serde_json::json!({ "gates": gates });
    let mut text =
        serde_json::to_string_pretty(&document).map_err(|e| format!("cannot save gates: {e}"))?;
    text.push('\n');
    let back = parse(&text, GateLayer::Repository)
        .map_err(|errors| format!("cannot save gates: {}", errors.join("; ")))?;
    if back.iter().map(|layered| &layered.gate).ne(gates.iter()) {
        return Err(
            "cannot save gates: a field would not be read back as it was written".to_owned(),
        );
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let mut file = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| format!("cannot write in {}: {e}", dir.display()))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|e| format!("cannot write in {}: {e}", dir.display()))?;
    let target = dir.join(FILE_NAME);
    file.persist(&target)
        .map_err(|e| format!("cannot save {}: {}", target.display(), e.error))?;
    Ok(())
}

/// Checks the gates that apply to `op` (every gate when `op` is `None`) against the facts.
///
/// A gate that blocks names its reason; a gate whose conditions do not match the change is not
/// applicable and passes. `ready` is true when every considered gate passes. Gates are listed
/// sorted by id, and `missing` names each reviewer a gate needs and that has no verdict for the
/// head, sorted by name. The result does not depend on the order of any input (gates ids are
/// unique in a loaded set; the check sorts by them). `errors` is left empty for the caller.
#[must_use]
pub fn evaluate(gates: &[LayeredGate], op: Option<GateOp>, facts: &GateFacts<'_>) -> GatesStatus {
    let mut considered: Vec<&LayeredGate> = gates
        .iter()
        .filter(|layered| op.is_none_or(|op| layered.gate.on.contains(&op)))
        .collect();
    considered.sort_by(|a, b| a.gate.id.cmp(&b.gate.id));
    let mut missing: BTreeMap<String, MissingReviewer> = BTreeMap::new();
    let gates: Vec<GateOutcome> = considered
        .into_iter()
        .map(|layered| evaluate_gate(layered, facts, &mut missing))
        .collect();
    GatesStatus {
        branch: facts.branch.to_owned(),
        head: facts.head.to_owned(),
        ready: gates.iter().all(|outcome| outcome.passed),
        gates,
        missing: missing.into_values().collect(),
        errors: Vec::new(),
    }
}

/// The outcome of one gate. A reviewer it needs and that has no verdict for the head is added to
/// `missing`.
fn evaluate_gate(
    layered: &LayeredGate,
    facts: &GateFacts<'_>,
    missing: &mut BTreeMap<String, MissingReviewer>,
) -> GateOutcome {
    let gate = &layered.gate;
    let base = GateOutcome {
        id: gate.id.clone(),
        kind: kind_name(&gate.kind).to_owned(),
        layer: layered.layer,
        applicable: true,
        not_applicable: None,
        passed: true,
        reason: None,
    };
    if let Err(why) = applies(&gate.when.branches, &gate.when.paths, facts) {
        return GateOutcome {
            applicable: false,
            not_applicable: Some(why),
            ..base
        };
    }
    let blocked = match &gate.kind {
        GateKind::FreshVerdict { reviewers } => fresh_verdict(reviewers, facts, missing),
        GateKind::Findings { severity } => findings(*severity, facts),
        GateKind::Signal { reviewer, max, min } => {
            signal(reviewer, max.as_ref(), min.as_ref(), facts, missing)
        }
        GateKind::UpToDate => up_to_date(facts),
        GateKind::HumanApproval => human_approval(facts),
    };
    GateOutcome {
        passed: blocked.is_none(),
        reason: blocked,
        ..base
    }
}

/// `fresh_verdict`: every named reviewer that is defined and applies has a verdict for the head.
fn fresh_verdict(
    names: &[String],
    facts: &GateFacts<'_>,
    missing: &mut BTreeMap<String, MissingReviewer>,
) -> Option<String> {
    let mut reasons = Vec::new();
    for name in names {
        let Some(reviewer) = find_reviewer(facts, name) else {
            reasons.push(format!("reviewer {name} is not defined"));
            continue;
        };
        if applies(&reviewer.branches, &reviewer.paths, facts).is_err()
            || newest(facts.verdicts, name, Some(facts.head)).is_some()
        {
            continue;
        }
        reasons.push(no_verdict(name, facts));
        add_missing(missing, reviewer, facts);
    }
    join(&reasons)
}

/// `findings`: no unresolved finding (open or handled, not resolved) at or above `severity`.
fn findings(severity: ReviewSeverity, facts: &GateFacts<'_>) -> Option<String> {
    let count = facts
        .findings
        .iter()
        .filter(|finding| {
            finding.status != ReviewStatus::Resolved
                && severity_rank(finding.severity) >= severity_rank(severity)
        })
        .count();
    if count == 0 {
        return None;
    }
    let noun = if count == 1 { "finding" } else { "findings" };
    Some(format!(
        "{count} unresolved {noun} at or above {}",
        severity_text(severity)
    ))
}

/// `signal`: the reviewer's current verdict is within the bound, read by the reviewer's own
/// signal.
fn signal(
    name: &str,
    max: Option<&Value>,
    min: Option<&Value>,
    facts: &GateFacts<'_>,
    missing: &mut BTreeMap<String, MissingReviewer>,
) -> Option<String> {
    let Some(reviewer) = find_reviewer(facts, name) else {
        return Some(format!("reviewer {name} is not defined"));
    };
    if applies(&reviewer.branches, &reviewer.paths, facts).is_err() {
        return None;
    }
    let Some(verdict) = newest(facts.verdicts, name, Some(facts.head)) else {
        add_missing(missing, reviewer, facts);
        return Some(format!("no current verdict from {name}"));
    };
    match reviewer.signal {
        ReviewerSignal::Pass => pass_signal(name, &verdict.value, max, min),
        ReviewerSignal::Score => {
            if max.is_none() && min.is_none() {
                return Some(needs_bound(name));
            }
            score_bounds(name, &verdict.value, max, min)
        }
        ReviewerSignal::Level => {
            if max.is_none() && min.is_none() {
                return Some(needs_bound(name));
            }
            level_bounds(name, &verdict.value, max, min)
        }
    }
}

/// A pass reviewer takes no bound: its value must be `pass`.
fn pass_signal(
    name: &str,
    value: &str,
    max: Option<&Value>,
    min: Option<&Value>,
) -> Option<String> {
    if max.is_some() || min.is_some() {
        return Some("gate misconfigured: pass reviewers take no bound".to_owned());
    }
    (value != "pass").then(|| format!("{name} recorded {value}, not pass"))
}

/// A score is compared with its bounds as integers.
fn score_bounds(
    name: &str,
    value: &str,
    max: Option<&Value>,
    min: Option<&Value>,
) -> Option<String> {
    if [max, min]
        .into_iter()
        .flatten()
        .any(|bound| bound.as_i64().is_none())
    {
        return Some(misconfigured(
            name,
            "a score",
            "a whole number from 0 to 100",
        ));
    }
    let Ok(score) = value.parse::<i64>() else {
        return Some(format!("{name} recorded {value}, which is not a score"));
    };
    let mut reasons = Vec::new();
    if let Some(limit) = max.and_then(Value::as_i64).filter(|limit| score > *limit) {
        reasons.push(format!("{name} is {score}, above the maximum {limit}"));
    }
    if let Some(limit) = min.and_then(Value::as_i64).filter(|limit| score < *limit) {
        reasons.push(format!("{name} is {score}, below the minimum {limit}"));
    }
    join(&reasons)
}

/// A level is compared with its bounds in the order low, medium, high.
fn level_bounds(
    name: &str,
    value: &str,
    max: Option<&Value>,
    min: Option<&Value>,
) -> Option<String> {
    if [max, min]
        .into_iter()
        .flatten()
        .any(|bound| bound.as_str().and_then(level_of).is_none())
    {
        return Some(misconfigured(name, "a level", "low, medium or high"));
    }
    let Some(level) = level_of(value) else {
        return Some(format!("{name} recorded {value}, which is not a level"));
    };
    let mut reasons = Vec::new();
    if let Some(limit) = max
        .and_then(Value::as_str)
        .and_then(level_of)
        .filter(|limit| level > *limit)
    {
        reasons.push(format!(
            "{name} is {}, above the maximum {}",
            level_text(level),
            level_text(limit)
        ));
    }
    if let Some(limit) = min
        .and_then(Value::as_str)
        .and_then(level_of)
        .filter(|limit| level < *limit)
    {
        reasons.push(format!(
            "{name} is {}, below the minimum {}",
            level_text(level),
            level_text(limit)
        ));
    }
    join(&reasons)
}

/// `up_to_date`: the branch is not behind its parent in the stack model.
fn up_to_date(facts: &GateFacts<'_>) -> Option<String> {
    facts
        .behind_parent
        .filter(|behind| *behind > 0)
        .map(|behind| format!("{behind} commit(s) behind its parent"))
}

/// `human_approval`: a person approved the head (D215 (2)).
fn human_approval(facts: &GateFacts<'_>) -> Option<String> {
    (!facts.approved).then(|| format!("you have not approved {}", short(facts.head)))
}

/// Whether a gate or reviewer with these conditions applies to the change (D214 (1)). The
/// branch condition is checked first; an unknown change counts as touching every path.
fn applies(branches: &[String], paths: &[String], facts: &GateFacts<'_>) -> Result<(), String> {
    if !branches.is_empty()
        && !branches
            .iter()
            .any(|pattern| glob::matches(pattern, facts.branch))
    {
        return Err(format!("the branch does not match {}", branches.join(", ")));
    }
    match facts.changed_paths {
        Some(changed)
            if !paths.is_empty()
                && !changed
                    .iter()
                    .any(|path| paths.iter().any(|pattern| glob::matches(pattern, path))) =>
        {
            Err(format!("no changed file matches {}", paths.join(", ")))
        }
        _ => Ok(()),
    }
}

/// The newest verdict of a reviewer, for `head` when one is given. Newest is by time, then id,
/// so the choice does not depend on the order of the list.
fn newest<'a>(verdicts: &'a [Verdict], reviewer: &str, head: Option<&str>) -> Option<&'a Verdict> {
    verdicts
        .iter()
        .filter(|verdict| verdict.reviewer == reviewer && head.is_none_or(|h| verdict.head == h))
        .max_by_key(|verdict| (verdict.at, verdict.id))
}

fn find_reviewer<'a>(facts: &GateFacts<'a>, name: &str) -> Option<&'a Reviewer> {
    facts
        .reviewers
        .iter()
        .find(|reviewer| reviewer.name == name)
}

/// The reason a reviewer has no verdict for the head, naming the last one it has.
fn no_verdict(name: &str, facts: &GateFacts<'_>) -> String {
    let head = short(facts.head);
    match newest(facts.verdicts, name, None) {
        Some(older) => format!(
            "{name} has no verdict for {head} (its last verdict is for {})",
            short(&older.head)
        ),
        None => format!("{name} has no verdict for {head}"),
    }
}

/// Records a reviewer that a gate needs and that has no verdict for the head, once per name.
fn add_missing(
    missing: &mut BTreeMap<String, MissingReviewer>,
    reviewer: &Reviewer,
    facts: &GateFacts<'_>,
) {
    let reason = if newest(facts.verdicts, &reviewer.name, None).is_some() {
        MissingReason::Stale
    } else {
        MissingReason::NoVerdict
    };
    missing
        .entry(reviewer.name.clone())
        .or_insert_with(|| MissingReviewer {
            reviewer: reviewer.name.clone(),
            subject: reviewer.subject,
            model: reviewer.model.clone(),
            reason,
        });
}

/// The reasons of one gate, joined; none is no block.
fn join(reasons: &[String]) -> Option<String> {
    if reasons.is_empty() {
        None
    } else {
        Some(reasons.join("; "))
    }
}

fn needs_bound(name: &str) -> String {
    format!("gate misconfigured: a signal gate on {name} needs max or min")
}

fn misconfigured(name: &str, what: &str, expected: &str) -> String {
    format!("gate misconfigured: {name} is {what} reviewer, so its bound is {expected}")
}

/// The first seven characters of a commit, as a reason shows it.
fn short(head: &str) -> String {
    head.chars().take(7).collect()
}

fn severity_rank(severity: ReviewSeverity) -> u8 {
    match severity {
        ReviewSeverity::Low => 0,
        ReviewSeverity::Medium => 1,
        ReviewSeverity::High => 2,
    }
}

fn severity_text(severity: ReviewSeverity) -> &'static str {
    match severity {
        ReviewSeverity::Low => "low",
        ReviewSeverity::Medium => "medium",
        ReviewSeverity::High => "high",
    }
}

fn level_of(text: &str) -> Option<Level> {
    match text {
        "low" => Some(Level::Low),
        "medium" => Some(Level::Medium),
        "high" => Some(Level::High),
        _ => None,
    }
}

fn level_text(level: Level) -> &'static str {
    match level {
        Level::Low => "low",
        Level::Medium => "medium",
        Level::High => "high",
    }
}

fn kind_name(kind: &GateKind) -> &'static str {
    match kind {
        GateKind::FreshVerdict { .. } => "fresh_verdict",
        GateKind::Findings { .. } => "findings",
        GateKind::Signal { .. } => "signal",
        GateKind::UpToDate => "up_to_date",
        GateKind::HumanApproval => "human_approval",
    }
}

/// The keys a gate object may have, for its kind.
fn kind_keys(kind: &str) -> &'static [&'static str] {
    match kind {
        "fresh_verdict" => &["id", "kind", "on", "when", "reviewers"],
        "findings" => &["id", "kind", "on", "when", "severity"],
        "signal" => &["id", "kind", "on", "when", "reviewer", "max", "min"],
        _ => &["id", "kind", "on", "when"],
    }
}

fn read_file(path: &Path, layer: GateLayer) -> Result<Vec<LayeredGate>, Vec<String>> {
    let shown = path.display();
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(vec![format!("{shown}: cannot read the file: {e}")]),
    };
    if meta.file_type().is_symlink() {
        return Err(vec![format!("{shown}: a symbolic link is not read")]);
    }
    if !meta.is_file() {
        return Err(vec![format!("{shown}: not a file")]);
    }
    if meta.len() > FILE_LIMIT {
        return Err(vec![format!("{shown}: the file is over 256 KiB")]);
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| vec![format!("{shown}: cannot read the file: {e}")])?;
    parse(&text, layer).map_err(|errors| {
        errors
            .into_iter()
            .map(|e| format!("{shown}: {e}"))
            .collect()
    })
}

/// Parses one gate object. Every problem in it is returned, so a file reports them all.
fn parse_gate(index: usize, item: &Value) -> Result<GateDef, Vec<String>> {
    let Some(object) = item.as_object() else {
        return Err(vec![format!("gate {}: must be a JSON object", index + 1)]);
    };
    let label = match object.get("id").and_then(Value::as_str) {
        Some(id) => format!("gate `{id}`"),
        None => format!("gate {}", index + 1),
    };
    let mut errors = Vec::new();
    let id = match object.get("id") {
        None => {
            errors.push(format!("{label}: `id` is required"));
            String::new()
        }
        Some(Value::String(id)) => {
            if let Err(reason) = reviewers::validate_name(id) {
                errors.push(format!("{label}: {reason}"));
            }
            id.clone()
        }
        Some(_) => {
            errors.push(format!("{label}: `id` must be a string"));
            String::new()
        }
    };
    let kind_name = match object.get("kind") {
        None => {
            errors.push(format!("{label}: `kind` is required"));
            None
        }
        Some(Value::String(kind)) if KINDS.contains(&kind.as_str()) => Some(kind.as_str()),
        Some(Value::String(other)) => {
            errors.push(format!(
                "{label}: unknown kind `{other}`: use {}",
                KINDS.join(", ")
            ));
            None
        }
        Some(_) => {
            errors.push(format!("{label}: `kind` must be a word"));
            None
        }
    };
    if let Some(kind_name) = kind_name {
        for key in object.keys() {
            if !kind_keys(kind_name).contains(&key.as_str()) {
                errors.push(format!("{label}: unknown field `{key}`"));
            }
        }
    }
    let on = parse_on(object.get("on"), &label, &mut errors);
    let when = match object.get("when") {
        None => When::default(),
        Some(value) => parse_when(value, &label, &mut errors),
    };
    let kind = kind_name.and_then(|kind_name| parse_kind(kind_name, object, &label, &mut errors));
    match kind {
        Some(kind) if errors.is_empty() => Ok(GateDef { id, kind, on, when }),
        _ => Err(errors),
    }
}

/// The settings of a gate kind, read from its object.
fn parse_kind(
    kind_name: &str,
    object: &Map<String, Value>,
    label: &str,
    errors: &mut Vec<String>,
) -> Option<GateKind> {
    match kind_name {
        "fresh_verdict" => {
            let names = name_list(object.get("reviewers"), label, errors)?;
            if names.is_empty() {
                errors.push(format!("{label}: `reviewers` names at least one reviewer"));
                return None;
            }
            if names.len() > REVIEWER_LIMIT {
                errors.push(format!(
                    "{label}: `reviewers` names {} reviewers, the most is {REVIEWER_LIMIT}",
                    names.len()
                ));
            }
            Some(GateKind::FreshVerdict { reviewers: names })
        }
        "findings" => {
            let severity = match object.get("severity").and_then(Value::as_str) {
                Some("low") => ReviewSeverity::Low,
                Some("medium") => ReviewSeverity::Medium,
                Some("high") => ReviewSeverity::High,
                _ => {
                    errors.push(format!("{label}: `severity` must be low, medium or high"));
                    return None;
                }
            };
            Some(GateKind::Findings { severity })
        }
        "signal" => {
            let reviewer = if let Some(value) = object.get("reviewer") {
                name_of(value, label, errors)
            } else {
                errors.push(format!("{label}: `reviewer` is required"));
                None
            };
            let max = bound(object, "max", label, errors);
            let min = bound(object, "min", label, errors);
            Some(GateKind::Signal {
                reviewer: reviewer?,
                max,
                min,
            })
        }
        "up_to_date" => Some(GateKind::UpToDate),
        _ => Some(GateKind::HumanApproval),
    }
}

fn parse_on(value: Option<&Value>, label: &str, errors: &mut Vec<String>) -> Vec<GateOp> {
    match value {
        None => {
            errors.push(format!(
                "{label}: `on` is required: a list of push and restack"
            ));
            Vec::new()
        }
        Some(Value::Array(items)) => {
            if items.is_empty() {
                errors.push(format!(
                    "{label}: `on` is empty: name push, restack or both"
                ));
            }
            items
                .iter()
                .filter_map(|item| match item.as_str() {
                    Some("push") => Some(GateOp::Push),
                    Some("restack") => Some(GateOp::Restack),
                    _ => {
                        errors.push(format!("{label}: `on` takes push or restack, not {item}"));
                        None
                    }
                })
                .collect()
        }
        Some(_) => {
            errors.push(format!("{label}: `on` must be a list"));
            Vec::new()
        }
    }
}

fn parse_when(value: &Value, label: &str, errors: &mut Vec<String>) -> When {
    let Some(object) = value.as_object() else {
        errors.push(format!("{label}: `when` must be an object"));
        return When::default();
    };
    for key in object.keys() {
        if key != "branches" && key != "paths" {
            errors.push(format!("{label}: unknown field `when.{key}`"));
        }
    }
    When {
        branches: patterns(
            object.get("branches"),
            "when.branches",
            glob::check_pattern,
            label,
            errors,
        ),
        paths: patterns(
            object.get("paths"),
            "when.paths",
            glob::check_path_pattern,
            label,
            errors,
        ),
    }
}

/// The patterns of one `when` field, each checked by `check`.
fn patterns(
    value: Option<&Value>,
    field: &str,
    check: fn(&str) -> Result<(), String>,
    label: &str,
    errors: &mut Vec<String>,
) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        errors.push(format!("{label}: `{field}` must be a list of patterns"));
        return Vec::new();
    };
    if items.len() > PATTERN_LIMIT {
        errors.push(format!(
            "{label}: `{field}` has {} patterns, the most is {PATTERN_LIMIT}",
            items.len()
        ));
    }
    let mut kept = Vec::with_capacity(items.len());
    for item in items {
        match item.as_str() {
            Some(pattern) => match check(pattern) {
                Ok(()) => kept.push(pattern.to_owned()),
                Err(reason) => {
                    errors.push(format!("{label}: `{field}` pattern `{pattern}`: {reason}"));
                }
            },
            None => errors.push(format!(
                "{label}: `{field}` takes text patterns, not {item}"
            )),
        }
    }
    kept
}

/// A list of reviewer names. Absent is an error, since the kind needs it.
fn name_list(value: Option<&Value>, label: &str, errors: &mut Vec<String>) -> Option<Vec<String>> {
    let Some(value) = value else {
        errors.push(format!("{label}: `reviewers` is required"));
        return None;
    };
    let Some(items) = value.as_array() else {
        errors.push(format!(
            "{label}: `reviewers` must be a list of reviewer names"
        ));
        return None;
    };
    Some(
        items
            .iter()
            .filter_map(|item| name_of(item, label, errors))
            .collect(),
    )
}

/// A reviewer name, checked with the reviewer name rule.
fn name_of(value: &Value, label: &str, errors: &mut Vec<String>) -> Option<String> {
    let Some(name) = value.as_str() else {
        errors.push(format!("{label}: a reviewer name is text, not {value}"));
        return None;
    };
    match reviewers::validate_name(name) {
        Ok(()) => Some(name.to_owned()),
        Err(reason) => {
            errors.push(format!("{label}: {reason}"));
            None
        }
    }
}

/// A `max` or `min` bound: a whole number from 0 to 100 for a score, or a level word. Absent
/// and `null` are no bound. The reviewer's signal decides which one fits, at evaluation.
fn bound(
    object: &Map<String, Value>,
    field: &str,
    label: &str,
    errors: &mut Vec<String>,
) -> Option<Value> {
    match object.get(field) {
        None | Some(Value::Null) => None,
        Some(value) => {
            let whole = value.as_u64().is_some_and(|number| number <= 100);
            let level = value.as_str().is_some_and(|word| level_of(word).is_some());
            if whole || level {
                Some(value.clone())
            } else {
                errors.push(format!(
                    "{label}: `{field}` must be a whole number from 0 to 100 for a score, or low, medium or high for a level"
                ));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use gm_schema::{ReviewerDirection, ReviewerSource, ReviewerSubject};
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    const HEAD: &str = "1234567890abcdef";
    const OLD: &str = "abcdef0123456789";

    fn reviewer(name: &str, signal: ReviewerSignal) -> Reviewer {
        Reviewer {
            name: name.to_owned(),
            description: "d".to_owned(),
            instructions: "Look.".to_owned(),
            signal,
            direction: ReviewerDirection::HigherIsBetter,
            subject: ReviewerSubject::Branch,
            model: Some("haiku".to_owned()),
            source: ReviewerSource::Repository,
            paths: Vec::new(),
            branches: Vec::new(),
        }
    }

    fn verdict(reviewer: &str, head: &str, value: &str, at: i64) -> Verdict {
        Verdict {
            id: at,
            reviewer: reviewer.to_owned(),
            branch: "feat/x".to_owned(),
            head: head.to_owned(),
            signal: ReviewerSignal::Score,
            value: value.to_owned(),
            rationale: String::new(),
            origin: "agent:test".to_owned(),
            model: None,
            run_thread: None,
            at,
            stale: false,
            findings: Vec::new(),
        }
    }

    fn gate(id: &str, kind: GateKind) -> LayeredGate {
        LayeredGate {
            gate: GateDef {
                id: id.to_owned(),
                kind,
                on: vec![GateOp::Push, GateOp::Restack],
                when: When::default(),
            },
            layer: GateLayer::Repository,
        }
    }

    fn signal_gate(
        id: &str,
        reviewer: &str,
        max: Option<Value>,
        min: Option<Value>,
    ) -> LayeredGate {
        gate(
            id,
            GateKind::Signal {
                reviewer: reviewer.to_owned(),
                max,
                min,
            },
        )
    }

    fn fresh(id: &str, names: &[&str]) -> LayeredGate {
        gate(
            id,
            GateKind::FreshVerdict {
                reviewers: names.iter().map(|n| (*n).to_owned()).collect(),
            },
        )
    }

    /// A branch `feat/x` at `HEAD`, with the facts the tests change.
    struct Setup {
        reviewers: Vec<Reviewer>,
        verdicts: Vec<Verdict>,
        findings: Vec<FindingFact>,
        changed: Option<Vec<String>>,
        behind: Option<u32>,
        approved: bool,
    }

    impl Setup {
        fn new(reviewers: Vec<Reviewer>, verdicts: Vec<Verdict>) -> Self {
            Self {
                reviewers,
                verdicts,
                findings: Vec::new(),
                changed: Some(vec!["src/lib.rs".to_owned()]),
                behind: Some(0),
                approved: true,
            }
        }

        fn facts(&self) -> GateFacts<'_> {
            GateFacts {
                branch: "feat/x",
                head: HEAD,
                changed_paths: self.changed.as_deref(),
                reviewers: &self.reviewers,
                verdicts: &self.verdicts,
                findings: &self.findings,
                behind_parent: self.behind,
                approved: self.approved,
            }
        }
    }

    fn check(gates: &[LayeredGate], setup: &Setup) -> GatesStatus {
        evaluate(gates, None, &setup.facts())
    }

    fn outcome<'a>(status: &'a GatesStatus, id: &str) -> &'a GateOutcome {
        status
            .gates
            .iter()
            .find(|g| g.id == id)
            .expect("gate listed")
    }

    fn reason(status: &GatesStatus, id: &str) -> String {
        outcome(status, id).reason.clone().unwrap_or_default()
    }

    #[test]
    fn a_fresh_verdict_passes_when_each_reviewer_has_one_for_the_head() {
        let setup = Setup::new(
            vec![reviewer("code-review", ReviewerSignal::Score)],
            vec![verdict("code-review", HEAD, "80", 1)],
        );
        let status = check(&[fresh("review", &["code-review"])], &setup);
        assert!(status.ready);
        assert!(outcome(&status, "review").passed);
        assert!(status.missing.is_empty());
    }

    #[test]
    fn a_fresh_verdict_blocks_without_a_verdict_and_names_the_head() {
        let setup = Setup::new(vec![reviewer("code-review", ReviewerSignal::Score)], vec![]);
        let status = check(&[fresh("review", &["code-review"])], &setup);
        assert!(!status.ready);
        assert_eq!(
            reason(&status, "review"),
            "code-review has no verdict for 1234567"
        );
        assert_eq!(
            status.missing,
            [MissingReviewer {
                reviewer: "code-review".to_owned(),
                subject: ReviewerSubject::Branch,
                model: Some("haiku".to_owned()),
                reason: MissingReason::NoVerdict,
            }]
        );
    }

    #[test]
    fn a_stale_verdict_blocks_and_says_which_head_it_is_for() {
        let setup = Setup::new(
            vec![reviewer("code-review", ReviewerSignal::Score)],
            vec![verdict("code-review", OLD, "80", 1)],
        );
        let status = check(&[fresh("review", &["code-review"])], &setup);
        assert_eq!(
            reason(&status, "review"),
            "code-review has no verdict for 1234567 (its last verdict is for abcdef0)"
        );
        assert_eq!(status.missing[0].reason, MissingReason::Stale);
    }

    #[test]
    fn an_undefined_reviewer_blocks_the_gate_it_is_named_in() {
        let setup = Setup::new(vec![], vec![]);
        let status = check(&[fresh("review", &["ghost"])], &setup);
        assert_eq!(reason(&status, "review"), "reviewer ghost is not defined");
        assert!(status.missing.is_empty());
        let signal = check(&[signal_gate("bar", "ghost", Some(json!(5)), None)], &setup);
        assert_eq!(reason(&signal, "bar"), "reviewer ghost is not defined");
    }

    #[test]
    fn findings_block_at_or_above_the_severity_until_resolved() {
        let mut setup = Setup::new(vec![], vec![]);
        setup.findings = vec![
            FindingFact {
                severity: ReviewSeverity::High,
                status: ReviewStatus::Open,
            },
            FindingFact {
                severity: ReviewSeverity::Low,
                status: ReviewStatus::Open,
            },
            FindingFact {
                severity: ReviewSeverity::Medium,
                status: ReviewStatus::Resolved,
            },
        ];
        let medium = gate(
            "f",
            GateKind::Findings {
                severity: ReviewSeverity::Medium,
            },
        );
        let status = check(std::slice::from_ref(&medium), &setup);
        assert_eq!(
            reason(&status, "f"),
            "1 unresolved finding at or above medium"
        );
        setup.findings[0].status = ReviewStatus::Handled;
        setup.findings[1].severity = ReviewSeverity::Medium;
        let status = check(std::slice::from_ref(&medium), &setup);
        assert_eq!(
            reason(&status, "f"),
            "2 unresolved findings at or above medium"
        );
        for finding in &mut setup.findings {
            finding.status = ReviewStatus::Resolved;
        }
        assert!(check(&[medium], &setup).ready);
    }

    #[test]
    fn a_score_is_compared_with_its_bounds() {
        let setup = Setup::new(
            vec![reviewer("code-review", ReviewerSignal::Score)],
            vec![verdict("code-review", HEAD, "45", 1)],
        );
        let high = check(
            &[signal_gate("s", "code-review", Some(json!(30)), None)],
            &setup,
        );
        assert_eq!(
            reason(&high, "s"),
            "code-review is 45, above the maximum 30"
        );
        let low = check(
            &[signal_gate("s", "code-review", None, Some(json!(60)))],
            &setup,
        );
        assert_eq!(reason(&low, "s"), "code-review is 45, below the minimum 60");
        let inside = check(
            &[signal_gate(
                "s",
                "code-review",
                Some(json!(50)),
                Some(json!(40)),
            )],
            &setup,
        );
        assert!(inside.ready);
    }

    #[test]
    fn a_level_is_compared_low_medium_high() {
        let setup = Setup::new(
            vec![reviewer("security", ReviewerSignal::Level)],
            vec![verdict("security", HEAD, "high", 1)],
        );
        let max = check(
            &[signal_gate("s", "security", Some(json!("medium")), None)],
            &setup,
        );
        assert_eq!(
            reason(&max, "s"),
            "security is high, above the maximum medium"
        );
        let mut setup = setup;
        setup.verdicts = vec![verdict("security", HEAD, "low", 2)];
        let min = check(
            &[signal_gate("s", "security", None, Some(json!("medium")))],
            &setup,
        );
        assert_eq!(
            reason(&min, "s"),
            "security is low, below the minimum medium"
        );
        setup.verdicts = vec![verdict("security", HEAD, "medium", 3)];
        assert!(
            check(
                &[signal_gate("s", "security", Some(json!("medium")), None)],
                &setup
            )
            .ready
        );
    }

    #[test]
    fn a_pass_reviewer_needs_a_pass_and_takes_no_bound() {
        let mut setup = Setup::new(
            vec![reviewer("builds", ReviewerSignal::Pass)],
            vec![verdict("builds", HEAD, "pass", 1)],
        );
        let plain = signal_gate("p", "builds", None, None);
        assert!(check(std::slice::from_ref(&plain), &setup).ready);
        setup.verdicts = vec![verdict("builds", HEAD, "fail", 2)];
        assert_eq!(
            reason(&check(std::slice::from_ref(&plain), &setup), "p"),
            "builds recorded fail, not pass"
        );
        let bounded = signal_gate("p", "builds", Some(json!(5)), None);
        assert_eq!(
            reason(&check(&[bounded], &setup), "p"),
            "gate misconfigured: pass reviewers take no bound"
        );
    }

    #[test]
    fn a_bound_of_the_wrong_type_is_misconfigured() {
        let setup = Setup::new(
            vec![
                reviewer("code-review", ReviewerSignal::Score),
                reviewer("security", ReviewerSignal::Level),
            ],
            vec![
                verdict("code-review", HEAD, "80", 1),
                verdict("security", HEAD, "low", 2),
            ],
        );
        let score_word = signal_gate("a", "code-review", Some(json!("high")), None);
        assert_eq!(
            reason(&check(&[score_word], &setup), "a"),
            "gate misconfigured: code-review is a score reviewer, so its bound is a whole number from 0 to 100"
        );
        let level_number = signal_gate("b", "security", Some(json!(5)), None);
        assert_eq!(
            reason(&check(&[level_number], &setup), "b"),
            "gate misconfigured: security is a level reviewer, so its bound is low, medium or high"
        );
        let unbounded = signal_gate("c", "security", None, None);
        assert_eq!(
            reason(&check(&[unbounded], &setup), "c"),
            "gate misconfigured: a signal gate on security needs max or min"
        );
    }

    #[test]
    fn a_signal_without_a_current_verdict_blocks_and_is_missing() {
        let setup = Setup::new(
            vec![reviewer("code-review", ReviewerSignal::Score)],
            vec![verdict("code-review", OLD, "80", 1)],
        );
        let status = check(
            &[signal_gate("s", "code-review", Some(json!(90)), None)],
            &setup,
        );
        assert_eq!(reason(&status, "s"), "no current verdict from code-review");
        assert_eq!(status.missing[0].reason, MissingReason::Stale);
    }

    #[test]
    fn a_behind_branch_blocks_and_an_up_to_date_one_passes() {
        let mut setup = Setup::new(vec![], vec![]);
        let up = gate("u", GateKind::UpToDate);
        setup.behind = Some(2);
        let status = check(std::slice::from_ref(&up), &setup);
        assert_eq!(reason(&status, "u"), "2 commit(s) behind its parent");
        setup.behind = None;
        assert!(check(std::slice::from_ref(&up), &setup).ready);
        setup.behind = Some(0);
        assert!(check(&[up], &setup).ready);
    }

    #[test]
    fn an_unapproved_branch_blocks_human_approval() {
        let mut setup = Setup::new(vec![], vec![]);
        setup.approved = false;
        let approval = gate("h", GateKind::HumanApproval);
        let status = check(std::slice::from_ref(&approval), &setup);
        assert_eq!(reason(&status, "h"), "you have not approved 1234567");
        setup.approved = true;
        assert!(check(&[approval], &setup).ready);
    }

    #[test]
    fn a_gate_for_another_branch_is_not_applicable_and_passes() {
        let setup = Setup::new(vec![], vec![]);
        let mut only_fixes = fresh("f", &["code-review"]);
        only_fixes.gate.when.branches = vec!["fix/*".to_owned()];
        let status = check(&[only_fixes], &setup);
        let entry = outcome(&status, "f");
        assert!(!entry.applicable && entry.passed);
        assert_eq!(
            entry.not_applicable.as_deref(),
            Some("the branch does not match fix/*")
        );
        assert!(status.ready && status.missing.is_empty());
    }

    #[test]
    fn a_gate_whose_paths_the_change_does_not_touch_is_not_applicable() {
        let mut setup = Setup::new(vec![], vec![]);
        setup.changed = Some(vec!["README.md".to_owned()]);
        let mut billing = gate("b", GateKind::UpToDate);
        billing.gate.when.paths = vec!["services/**".to_owned()];
        let status = check(std::slice::from_ref(&billing), &setup);
        assert_eq!(
            outcome(&status, "b").not_applicable.as_deref(),
            Some("no changed file matches services/**")
        );
        setup.changed = Some(vec!["services/billing/api.rs".to_owned()]);
        let status = check(std::slice::from_ref(&billing), &setup);
        assert!(outcome(&status, "b").applicable);
        setup.behind = Some(1);
        assert!(!check(&[billing], &setup).ready);
    }

    #[test]
    fn an_unknown_change_applies_a_gate_with_paths() {
        let mut setup = Setup::new(vec![], vec![]);
        setup.changed = None;
        let mut billing = fresh("b", &["code-review"]);
        billing.gate.when.paths = vec!["services/**".to_owned()];
        let status = check(&[billing], &setup);
        assert!(outcome(&status, "b").applicable);
        assert!(!status.ready);
    }

    #[test]
    fn a_reviewer_whose_conditions_do_not_apply_is_skipped() {
        let mut docs = reviewer("docs", ReviewerSignal::Score);
        docs.paths = vec!["docs/**".to_owned()];
        let setup = Setup::new(vec![docs], vec![]);
        let status = check(&[fresh("review", &["docs"])], &setup);
        assert!(status.ready, "{status:?}");
        assert!(status.missing.is_empty());
    }

    #[test]
    fn a_missing_reviewer_is_listed_once_by_name_with_its_model() {
        let mut slow = reviewer("slow", ReviewerSignal::Score);
        slow.subject = ReviewerSubject::Staged;
        slow.model = None;
        let setup = Setup::new(
            vec![reviewer("code-review", ReviewerSignal::Score), slow],
            vec![],
        );
        let gates = [
            signal_gate("b", "slow", Some(json!(9)), None),
            fresh("a", &["slow", "code-review"]),
        ];
        let status = check(&gates, &setup);
        let names: Vec<&str> = status.missing.iter().map(|m| m.reviewer.as_str()).collect();
        assert_eq!(names, ["code-review", "slow"]);
        assert_eq!(status.missing[1].subject, ReviewerSubject::Staged);
        assert_eq!(status.missing[1].model, None);
    }

    #[test]
    fn gates_are_listed_by_id_and_the_op_filters_them() {
        let setup = Setup::new(vec![], vec![]);
        let mut push_only = gate("z", GateKind::HumanApproval);
        push_only.gate.on = vec![GateOp::Push];
        let mut restack_only = gate("m", GateKind::UpToDate);
        restack_only.gate.on = vec![GateOp::Restack];
        let gates = [push_only, restack_only];
        let all = evaluate(&gates, None, &setup.facts());
        let ids: Vec<&str> = all.gates.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["m", "z"]);
        let pushes = evaluate(&gates, Some(GateOp::Push), &setup.facts());
        assert_eq!(pushes.gates.len(), 1);
        assert_eq!(pushes.gates[0].id, "z");
        assert_eq!(all.branch, "feat/x");
        assert_eq!(all.head, HEAD);
    }

    /// Writes `text` as `gates.json` in `dir`.
    fn write(dir: &Path, text: &str) {
        std::fs::create_dir_all(dir).expect("dir");
        std::fs::write(dir.join(FILE_NAME), text).expect("write");
    }

    const GOOD: &str = r#"{"gates": [
        {"id": "review", "kind": "fresh_verdict", "reviewers": ["code-review"], "on": ["push"]},
        {"id": "no-high", "kind": "findings", "severity": "high", "on": ["push", "restack"]}
    ]}"#;

    #[test]
    fn a_valid_file_parses_into_gates_with_their_layer() {
        let gates = parse(GOOD, GateLayer::User).expect("parses");
        assert_eq!(gates.len(), 2);
        assert_eq!(gates[0].layer, GateLayer::User);
        assert_eq!(
            gates[1].gate,
            GateDef {
                id: "no-high".to_owned(),
                kind: GateKind::Findings {
                    severity: ReviewSeverity::High
                },
                on: vec![GateOp::Push, GateOp::Restack],
                when: When::default(),
            }
        );
    }

    #[test]
    fn a_full_gate_reads_its_conditions_and_bounds() {
        let text = r#"{"gates": [{
            "id": "tests", "kind": "signal", "reviewer": "code-review", "min": 60,
            "on": ["restack"], "when": {"branches": ["feat/billing-*"], "paths": ["services/billing/**"]}
        }]}"#;
        let gates = parse(text, GateLayer::Repository).expect("parses");
        assert_eq!(gates[0].gate.when.branches, ["feat/billing-*"]);
        assert_eq!(gates[0].gate.when.paths, ["services/billing/**"]);
        assert_eq!(
            gates[0].gate.kind,
            GateKind::Signal {
                reviewer: "code-review".to_owned(),
                max: None,
                min: Some(json!(60)),
            }
        );
    }

    #[test]
    fn a_signal_gate_without_a_bound_parses_for_a_pass_reviewer() {
        let text = r#"{"gates": [{"id": "builds", "kind": "signal", "reviewer": "builds", "on": ["push"]}]}"#;
        let gates = parse(text, GateLayer::Repository).expect("parses");
        assert_eq!(
            gates[0].gate.kind,
            GateKind::Signal {
                reviewer: "builds".to_owned(),
                max: None,
                min: None,
            }
        );
    }

    #[test]
    fn every_parse_problem_is_reported_not_only_the_first() {
        let text = r#"{"gates": [
            {"id": "Bad_ID", "kind": "findings", "severity": "high", "on": ["push"]},
            {"id": "dup", "kind": "up_to_date", "on": ["push"]},
            {"id": "dup", "kind": "up_to_date", "on": ["push"]},
            {"id": "spin", "kind": "spin", "on": ["push"]},
            {"id": "extra", "kind": "up_to_date", "on": ["push"], "colour": "red"},
            {"id": "noop", "kind": "up_to_date", "on": []},
            {"id": "merge", "kind": "up_to_date", "on": ["merge"]},
            {"id": "paths", "kind": "up_to_date", "on": ["push"], "when": {"paths": ["/abs/**"]}},
            {"id": "fresh", "kind": "fresh_verdict", "reviewers": [], "on": ["push"]},
            {"id": "many", "kind": "fresh_verdict", "reviewers": ["a","b","c","d","e","f","g","h","i","j","k"], "on": ["push"]},
            {"id": "frac", "kind": "signal", "reviewer": "x", "max": 50.5, "on": ["push"]},
            {"id": "big", "kind": "signal", "reviewer": "x", "max": 101, "on": ["push"]},
            {"id": "sev", "kind": "findings", "severity": "critical", "on": ["push"]},
            {"kind": "up_to_date", "on": ["push"]},
            {"id": "nokind", "on": ["push"]}
        ]}"#;
        let errors = parse(text, GateLayer::Repository).expect_err("refused");
        let has = |needle: &str| errors.iter().any(|e| e.contains(needle));
        assert!(has("gate `Bad_ID`"), "{errors:?}");
        assert!(has("gate `dup`: duplicate id"), "{errors:?}");
        assert!(has("unknown kind `spin`"), "{errors:?}");
        assert!(has("unknown field `colour`"), "{errors:?}");
        assert!(has("`on` is empty"), "{errors:?}");
        assert!(
            has("`on` takes push or restack, not \"merge\""),
            "{errors:?}"
        );
        assert!(has("`when.paths` pattern `/abs/**`"), "{errors:?}");
        assert!(has("`reviewers` names at least one reviewer"), "{errors:?}");
        assert!(has("`reviewers` names 11 reviewers"), "{errors:?}");
        assert!(
            has("`max` must be a whole number from 0 to 100"),
            "{errors:?}"
        );
        assert!(has("`severity` must be low, medium or high"), "{errors:?}");
        assert!(has("gate 14: `id` is required"), "{errors:?}");
        assert!(has("gate `nokind`: `kind` is required"), "{errors:?}");
    }

    #[test]
    fn the_file_shape_and_the_limits_are_checked() {
        assert!(parse("not json", GateLayer::User).unwrap_err()[0].starts_with("not JSON"));
        assert!(parse("[]", GateLayer::User).unwrap_err()[0].contains("must be a JSON object"));
        assert!(parse("{}", GateLayer::User).unwrap_err()[0].starts_with("missing `gates`"));
        assert!(
            parse(r#"{"gates": {}}"#, GateLayer::User).unwrap_err()[0].contains("must be a list")
        );
        assert!(
            parse(r#"{"gates": [], "x": 1}"#, GateLayer::User).unwrap_err()[0]
                .contains("unknown key `x`")
        );
        assert_eq!(parse(r#"{"gates": []}"#, GateLayer::User), Ok(vec![]));
        let many: Vec<String> = (0..51)
            .map(|i| format!(r#"{{"id": "g{i}", "kind": "up_to_date", "on": ["push"]}}"#))
            .collect();
        let text = format!(r#"{{"gates": [{}]}}"#, many.join(","));
        assert!(parse(&text, GateLayer::User).unwrap_err()[0].contains("51 gates, the most is 50"));
        let bounds = |max: &str| {
            format!(
                r#"{{"gates": [{{"id": "s", "kind": "signal", "reviewer": "x", "max": {max}, "on": ["push"]}}]}}"#
            )
        };
        for good in ["0", "100", "\"low\"", "\"high\""] {
            assert!(parse(&bounds(good), GateLayer::User).is_ok(), "{good}");
        }
        for bad in ["-1", "\"critical\"", "true", "[1]"] {
            assert!(parse(&bounds(bad), GateLayer::User).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_repository_gate_wins_over_a_user_gate_with_the_same_id() {
        let root = tempfile::tempdir().expect("tempdir");
        let repo = root.path().join("repo");
        let user = root.path().join("user");
        write(
            &repo,
            r#"{"gates": [{"id": "no-high", "kind": "findings", "severity": "low", "on": ["push"]}]}"#,
        );
        write(
            &user,
            r#"{"gates": [
                {"id": "no-high", "kind": "findings", "severity": "high", "on": ["push"]},
                {"id": "only-user", "kind": "up_to_date", "on": ["restack"]}
            ]}"#,
        );
        let loaded = load(Some(&repo), Some(&user));
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let ids: Vec<(&str, GateLayer)> = loaded
            .gates
            .iter()
            .map(|g| (g.gate.id.as_str(), g.layer))
            .collect();
        assert_eq!(
            ids,
            [
                ("no-high", GateLayer::Repository),
                ("only-user", GateLayer::User)
            ]
        );
        assert_eq!(
            loaded.gates[0].gate.kind,
            GateKind::Findings {
                severity: ReviewSeverity::Low
            }
        );
    }

    #[test]
    fn a_malformed_file_contributes_nothing_and_its_errors_name_it() {
        let root = tempfile::tempdir().expect("tempdir");
        let repo = root.path().join("repo");
        let user = root.path().join("user");
        write(
            &repo,
            r#"{"gates": [{"id": "broken", "kind": "nope", "on": ["push"]}]}"#,
        );
        write(&user, GOOD);
        let loaded = load(Some(&repo), Some(&user));
        let ids: Vec<&str> = loaded.gates.iter().map(|g| g.gate.id.as_str()).collect();
        assert_eq!(ids, ["no-high", "review"]);
        assert_eq!(loaded.errors.len(), 1);
        let shown = repo.join(FILE_NAME).display().to_string();
        assert!(
            loaded.errors[0].starts_with(&format!("{shown}: gate `broken`")),
            "{:?}",
            loaded.errors
        );
    }

    #[test]
    fn missing_directories_are_no_gates_and_no_error() {
        let root = tempfile::tempdir().expect("tempdir");
        let loaded = load(Some(&root.path().join("nope")), None);
        assert_eq!(loaded, LoadedGates::default());
    }

    #[test]
    fn a_big_file_and_a_non_file_are_errors() {
        let root = tempfile::tempdir().expect("tempdir");
        write(root.path(), &" ".repeat(256 * 1024 + 1));
        let loaded = load(Some(root.path()), None);
        assert!(
            loaded.errors[0].ends_with("the file is over 256 KiB"),
            "{:?}",
            loaded.errors
        );
        std::fs::remove_file(root.path().join(FILE_NAME)).expect("remove");
        std::fs::create_dir(root.path().join(FILE_NAME)).expect("dir");
        let loaded = load(Some(root.path()), None);
        assert!(
            loaded.errors[0].ends_with("not a file"),
            "{:?}",
            loaded.errors
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_gate_file_is_an_error() {
        let root = tempfile::tempdir().expect("tempdir");
        let elsewhere = root.path().join("elsewhere.json");
        std::fs::write(&elsewhere, GOOD).expect("write");
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).expect("dir");
        std::os::unix::fs::symlink(&elsewhere, repo.join(FILE_NAME)).expect("symlink");
        let loaded = load(Some(&repo), None);
        assert!(loaded.gates.is_empty());
        assert!(
            loaded.errors[0].ends_with("a symbolic link is not read"),
            "{:?}",
            loaded.errors
        );
    }

    #[test]
    fn save_writes_a_file_that_loads_back_as_the_same_gates() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("gitmaster");
        let gates = parse(GOOD, GateLayer::Repository).expect("parses");
        let defs: Vec<GateDef> = gates.iter().map(|g| g.gate.clone()).collect();
        save(&dir, &defs).expect("save");
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).expect("read");
        assert!(text.starts_with("{\n  \"gates\": ["), "{text}");
        assert!(text.ends_with("}\n"));
        let loaded = load(Some(&dir), None);
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let mut expected = gates;
        expected.sort_by(|a, b| a.gate.id.cmp(&b.gate.id));
        assert_eq!(loaded.gates, expected);
        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .expect("dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, [FILE_NAME], "no temporary file is left");
    }

    #[test]
    fn save_refuses_a_gate_that_would_not_read_back() {
        let root = tempfile::tempdir().expect("tempdir");
        let bad = GateDef {
            id: "Bad Id".to_owned(),
            kind: GateKind::UpToDate,
            on: vec![GateOp::Push],
            when: When::default(),
        };
        assert!(
            save(root.path(), &[bad])
                .unwrap_err()
                .contains("cannot save gates")
        );
        assert!(!root.path().join(FILE_NAME).exists());
    }

    /// The world a property test builds: reviewers, verdicts, findings and gates in one place,
    /// so the test can give the same facts in another order.
    #[derive(Clone, Debug)]
    struct World {
        gates: Vec<LayeredGate>,
        reviewers: Vec<Reviewer>,
        verdicts: Vec<Verdict>,
        findings: Vec<FindingFact>,
        behind: Option<u32>,
        approved: bool,
    }

    const NAMES: [&str; 3] = ["alpha", "beta", "gamma"];

    fn reviewer_of(index: usize) -> Reviewer {
        let signal = [
            ReviewerSignal::Score,
            ReviewerSignal::Level,
            ReviewerSignal::Pass,
        ][index % 3];
        reviewer(NAMES[index % 3], signal)
    }

    fn value_for(index: usize, seed: u8) -> String {
        match index % 3 {
            0 => (u16::from(seed) % 101).to_string(),
            1 => ["low", "medium", "high"][usize::from(seed) % 3].to_owned(),
            _ => ["pass", "fail"][usize::from(seed) % 2].to_owned(),
        }
    }

    fn gate_of(choice: usize, position: usize) -> LayeredGate {
        let id = format!("g{position:02}");
        let name = NAMES[choice % 3];
        match choice % 6 {
            0 => fresh(&id, &["alpha", "beta"]),
            1 => fresh(&id, &["gamma"]),
            2 => gate(
                &id,
                GateKind::Findings {
                    severity: ReviewSeverity::Medium,
                },
            ),
            3 => signal_gate(&id, name, Some(json!(60)), None),
            4 => signal_gate(&id, "beta", None, Some(json!("medium"))),
            _ => gate(&id, GateKind::HumanApproval),
        }
    }

    fn build(
        gate_choices: &[usize],
        verdict_specs: &[(usize, bool, u8, i64)],
        finding_specs: &[(usize, usize)],
        behind: Option<u32>,
        approved: bool,
    ) -> World {
        World {
            gates: gate_choices
                .iter()
                .enumerate()
                .map(|(position, choice)| gate_of(*choice, position))
                .collect(),
            reviewers: (0..3).map(reviewer_of).collect(),
            verdicts: verdict_specs
                .iter()
                .enumerate()
                .map(|(position, (reviewer, current, seed, at))| {
                    let head = if *current { HEAD } else { OLD };
                    let at = *at * 10 + i64::try_from(position).unwrap_or(0);
                    let mut v =
                        verdict(NAMES[*reviewer % 3], head, &value_for(*reviewer, *seed), at);
                    v.signal = reviewer_of(*reviewer).signal;
                    v
                })
                .collect(),
            findings: finding_specs
                .iter()
                .map(|(severity, status)| FindingFact {
                    severity: [
                        ReviewSeverity::Low,
                        ReviewSeverity::Medium,
                        ReviewSeverity::High,
                    ][*severity % 3],
                    status: [
                        ReviewStatus::Open,
                        ReviewStatus::Handled,
                        ReviewStatus::Resolved,
                    ][*status % 3],
                })
                .collect(),
            behind,
            approved,
        }
    }

    impl World {
        fn status(&self) -> GatesStatus {
            let changed = vec!["src/lib.rs".to_owned()];
            evaluate(
                &self.gates,
                None,
                &GateFacts {
                    branch: "feat/x",
                    head: HEAD,
                    changed_paths: Some(changed.as_slice()),
                    reviewers: &self.reviewers,
                    verdicts: &self.verdicts,
                    findings: &self.findings,
                    behind_parent: self.behind,
                    approved: self.approved,
                },
            )
        }

        /// The same world with every list reversed and rotated.
        fn shuffled(&self, turn: usize) -> World {
            fn turned<T: Clone>(items: &[T], turn: usize) -> Vec<T> {
                let mut out: Vec<T> = items.iter().rev().cloned().collect();
                let len = out.len();
                if len > 0 {
                    out.rotate_left(turn % len);
                }
                out
            }
            World {
                gates: turned(&self.gates, turn),
                reviewers: turned(&self.reviewers, turn),
                verdicts: turned(&self.verdicts, turn),
                findings: turned(&self.findings, turn),
                ..self.clone()
            }
        }
    }

    proptest! {
        #[test]
        fn evaluate_does_not_depend_on_the_order_of_its_inputs(
            gate_choices in prop::collection::vec(0usize..6, 0..6),
            verdict_specs in prop::collection::vec((0usize..3, any::<bool>(), any::<u8>(), 0i64..5), 0..6),
            finding_specs in prop::collection::vec((0usize..3, 0usize..3), 0..5),
            behind in prop::option::of(0u32..3),
            approved in any::<bool>(),
            turn in 0usize..8,
        ) {
            let world = build(&gate_choices, &verdict_specs, &finding_specs, behind, approved);
            prop_assert_eq!(world.status(), world.shuffled(turn).status());
        }

        #[test]
        fn a_verdict_for_another_head_never_passes_a_fresh_or_signal_gate(
            gate_choices in prop::collection::vec(0usize..6, 1..6),
            verdict_specs in prop::collection::vec((0usize..3, any::<bool>(), any::<u8>(), 0i64..5), 0..6),
        ) {
            let stale: Vec<(usize, bool, u8, i64)> = verdict_specs
                .iter()
                .map(|(reviewer, _, seed, at)| (*reviewer, false, *seed, *at))
                .collect();
            let world = build(&gate_choices, &stale, &[], Some(0), true);
            for outcome in world.status().gates {
                if outcome.kind == "fresh_verdict" || outcome.kind == "signal" {
                    prop_assert!(!outcome.passed, "{outcome:?}");
                }
            }
        }

        #[test]
        fn a_gate_that_does_not_apply_never_changes_ready(
            gate_choices in prop::collection::vec(0usize..6, 0..6),
            verdict_specs in prop::collection::vec((0usize..3, any::<bool>(), any::<u8>(), 0i64..5), 0..6),
            finding_specs in prop::collection::vec((0usize..3, 0usize..3), 0..5),
            approved in any::<bool>(),
        ) {
            let mut world = build(&gate_choices, &verdict_specs, &finding_specs, Some(0), approved);
            let without = world.status();
            let mut other = gate("zz-release", GateKind::HumanApproval);
            other.gate.when.branches = vec!["release/*".to_owned()];
            world.gates.push(other);
            let with = world.status();
            prop_assert_eq!(with.ready, without.ready);
            let entry = with.gates.iter().find(|g| g.id == "zz-release").expect("listed");
            prop_assert!(!entry.applicable && entry.passed);
        }

        #[test]
        fn an_unresolved_finding_never_unblocks_a_findings_gate(
            severity in 0usize..3,
            finding_specs in prop::collection::vec((0usize..3, 0usize..3), 0..5),
            extra_severity in 0usize..3,
            extra_status in prop::sample::select(vec![ReviewStatus::Open, ReviewStatus::Handled]),
        ) {
            let gate_severity = [ReviewSeverity::Low, ReviewSeverity::Medium, ReviewSeverity::High][severity];
            let gates = [gate("f", GateKind::Findings { severity: gate_severity })];
            let mut setup = Setup::new(vec![], vec![]);
            setup.findings = build(&[], &[], &finding_specs, None, true).findings;
            let before = check(&gates, &setup);
            setup.findings.push(FindingFact {
                severity: [ReviewSeverity::Low, ReviewSeverity::Medium, ReviewSeverity::High][extra_severity],
                status: extra_status,
            });
            let after = check(&gates, &setup);
            if !before.ready {
                prop_assert!(!after.ready);
            }
        }
    }
}

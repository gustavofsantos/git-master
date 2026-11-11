//! The code health tools (docs/guide/code-health.md, D225): what an agent reads about how the
//! files it is about to change have changed over time. Each is the engine's `LoadHealth` run as
//! the agent; the indicators come from the history at a commit and the configuration, and these
//! tools only translate arguments and results. Both are read-only.

use std::collections::HashMap;

use gm_schema::{CommandKind, Event, FileHealth, HealthReport, Trend};
use serde_json::{Value, json};

use crate::session::Session;
use crate::tools::{ToolDefinition, bounded, optional_str, optional_text, repository_path};

/// The most paths one `health_context` call asks about.
const CONTEXT_MAX_PATHS: usize = 50;
const HOTSPOTS_DEFAULT_LIMIT: i64 = 20;
const HOTSPOTS_MAX_LIMIT: i64 = 100;
/// The characters of the commit id a cursor keeps to say which commit its listing was made at.
const CURSOR_REV_CHARS: usize = 12;
/// The note of a report the index does not hold every commit of yet.
const PARTIAL_NOTE: &str = "The index does not hold every commit of the window yet, so these answers are partial; ask again later.";
/// The refusal of a cursor that does not belong to the listing it is passed to.
const CURSOR_MISMATCH: &str = "the cursor belongs to another listing (another rev, configuration or scope): start again without a cursor";

/// The definitions of the code health tools, in the order the server lists them.
pub(crate) fn definitions() -> Vec<ToolDefinition> {
    vec![health_context_definition(), hotspots_list_definition()]
}

fn health_context_definition() -> ToolDefinition {
    ToolDefinition {
        name: "health_context",
        description: "Code health of the files you are about to change, from the history of the repository: how much each file changed recently (`heat`, with its `rank` among `of` files and its `percentile`), whether that is rising, steady or falling (`trend`), whether a quiet file has started to change (`dormancy_break`), which other files change in the same commits (`changes_together`), and how widely the file is owned (`ownership`: its authors, the share of commits made by the most frequent one, and how many occasional authors there are). Call it with every path you plan to change, BEFORE you edit. Treat each entry of `constraints` as a requirement of your change. For a file with a `dormancy_break`, explain its current behavior back before you change it. The numbers are computed at `rev` (default HEAD) under the configuration named by `config_hash`: they are deterministic and are context for the change, not a score to optimize. The first call in a repository builds the index from its history and can take a while in a very large one; `complete` false, with a `note`, means the answers are partial. A path with no change in the analysis window is `known: false`. Read-only.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": CONTEXT_MAX_PATHS,
                    "items": { "type": "string", "minLength": 1 },
                    "description": "Repository-relative paths, such as src/billing/Invoice.java: 1 to 50. Not absolute, and no `..`."
                },
                "rev": { "type": "string", "description": "The commit to compute at, by any revision Git reads. Default HEAD." }
            },
            "required": ["paths"],
            "additionalProperties": false
        }),
        read_only: true,
    }
}

fn hotspots_list_definition() -> ToolDefinition {
    ToolDefinition {
        name: "hotspots_list",
        description: "The hottest files under a folder, or of the whole repository, hottest first, a page at a time: each with its `rank`, `percentile`, `value` (its heat), `trend` and whether it has a `dormancy_break`. Use it to see which files need care in a folder before you plan a change there, then call health_context for the files you will change. `scope` is a repository-relative folder such as src/billing; left out, it is the whole repository. `limit` is 1 to 100, 20 by default. When `has_more` is true, pass `next_cursor` back with the same arguments to read the next page; a cursor from another rev, configuration or scope is refused. The numbers are deterministic context, not a score to optimize. Read-only.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "scope": { "type": "string", "description": "A repository-relative folder, such as src/billing. Left out or empty, the whole repository." },
                "rev": { "type": "string", "description": "The commit to list at, by any revision Git reads. Default HEAD." },
                "limit": { "type": "integer", "minimum": 1, "maximum": HOTSPOTS_MAX_LIMIT, "default": HOTSPOTS_DEFAULT_LIMIT },
                "cursor": { "type": "string", "description": "next_cursor of the previous page, with the same rev, scope and configuration." }
            },
            "additionalProperties": false
        }),
        read_only: true,
    }
}

/// `health_context`: the indicators of each path asked about, in the order asked.
pub(crate) fn health_context(session: &mut Session, args: &Value) -> Result<Value, String> {
    let paths = context_paths(args)?;
    let rev = optional_text(args, "rev")?;
    let report = health_report(session, rev)?;
    let of = file_count(&report);
    let by_path: HashMap<&str, &FileHealth> = report
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let answers: Vec<Value> = paths
        .iter()
        .map(|path| path_json(path, by_path.get(path.as_str()).copied(), of))
        .collect();
    let mut result = json!({
        "rev": report.rev,
        "config_hash": report.config_hash,
        "untrusted": true,
        "complete": report.complete,
        "paths": answers,
    });
    if !report.complete {
        result["note"] = json!(PARTIAL_NOTE);
    }
    Ok(result)
}

/// `hotspots_list`: one page of the files under a scope, in rank order.
pub(crate) fn hotspots_list(session: &mut Session, args: &Value) -> Result<Value, String> {
    let scope = scope_arg(args)?;
    let rev = optional_text(args, "rev")?;
    let limit = bounded(args, "limit", 1, HOTSPOTS_MAX_LIMIT)?.unwrap_or(HOTSPOTS_DEFAULT_LIMIT);
    let cursor = optional_str(args, "cursor")?;
    let report = health_report(session, rev)?;
    let offset = match cursor {
        None => 0,
        Some(cursor) => cursor_offset(cursor, &report, &scope)?,
    };
    // A folder's files start with its name and a slash, so `src/a` never takes `src/ab`.
    let prefix = if scope.is_empty() {
        String::new()
    } else {
        format!("{scope}/")
    };
    let listed: Vec<&FileHealth> = report
        .files
        .iter()
        .filter(|file| file.path.starts_with(prefix.as_str()))
        .collect();
    let rest = listed.get(offset..).unwrap_or(&[]);
    let page: Vec<&FileHealth> = rest
        .iter()
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .copied()
        .collect();
    let has_more = rest.len() > page.len();
    let next_cursor = has_more.then(|| {
        format!(
            "{}:{}:{}:{scope}",
            offset + page.len(),
            short_rev(&report),
            report.config_hash
        )
    });
    let mut result = json!({
        "rev": report.rev,
        "config_hash": report.config_hash,
        "untrusted": true,
        "complete": report.complete,
        "files": page.iter().map(|file| hotspot_json(file)).collect::<Vec<_>>(),
        "has_more": has_more,
        "next_cursor": next_cursor,
    });
    if !report.complete {
        result["note"] = json!(PARTIAL_NOTE);
    }
    Ok(result)
}

/// The engine's report for `rev`. Its last `HealthLoaded` is the answer: the earlier ones, while
/// the index fills, may be partial. A failed read has already been refused by the session.
fn health_report(session: &mut Session, rev: Option<String>) -> Result<HealthReport, String> {
    session
        .run(CommandKind::LoadHealth { rev })?
        .into_iter()
        .filter_map(|event| match event {
            Event::HealthLoaded { report, .. } => Some(*report),
            _ => None,
        })
        .next_back()
        .ok_or_else(|| "the read returned no answer".to_owned())
}

/// The `paths` argument: 1 to 50 repository-relative paths, each checked as `provenance` checks its path.
fn context_paths(args: &Value) -> Result<Vec<String>, String> {
    let list = match args.get("paths") {
        None | Some(Value::Null) => {
            return Err("paths is required: 1 to 50 repository paths".to_owned());
        }
        Some(Value::Array(list)) => list,
        Some(_) => return Err("paths must be a list of repository paths".to_owned()),
    };
    if !(1..=CONTEXT_MAX_PATHS).contains(&list.len()) {
        return Err(format!(
            "paths must hold 1 to {CONTEXT_MAX_PATHS} paths, not {}",
            list.len()
        ));
    }
    let mut paths = Vec::with_capacity(list.len());
    for value in list {
        let Some(path) = value.as_str() else {
            return Err("each of paths must be a string".to_owned());
        };
        if path.is_empty() {
            return Err("a path must not be empty".to_owned());
        }
        repository_path(path)?;
        paths.push(path.to_owned());
    }
    Ok(paths)
}

/// The `scope` argument as a normalised folder: no leading, trailing or doubled slashes. Empty is
/// the whole repository.
fn scope_arg(args: &Value) -> Result<String, String> {
    let Some(scope) = optional_str(args, "scope")? else {
        return Ok(String::new());
    };
    let folder = scope
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if !folder.is_empty() {
        repository_path(&folder)?;
    }
    Ok(folder)
}

/// The offset a cursor names, when it was made for this report, this configuration and this scope.
fn cursor_offset(cursor: &str, report: &HealthReport, scope: &str) -> Result<usize, String> {
    let mut parts = cursor.splitn(4, ':');
    let (Some(offset), Some(rev), Some(hash), Some(cursor_scope)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(CURSOR_MISMATCH.to_owned());
    };
    let offset = offset
        .parse::<usize>()
        .map_err(|_| CURSOR_MISMATCH.to_owned())?;
    if rev == short_rev(report) && hash == report.config_hash && cursor_scope == scope {
        Ok(offset)
    } else {
        Err(CURSOR_MISMATCH.to_owned())
    }
}

fn short_rev(report: &HealthReport) -> &str {
    report
        .rev
        .get(..CURSOR_REV_CHARS)
        .unwrap_or(report.rev.as_str())
}

fn file_count(report: &HealthReport) -> u32 {
    u32::try_from(report.files.len()).unwrap_or(u32::MAX)
}

/// One path of a `health_context` answer. A path the report does not hold is unknown, with no
/// indicators and no constraints.
fn path_json(path: &str, file: Option<&FileHealth>, of: u32) -> Value {
    let Some(file) = file else {
        return json!({ "path": path, "known": false, "constraints": [] });
    };
    json!({
        "path": path,
        "known": true,
        "heat": {
            "rank": file.rank,
            "of": of,
            "percentile": file.percentile,
            "value": round2(file.heat),
            "trend": trend_name(file.trend),
        },
        "dormancy_break": file.dormancy_break.as_ref().map(|dormancy| json!({
            "quiet_active_days": dormancy.quiet_active_days,
            "recent_active_days": dormancy.recent_active_days,
        })),
        "changes_together": file.changes_together.iter().map(|partner| json!({
            "path": partner.path,
            "degree": partner.degree,
            "shared_commits": partner.shared_commits,
        })).collect::<Vec<_>>(),
        "ownership": {
            "authors": file.ownership.authors,
            "top_author_share": file.ownership.top_author_share,
            "minor_contributors": file.ownership.minor_contributors,
        },
        "constraints": file.constraints,
    })
}

/// One row of a `hotspots_list` page.
fn hotspot_json(file: &FileHealth) -> Value {
    json!({
        "path": file.path,
        "rank": file.rank,
        "percentile": file.percentile,
        "value": round2(file.heat),
        "trend": trend_name(file.trend),
        "dormancy_break": file.dormancy_break.is_some(),
    })
}

fn trend_name(trend: Trend) -> &'static str {
    match trend {
        Trend::Rising => "rising",
        Trend::Steady => "steady",
        Trend::Falling => "falling",
    }
}

/// Heat to two decimals, as the answers show it.
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

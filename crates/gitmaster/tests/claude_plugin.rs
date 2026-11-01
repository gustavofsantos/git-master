//! The Claude Code plugin is source in this repository (D150). These tests read its files from
//! the repository root, as `claude plugin marketplace add` reads them, and check what the
//! hooks and the MCP settings must say for the app to capture sessions and serve its tools.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The command every hook runs. `gitmaster` comes from `PATH`, so the file has no absolute path.
const HOOK_COMMAND: &str = "gitmaster thread hook --harness claude-code";

/// The events the plugin hooks, sorted. Each one is a capture point of design/threads.md.
const EVENTS: [&str; 7] = [
    "PostToolUse",
    "PreToolUse",
    "SessionEnd",
    "SessionStart",
    "Stop",
    "SubagentStop",
    "UserPromptSubmit",
];

/// The tools the skill names. Each one is served by `gitmaster mcp` (docs/agents/mcp-server.md).
const SKILL_TOOLS: [&str; 5] = [
    "review_comments_list",
    "review_comment_resolve",
    "threads_search",
    "thread_get",
    "thread_read",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn plugin_dir() -> PathBuf {
    repo_root().join("plugins/claude-code")
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&read_text(path))
        .unwrap_or_else(|error| panic!("{} is not valid JSON: {error}", path.display()))
}

/// Every file under `dir`, in name order within each folder.
fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn the_marketplace_names_the_plugin_at_a_path_that_exists() {
    let marketplace = read_json(&repo_root().join(".claude-plugin/marketplace.json"));
    assert_eq!(marketplace["name"], "gitmaster");
    assert_eq!(marketplace["owner"]["name"], "GitMaster");

    let plugins = marketplace["plugins"].as_array().expect("plugins array");
    assert_eq!(plugins.len(), 1, "one plugin in the marketplace");
    assert_eq!(plugins[0]["name"], "gitmaster");
    let source = plugins[0]["source"].as_str().expect("source path");
    assert_eq!(source, "./plugins/claude-code");
    let dir = repo_root().join(source);
    assert!(
        dir.is_dir(),
        "marketplace source {} is missing",
        dir.display()
    );
}

#[test]
fn the_plugin_manifest_matches_the_app_version() {
    let manifest = read_json(&plugin_dir().join(".claude-plugin/plugin.json"));
    assert_eq!(manifest["name"], "gitmaster");
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        manifest["description"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "{manifest}"
    );
}

#[test]
fn every_json_file_of_the_plugin_is_valid_json() {
    let mut files = Vec::new();
    files_under(&plugin_dir(), &mut files);
    let json_files: Vec<&PathBuf> = files
        .iter()
        .filter(|path| path.extension() == Some(OsStr::new("json")))
        .collect();
    assert!(json_files.len() >= 3, "{json_files:?}");
    for path in json_files {
        read_json(path);
    }
}

#[test]
fn the_hooks_cover_the_capture_events_with_the_hook_command() {
    let hooks = read_json(&plugin_dir().join("hooks/hooks.json"));
    let events = hooks["hooks"].as_object().expect("hooks object");
    let mut names: Vec<&str> = events.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, EVENTS);

    for event in EVENTS {
        let group = &events[event][0];
        let hook = &group["hooks"][0];
        assert_eq!(hook["type"], "command", "{event}");
        assert_eq!(hook["command"], HOOK_COMMAND, "{event}");
        assert_eq!(hook["timeout"], 5, "{event}");
    }
    assert_eq!(
        events["PreToolUse"][0]["matcher"],
        "Edit|MultiEdit|Write|NotebookEdit"
    );
    assert_eq!(events["PostToolUse"][0]["matcher"], "*");
    assert!(events["Stop"][0].get("matcher").is_none());
}

#[test]
fn the_mcp_server_is_gitmaster_mcp_from_the_path() {
    let mcp = read_json(&plugin_dir().join(".mcp.json"));
    assert_eq!(
        mcp,
        json!({"mcpServers": {"gitmaster": {"command": "gitmaster", "args": ["mcp"]}}})
    );
}

#[test]
fn the_skill_is_named_gitmaster_and_names_the_tools() {
    let skill = read_text(&plugin_dir().join("skills/gitmaster/SKILL.md"));
    assert!(skill.starts_with("---\nname: gitmaster\n"), "{skill}");
    for tool in SKILL_TOOLS {
        assert!(skill.contains(tool), "skill does not name {tool}");
    }
}

#[test]
fn no_plugin_file_bakes_in_a_path_of_this_checkout() {
    let root = repo_root().canonicalize().expect("repository root");
    let mut files = Vec::new();
    files_under(&plugin_dir(), &mut files);
    files.push(repo_root().join(".claude-plugin/marketplace.json"));
    for path in files {
        let text = read_text(&path);
        assert!(
            !text.contains(&root.display().to_string()),
            "{} names the checkout path",
            path.display()
        );
    }
}

//! The Cursor plugin is source in the `git-master-ai` repository, included here as the `ai/`
//! submodule (D194, D255). These tests read its files from the submodule, as
//! `scripts/install-local.sh cursor` copies them, and check what the hooks and the MCP settings
//! must say for the app to capture sessions and serve its tools, and that the plugin carries
//! every skill and sub-agent of the Claude Code plugin. They fail, they do not skip, when the
//! submodule is not checked out.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The command every hook runs. `gitmaster` comes from `PATH`, so the file has no absolute path.
const HOOK_COMMAND: &str = "gitmaster thread hook --harness cursor";

/// The events the plugin hooks, sorted. Each one is mapped by the `cursor` adapter (D255).
const EVENTS: [&str; 9] = [
    "afterAgentResponse",
    "beforeSubmitPrompt",
    "postToolUse",
    "postToolUseFailure",
    "preToolUse",
    "sessionEnd",
    "sessionStart",
    "stop",
    "subagentStop",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `git-master-ai` checkout. A missing one is a failure with the command that fixes it.
fn ai_dir() -> PathBuf {
    let dir = repo_root().join("ai");
    assert!(
        dir.join("plugins.json").is_file(),
        "{} is empty: run `git submodule update --init`",
        dir.display()
    );
    dir
}

fn plugin_dir() -> PathBuf {
    ai_dir().join("plugins/cursor")
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&read_text(path))
        .unwrap_or_else(|error| panic!("{} is not valid JSON: {error}", path.display()))
}

/// `x.y.z` as a comparable triple.
fn semver(text: &str) -> (u64, u64, u64) {
    let parts: Vec<u64> = text
        .split('.')
        .map(|part| {
            part.parse()
                .unwrap_or_else(|_| panic!("{text} is not x.y.z"))
        })
        .collect();
    assert_eq!(parts.len(), 3, "{text} is not x.y.z");
    (parts[0], parts[1], parts[2])
}

/// The names of the entries of `dir`, sorted.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| {
            entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// The front matter of a skill or agent file: each `key: value` line between the two `---` lines.
fn front_matter(text: &str) -> Vec<(String, String)> {
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("---"),
        "the file opens with front matter"
    );
    lines
        .take_while(|line| *line != "---")
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn field<'a>(front: &'a [(String, String)], key: &str) -> Option<&'a str> {
    front
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

#[test]
fn the_plugin_is_named_gitmaster_with_its_own_version_and_needs_no_newer_app() {
    let manifest = read_json(&plugin_dir().join(".cursor-plugin/plugin.json"));
    assert_eq!(manifest["name"], "gitmaster");
    semver(manifest["version"].as_str().expect("version"));
    let registry = read_json(&ai_dir().join("plugins.json"));
    assert_eq!(
        registry["plugins"]["cursor"]["manifest"],
        "plugins/cursor/.cursor-plugin/plugin.json"
    );
    let min = registry["plugins"]["cursor"]["min_gitmaster"]
        .as_str()
        .expect("min_gitmaster");
    assert!(
        semver(min) <= semver(env!("CARGO_PKG_VERSION")),
        "the plugin needs gitmaster {min}, this is {}",
        env!("CARGO_PKG_VERSION")
    );
}

#[test]
fn the_marketplace_names_the_plugin_at_a_relative_path_that_exists() {
    let marketplace = read_json(&ai_dir().join(".cursor-plugin/marketplace.json"));
    assert_eq!(marketplace["name"], "gitmaster");
    let plugins = marketplace["plugins"].as_array().expect("plugins array");
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["name"], "gitmaster");
    let source = plugins[0]["source"].as_str().expect("source");
    assert!(
        !source.starts_with('/') && !source.contains(".."),
        "{source}"
    );
    assert!(ai_dir().join(source).is_dir(), "{source} is missing");
}

#[test]
fn the_hooks_cover_the_capture_events_with_the_hook_command() {
    let hooks = read_json(&plugin_dir().join("hooks/hooks.json"));
    assert_eq!(hooks["version"], 1);
    let events = hooks["hooks"].as_object().expect("hooks object");
    let mut names: Vec<&str> = events.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, EVENTS);
    for event in EVENTS {
        let hook = &events[event][0];
        assert_eq!(hook["command"], HOOK_COMMAND, "{event}");
        assert_eq!(hook["timeout"], 5, "{event}");
        // A hook that blocks on failure would let a GitMaster problem stop the agent.
        assert!(hook.get("failClosed").is_none(), "{event}");
        assert!(
            hook.get("type").is_none_or(|kind| kind == "command"),
            "{event}"
        );
    }
    assert_eq!(events["preToolUse"][0]["matcher"], "Write|Delete");
    assert_eq!(events["postToolUse"][0]["matcher"], "*");
    assert!(events["stop"][0].get("matcher").is_none());
}

#[test]
fn the_mcp_server_is_gitmaster_mcp_from_the_path_with_the_workspace_folder() {
    let mcp = read_json(&plugin_dir().join("mcp.json"));
    assert_eq!(
        mcp,
        json!({"mcpServers": {"gitmaster": {
            "command": "gitmaster",
            "args": ["mcp"],
            "env": {"GITMASTER_PROJECT_DIR": "${workspaceFolder}"}
        }}})
    );
}

/// Every skill and sub-agent of the Claude Code plugin has its Cursor counterpart, named with
/// the `gitmaster-` prefix because Cursor does not namespace plugin skills (D255 (2)).
#[test]
fn every_claude_code_skill_and_agent_has_a_cursor_counterpart() {
    let claude = ai_dir().join("plugins/claude-code");
    let cursor_name = |name: &str| {
        if name == "gitmaster" {
            name.to_owned()
        } else {
            format!("gitmaster-{name}")
        }
    };
    let expected: Vec<String> = names_in(&claude.join("skills"))
        .iter()
        .map(|name| cursor_name(name))
        .collect();
    let mut sorted = expected.clone();
    sorted.sort();
    assert_eq!(names_in(&plugin_dir().join("skills")), sorted);
    for skill in &sorted {
        let text = read_text(&plugin_dir().join("skills").join(skill).join("SKILL.md"));
        let front = front_matter(&text);
        assert_eq!(field(&front, "name"), Some(skill.as_str()));
        assert!(field(&front, "description").is_some_and(|text| !text.is_empty()));
        assert!(
            !text.contains("/gitmaster:"),
            "{skill} names a Claude Code command"
        );
        assert!(
            !text.contains("subagent_type"),
            "{skill} names a Claude Code tool"
        );
    }

    let agents: Vec<String> = names_in(&claude.join("agents"))
        .iter()
        .map(|file| format!("gitmaster-{file}"))
        .collect();
    assert_eq!(names_in(&plugin_dir().join("agents")), agents);
}

/// The sub-agents cannot be given a tool list in Cursor, so they are read-only and their
/// instructions allow only `GitMaster` tools; each can still propose a memory (D255 (3)).
#[test]
fn the_sub_agents_are_read_only_and_use_only_gitmaster_tools() {
    for name in ["gitmaster-memory-proposer", "gitmaster-memory-consolidator"] {
        let text = read_text(&plugin_dir().join("agents").join(format!("{name}.md")));
        let front = front_matter(&text);
        assert_eq!(field(&front, "name"), Some(name));
        assert_eq!(field(&front, "readonly"), Some("true"), "{name}");
        assert!(field(&front, "tools").is_none(), "{name}");
        assert!(text.contains("Your only actions are the GitMaster MCP tools"));
        assert!(
            text.contains("`memory_propose`"),
            "{name} proposes memories"
        );
        // The tools the Claude Code agent declares are the ones the Cursor agent may use.
        let claude_name = name.trim_start_matches("gitmaster-");
        let claude = read_text(
            &ai_dir()
                .join("plugins/claude-code/agents")
                .join(format!("{claude_name}.md")),
        );
        let declared = field(&front_matter(&claude), "tools").map(str::to_owned);
        for tool in declared.expect("claude tools").split(", ") {
            let bare = tool.rsplit("__").next().expect("tool name");
            assert!(
                text.contains(&format!("`{bare}`")),
                "{name} does not name {bare}"
            );
        }
    }
    let consolidator = read_text(&plugin_dir().join("agents/gitmaster-memory-consolidator.md"));
    assert!(
        !consolidator.contains("memory_accept") && !consolidator.contains("memory_reject"),
        "the consolidator cannot accept or reject"
    );
    let consolidate = read_text(&plugin_dir().join("skills/gitmaster-consolidate/SKILL.md"));
    assert!(consolidate.contains("gitmaster-memory-consolidator"));
    let propose = read_text(&plugin_dir().join("skills/gitmaster-propose-memories/SKILL.md"));
    assert!(propose.contains("gitmaster-memory-proposer"));
}

/// The review workflow starts Cursor's own command-line agent and hardcodes no permission flag,
/// as for Claude Code (D219, D255 (7)).
#[test]
fn the_write_reviewer_skill_runs_cursor_agent_without_permission_flags() {
    let skill = read_text(&plugin_dir().join("skills/gitmaster-write-reviewer/SKILL.md"));
    assert!(skill.contains(r#"ctx.exec({ "cursor-agent", "-p", prompt }"#));
    assert!(!skill.contains(r#""claude""#));
    assert!(skill.contains("scorecard_publish"));
    assert!(skill.contains("unreviewed until you approve"));
    for flag in ["--force", "--approve-mcps", "--yolo", "--sandbox"] {
        assert!(!skill.contains(flag), "the skill hardcodes {flag}");
    }
}

#[test]
fn no_plugin_file_bakes_in_a_path_of_this_checkout() {
    let root = repo_root().canonicalize().expect("repository root");
    let mut stack = vec![plugin_dir()];
    while let Some(dir) = stack.pop() {
        for name in names_in(&dir) {
            let path = dir.join(name);
            if path.is_dir() {
                stack.push(path);
            } else {
                assert!(
                    !read_text(&path).contains(&root.display().to_string()),
                    "{} names the checkout path",
                    path.display()
                );
            }
        }
    }
}

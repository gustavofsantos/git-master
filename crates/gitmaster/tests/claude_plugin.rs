//! The Claude Code plugin is source in the `git-master-ai` repository, included here as the
//! `ai/` submodule (D194). These tests read its files from the submodule, as `claude plugin
//! marketplace add` reads them, and check what the hooks and the MCP settings must say for the
//! app to capture sessions and serve its tools. They fail, they do not skip, when the submodule
//! is not checked out.

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
const SKILL_TOOLS: [&str; 7] = [
    "review_comments_list",
    "review_comment_create",
    "review_comment_resolve",
    "review_comment_reply",
    "threads_search",
    "thread_get",
    "thread_read",
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
    ai_dir().join("plugins/claude-code")
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
    let marketplace = read_json(&ai_dir().join(".claude-plugin/marketplace.json"));
    assert_eq!(marketplace["name"], "gitmaster");
    assert_eq!(marketplace["owner"]["name"], "GitMaster");

    let plugins = marketplace["plugins"].as_array().expect("plugins array");
    assert_eq!(plugins.len(), 1, "one plugin in the marketplace");
    assert_eq!(plugins[0]["name"], "gitmaster");
    let source = plugins[0]["source"].as_str().expect("source path");
    assert_eq!(source, "./plugins/claude-code");
    let dir = ai_dir().join(source);
    assert!(
        dir.is_dir(),
        "marketplace source {} is missing",
        dir.display()
    );
}

#[test]
fn the_plugin_has_its_own_version_and_needs_no_newer_app() {
    let manifest = read_json(&plugin_dir().join(".claude-plugin/plugin.json"));
    assert_eq!(manifest["name"], "gitmaster");
    semver(manifest["version"].as_str().expect("version"));
    let registry = read_json(&ai_dir().join("plugins.json"));
    let min = registry["plugins"]["claude-code"]["min_gitmaster"]
        .as_str()
        .expect("min_gitmaster");
    assert!(
        semver(min) <= semver(env!("CARGO_PKG_VERSION")),
        "the plugin needs gitmaster {min}, this is {}",
        env!("CARGO_PKG_VERSION")
    );
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
    files.push(ai_dir().join(".claude-plugin/marketplace.json"));
    for path in files {
        let text = read_text(&path);
        assert!(
            !text.contains(&root.display().to_string()),
            "{} names the checkout path",
            path.display()
        );
    }
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

fn field<'a>(front: &'a [(String, String)], key: &str) -> &'a str {
    let Some((_, value)) = front.iter().find(|(name, _)| name == key) else {
        panic!("front matter has no {key}");
    };
    value.as_str()
}

/// Where `needle` first appears in `text`; a missing needle is a failure that names it.
fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("the text does not say {needle}"))
}

/// The text after the line that opens a list, up to its first blank line.
fn list_after<'a>(text: &'a str, opener: &str) -> &'a str {
    let start = position(text, opener) + opener.len();
    let rest = &text[start..];
    rest.split("\n\n").next().expect("a list")
}

/// The two sub-agents that read `GitMaster` for a skill (D201 (2), (3)). Each is `haiku`, has only
/// `GitMaster` tools (no shell, no write) and can propose a memory, which is the only write it makes.
#[test]
fn the_sub_agents_are_haiku_and_hold_only_gitmaster_tools_and_propose() {
    for name in ["memory-proposer", "memory-consolidator"] {
        let text = read_text(&plugin_dir().join("agents").join(format!("{name}.md")));
        let front = front_matter(&text);
        assert_eq!(field(&front, "name"), name);
        assert_eq!(field(&front, "model"), "haiku", "{name}");
        let tools: Vec<&str> = field(&front, "tools").split(", ").collect();
        for tool in &tools {
            assert!(
                tool.starts_with("mcp__plugin_gitmaster_gitmaster__"),
                "{name} holds {tool}, which is not a GitMaster tool"
            );
        }
        assert!(
            tools.contains(&"mcp__plugin_gitmaster_gitmaster__memory_propose"),
            "{name} proposes memories"
        );
    }
    let consolidator = read_text(&plugin_dir().join("agents/memory-consolidator.md"));
    assert!(
        !consolidator.contains("memory_accept") && !consolidator.contains("memory_reject"),
        "the consolidator cannot accept or reject"
    );
}

/// The skills a person starts name the sub-agent they hand the reading to, by its namespaced type.
#[test]
fn the_skills_name_their_sub_agents_and_consolidate_is_a_skill_of_its_own() {
    let proposer = read_text(&plugin_dir().join("skills/propose-memories/SKILL.md"));
    assert!(proposer.contains("gitmaster:memory-proposer"));

    let consolidate = read_text(&plugin_dir().join("skills/consolidate/SKILL.md"));
    let front = front_matter(&consolidate);
    assert_eq!(field(&front, "name"), "consolidate");
    assert!(field(&front, "description").contains("/gitmaster:consolidate"));
    assert!(consolidate.contains("gitmaster:memory-consolidator"));
    assert!(consolidate.contains("Do not call `memory_propose`"));
}

/// `prepare` caps its memories (D201 (5)): at most 10 and about 2,000 characters, and it asks for
/// the relevance order, not the most-used one.
#[test]
fn prepare_caps_its_memories_in_count_and_size() {
    let prepare = read_text(&plugin_dir().join("skills/prepare/SKILL.md"));
    assert!(prepare.contains("At most 10 memories and about 2,000 characters"));
    assert!(prepare.contains("`limit` `10`"));
}

/// The plugin orders nothing by use and shows no retrieval count: counts are not in any result,
/// and no skill or agent asks for an order by them (D202 (4), D204 (4), slice 6q, Q3, as amended).
/// Marks are the relevance signal now, so the plugin talks about marks, not about use.
#[test]
fn no_plugin_file_orders_by_use_or_shows_retrieval_counts() {
    let mut files = Vec::new();
    files_under(&plugin_dir(), &mut files);
    for path in files {
        let text = read_text(&path).to_lowercase();
        for banned in [
            "most_retrieved",
            "most retrieved",
            "most-used",
            "most used",
            "used often",
            "retrieval count",
            "retrieval_count",
            "last_retrieved",
            "times retrieved",
        ] {
            assert!(!text.contains(banned), "{} says {banned}", path.display());
        }
    }
}

/// `prepare` names `memory_mark` and marks after the work, only the memories it used (D202).
#[test]
fn prepare_marks_the_memories_it_used_after_the_work() {
    let prepare = read_text(&plugin_dir().join("skills/prepare/SKILL.md"));
    assert!(
        prepare.contains("`memory_mark`"),
        "prepare does not name memory_mark"
    );
    assert!(prepare.contains("After the work is done"));
    assert!(prepare.contains("`useful` for a memory that helped the work"));
    assert!(prepare.contains("`wrong` for a memory that misled it"));
    assert!(prepare.contains("at most 200 characters"));
    assert!(prepare.contains("Mark only the memories you actually used"));
    assert!(
        prepare.contains("Building the brief records nothing"),
        "prepare must say that the brief itself writes nothing"
    );
}

/// A memory with a `wrong` mark is unverified in the brief, whoever marked it (D202 (3)).
#[test]
fn prepare_treats_a_wrong_mark_as_unverified() {
    let prepare = read_text(&plugin_dir().join("skills/prepare/SKILL.md"));
    assert!(prepare.contains("A memory with a `wrong` mark from any actor is unverified"));
    assert!(prepare.contains("Marked wrong by <actor>; check before relying on it"));
    assert!(prepare.contains("do not present it as established"));
    assert!(prepare.contains("At most 10 memories and about 2,000 characters"));
}

/// The `gitmaster` skill says why to mark, and that a mark can be revised (D202 (2)).
#[test]
fn the_gitmaster_skill_teaches_honest_marking_and_names_memory_mark() {
    let skill = read_text(&plugin_dir().join("skills/gitmaster/SKILL.md"));
    assert!(
        skill.contains("`memory_mark`"),
        "the skill does not name memory_mark"
    );
    assert!(skill.contains("A mark can change"));
    assert!(skill.contains("mark it `wrong` with the reason"));
}

/// The consolidator starts with the `wrong` marks, then the changed files, the unconfirmed
/// memories and the oldest (D202 (3)). The order is read from the list under "Where to start".
#[test]
fn the_consolidator_starts_with_wrong_marks_then_changed_files_then_unconfirmed_then_oldest() {
    let consolidator = read_text(&plugin_dir().join("agents/memory-consolidator.md"));
    let list = list_after(&consolidator, "Where to start, in this order:\n\n");
    let first = list.lines().next().expect("the first item");
    assert!(
        first.starts_with("- Memories with a `wrong` mark first"),
        "the first item is: {first}"
    );

    let order = [
        "- Memories with a `wrong` mark first",
        "- Then the memories whose files changed",
        "- Then the `unconfirmed` memories",
        "- Then the oldest accepted memories",
    ];
    let positions: Vec<usize> = order.iter().map(|item| position(list, item)).collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "the priority is out of order: {positions:?}"
    );
}

/// A disagreement between actors is a reason to check, never a count to settle (D202 (3)).
#[test]
fn the_consolidator_treats_a_disagreement_as_a_reason_to_check() {
    let consolidator = read_text(&plugin_dir().join("agents/memory-consolidator.md"));
    assert!(consolidator.contains("A disagreement is a reason to check"));
    assert!(
        consolidator.contains("one actor marked a memory `useful` and another marked it `wrong`")
    );
    assert!(consolidator.contains("Never settle a disagreement by counting marks"));
}

/// The consolidator proposes; it does not judge usefulness, so it holds no `memory_mark` (D202 (3)).
#[test]
fn the_consolidator_holds_no_memory_mark_tool() {
    let text = read_text(&plugin_dir().join("agents/memory-consolidator.md"));
    let front = front_matter(&text);
    let tools: Vec<&str> = field(&front, "tools").split(", ").collect();
    assert!(
        !tools.iter().any(|tool| tool.ends_with("__memory_mark")),
        "the consolidator must not hold memory_mark: {tools:?}"
    );
}

/// The `consolidate` skill passes the same order on to the sub-agent.
#[test]
fn the_consolidate_skill_passes_the_wrong_marks_first() {
    let consolidate = read_text(&plugin_dir().join("skills/consolidate/SKILL.md"));
    let order = [
        "with a `wrong` mark",
        "whose files changed",
        "the unconfirmed ones",
        "then the oldest",
    ];
    let positions: Vec<usize> = order
        .iter()
        .map(|needle| position(&consolidate, needle))
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "the consolidate priority is out of order: {positions:?}"
    );
}

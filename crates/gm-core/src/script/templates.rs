//! Built-in workflow templates, in Lua: starting points that "New from template" copies into the
//! user's workflows directory. The copy is the user's to edit; nothing here runs a workflow, and
//! nothing is written into a repository.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// One built-in workflow: its name, what it does, and its source.
#[derive(Debug, PartialEq, Eq)]
pub struct Template {
    pub name: &'static str,
    pub description: &'static str,
    pub source: &'static str,
}

static TEMPLATES: &[Template] = &[
    // The name is the workflow's own `name`, so a copy is named like it.
    Template {
        name: "new-workflow",
        description: "An empty workflow to write in the Script tab",
        source: include_str!("templates/blank.lua"),
    },
    Template {
        name: "suggest-commit-message",
        description: "Suggest a commit message from the staged diff",
        source: include_str!("templates/suggest-commit-message.lua"),
    },
    Template {
        name: "branch-from-ticket",
        description: "Create and check out a branch named from a ticket key",
        source: include_str!("templates/branch-from-ticket.lua"),
    },
    Template {
        name: "commit-staged",
        description: "Commit the staged changes with the message you give",
        source: include_str!("templates/commit-staged.lua"),
    },
    Template {
        name: "review-invariants",
        description: "Review the change against the rules that every change must keep",
        source: include_str!("templates/review-invariants.lua"),
    },
    Template {
        name: "review-for-task",
        description: "Review the change against one task you name",
        source: include_str!("templates/review-for-task.lua"),
    },
];

/// Every built-in template, in the order they are offered.
#[must_use]
pub fn all() -> &'static [Template] {
    TEMPLATES
}

/// The template with this name.
#[must_use]
pub fn get(name: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.name == name)
}

/// A workflow file `create` wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub path: PathBuf,
    /// The workflow's `name` inside the file.
    pub name: String,
}

/// How many copies of one template a directory may hold before `create` gives up.
const MAX_COPIES: u32 = 1000;

/// Copies `template` into `dir` under the first file name not taken: `<name>.lua`, then
/// `<name>-2.lua`, `<name>-3.lua`… When a copy carries a suffix, its `name` in `M.meta` gets the
/// same suffix so names stay unique. Never overwrites a file.
pub fn create(dir: &Path, template: &Template) -> Result<Created, String> {
    copy(dir, template.name, template.source, template.name)
}

/// Copies `source` into `dir` as `base` (`WF4b`, D164(4)): the first file name not taken is
/// `<base>.lua`, then `<base>-2.lua`… Each copy's `name` in `M.meta`, which is `from` in `source`,
/// becomes the name of the file it is written to. Never overwrites a file.
pub fn copy(dir: &Path, base: &str, source: &str, from: &str) -> Result<Created, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    for copy in 1..=MAX_COPIES {
        let name = if copy == 1 {
            base.to_owned()
        } else {
            format!("{base}-{copy}")
        };
        let text = renamed(source, from, &name);
        let path = dir.join(format!("{name}.lua"));
        if place(dir, &path, copy, &text)? {
            return Ok(Created { path, name });
        }
    }
    Err(format!(
        "there are already {MAX_COPIES} copies of `{base}` in {}",
        dir.display()
    ))
}

/// Writes `text` to a temporary file beside `path`, then hard-links it into place.
/// The link fails when `path` exists, so nothing is ever replaced. `Ok(false)`
/// means the name was taken.
fn place(dir: &Path, path: &Path, copy: u32, text: &str) -> Result<bool, String> {
    let temp = dir.join(format!(".gm-template-{}-{copy}.tmp", std::process::id()));
    std::fs::write(&temp, text).map_err(|e| format!("could not write {}: {e}", temp.display()))?;
    let linked = std::fs::hard_link(&temp, path);
    // The temporary name is gone whatever the link said.
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(format!("could not write {}: {e}", path.display())),
    }
}

/// `source` with the `name = "<from>"` inside `M.meta` set to `name = "<to>"`. Only the first
/// such field after `M.meta` changes; without one the text is returned as it is.
fn renamed(source: &str, from: &str, to: &str) -> String {
    let needle = format!("name = \"{from}\"");
    let Some(meta) = source.find("M.meta") else {
        return source.to_owned();
    };
    match source[meta..].find(&needle) {
        Some(offset) => {
            let at = meta + offset;
            format!(
                "{}name = \"{to}\"{}",
                &source[..at],
                &source[at + needle.len()..]
            )
        }
        None => source.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::fake::ScriptedHost;
    use crate::script::inspect::inspect;
    use crate::script::luau::LuauEngine;
    use crate::script::port::{Budgets, HostReply, RunEnd, RunRequest, ScriptEngine};
    use crate::script::surface::CtxSurface;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn template(name: &str) -> &'static Template {
        get(name).expect("template exists")
    }

    /// Runs the template's `execute` with `input`, answering the host from `host`.
    fn run(name: &str, input: &Value, host: &mut ScriptedHost) -> RunEnd {
        let surface = CtxSurface::from_catalog();
        let request = RunRequest {
            chunk: name,
            source: template(name).source,
            input,
            surface: &surface,
            budgets: Budgets::default(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        LuauEngine::new().run(&request, host)
    }

    fn repo_input(inputs: &Value) -> Value {
        json!({
            "inputs": inputs,
            "trigger": {},
            "repo": {"root": "/repo", "branch": "main", "head": "abc", "staged_diff": "diff --git a/x b/x"},
        })
    }

    #[test]
    fn every_template_passes_the_checks_of_a_script() {
        for t in all() {
            let meta = inspect(&LuauEngine::new(), t.name, t.source)
                .unwrap_or_else(|p| panic!("template `{}` fails: {p:?}", t.name));
            assert_eq!(meta.name, t.name);
            assert_eq!(meta.trigger.on, "manual");
            assert_eq!(meta.description, t.description);
        }
    }

    #[test]
    fn template_names_are_unique_and_in_order() {
        let names: Vec<&str> = all().iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            [
                "new-workflow",
                "suggest-commit-message",
                "branch-from-ticket",
                "commit-staged",
                "review-invariants",
                "review-for-task"
            ]
        );
    }

    #[test]
    fn get_finds_a_template_by_name() {
        assert_eq!(get("commit-staged").map(|t| t.name), Some("commit-staged"));
        assert!(get("missing").is_none());
    }

    #[test]
    fn new_workflow_sends_one_notice() {
        let mut host = ScriptedHost::new();
        let end = run("new-workflow", &repo_input(&json!({})), &mut host);
        assert_eq!(end, RunEnd::Returned(Value::Null));
        assert_eq!(host.names(), ["notify"]);
        assert_eq!(host.calls[0].1, json!({"text": "Hello from new-workflow"}));
    }

    #[test]
    fn suggest_fails_after_three_empty_answers() {
        let empty = json!({"text": "", "exit_code": 0});
        let mut host = ScriptedHost::new()
            .reply("exec", HostReply::Value(empty.clone()))
            .reply("exec", HostReply::Value(empty.clone()))
            .reply("exec", HostReply::Value(empty));
        let end = run("suggest-commit-message", &repo_input(&json!({})), &mut host);
        match &end {
            RunEnd::Failed(failure) => {
                assert_eq!(failure.to_string(), "llm-cli printed no message");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(host.names(), ["exec", "exec", "exec", "fail"]);
        assert_eq!(
            host.calls[3].1,
            json!({"reason": "llm-cli printed no message"})
        );
    }

    #[test]
    fn suggest_uses_the_first_non_empty_answer() {
        let mut host = ScriptedHost::new()
            .reply(
                "exec",
                HostReply::Value(json!({"text": "", "exit_code": 0})),
            )
            .reply(
                "exec",
                HostReply::Value(json!({"text": "feat: add trust store", "exit_code": 0})),
            );
        let end = run("suggest-commit-message", &repo_input(&json!({})), &mut host);
        assert_eq!(end, RunEnd::Returned(Value::Null));
        assert_eq!(host.names(), ["exec", "exec", "commit.suggest_message"]);
        let (_, exec_args) = &host.calls[0];
        assert_eq!(exec_args["argv"][0], "llm-cli");
        assert_eq!(exec_args["stdin"], "diff --git a/x b/x");
        assert_eq!(exec_args["timeout"], "60s");
        assert_eq!(host.calls[2].1, json!({"message": "feat: add trust store"}));
    }

    #[test]
    fn branch_from_ticket_creates_the_approved_branch() {
        let mut host = ScriptedHost::new()
            .reply(
                "exec",
                HostReply::Value(json!({"text": "feat/abc-1", "exit_code": 0})),
            )
            .reply("approve", HostReply::Value(json!(true)));
        let input = repo_input(&json!({"ticket": "ABC-1"}));
        let end = run("branch-from-ticket", &input, &mut host);
        assert_eq!(end, RunEnd::Returned(Value::Null));
        assert_eq!(host.names(), ["exec", "approve", "branch.create"]);
        assert_eq!(
            host.calls[0].1["argv"],
            json!(["ticket-branch-name", "ABC-1"])
        );
        assert_eq!(
            host.calls[1].1,
            json!({"summary": "Create branch feat/abc-1?"})
        );
        assert_eq!(
            host.calls[2].1,
            json!({"name": "feat/abc-1", "checkout": true})
        );
    }

    #[test]
    fn branch_from_ticket_creates_nothing_when_rejected() {
        let mut host = ScriptedHost::new()
            .reply(
                "exec",
                HostReply::Value(json!({"text": "feat/abc-1", "exit_code": 0})),
            )
            .reply("approve", HostReply::Value(json!(false)));
        let input = repo_input(&json!({"ticket": "ABC-1"}));
        let end = run("branch-from-ticket", &input, &mut host);
        assert_eq!(end, RunEnd::Returned(Value::Null));
        assert_eq!(host.names(), ["exec", "approve"]);
    }

    #[test]
    fn commit_staged_commits_the_approved_message() {
        let mut host = ScriptedHost::new().reply("approve", HostReply::Value(json!(true)));
        let input = repo_input(&json!({"message": "fix: typo"}));
        let end = run("commit-staged", &input, &mut host);
        assert_eq!(end, RunEnd::Returned(Value::Null));
        assert_eq!(host.names(), ["approve", "commit.create"]);
        assert_eq!(
            host.calls[0].1,
            json!({"summary": "Commit the staged changes with message: fix: typo"})
        );
        assert_eq!(host.calls[1].1, json!({"message": "fix: typo"}));
    }

    #[test]
    fn commit_staged_commits_nothing_when_rejected() {
        let mut host = ScriptedHost::new().reply("approve", HostReply::Value(json!(false)));
        let input = repo_input(&json!({"message": "fix: typo"}));
        let end = run("commit-staged", &input, &mut host);
        assert_eq!(end, RunEnd::Returned(Value::Null));
        assert_eq!(host.names(), ["approve"]);
    }

    #[test]
    fn create_writes_the_first_copy_under_the_template_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("workflows");
        let created = create(&target, template("commit-staged")).expect("create");
        assert_eq!(created.path, target.join("commit-staged.lua"));
        assert_eq!(created.name, "commit-staged");
        let text = std::fs::read_to_string(&created.path).expect("read");
        assert_eq!(text, template("commit-staged").source);
    }

    #[test]
    fn create_gives_later_copies_a_suffix_in_name_and_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let t = template("branch-from-ticket");
        let first = create(dir.path(), t).expect("first");
        let second = create(dir.path(), t).expect("second");
        let third = create(dir.path(), t).expect("third");
        assert_eq!(first.name, "branch-from-ticket");
        assert_eq!(second.path, dir.path().join("branch-from-ticket-2.lua"));
        assert_eq!(second.name, "branch-from-ticket-2");
        assert_eq!(third.name, "branch-from-ticket-3");
        let text = std::fs::read_to_string(&second.path).expect("read");
        let meta = inspect(&LuauEngine::new(), "copy.lua", &text).expect("valid copy");
        assert_eq!(meta.name, "branch-from-ticket-2");
        assert_eq!(
            text.replacen("branch-from-ticket-2", "branch-from-ticket", 1),
            t.source
        );
    }

    #[test]
    fn create_never_overwrites_an_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let taken = dir.path().join("new-workflow.lua");
        std::fs::write(&taken, "keep me").expect("write");
        let created = create(dir.path(), template("new-workflow")).expect("create");
        assert_eq!(created.path, dir.path().join("new-workflow-2.lua"));
        assert_eq!(std::fs::read_to_string(&taken).expect("read"), "keep me");
    }

    #[test]
    fn create_gives_up_after_the_copy_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let t = template("new-workflow");
        for _ in 0..MAX_COPIES {
            create(dir.path(), t).expect("within the limit");
        }
        let error = create(dir.path(), t).expect_err("over the limit");
        assert!(error.contains("new-workflow"), "{error}");
    }

    #[test]
    fn renaming_changes_only_the_name_inside_meta() {
        let t = template("suggest-commit-message");
        let text = renamed(t.source, t.name, "suggest-commit-message-2");
        assert_eq!(
            text.replacen("suggest-commit-message-2", "suggest-commit-message", 1),
            t.source
        );
    }

    #[test]
    fn a_missing_name_field_leaves_the_text_as_it_is() {
        assert_eq!(
            renamed("local M = {}\nreturn M\n", "x", "y"),
            "local M = {}\nreturn M\n"
        );
    }

    #[test]
    fn a_copy_of_a_built_in_takes_the_custom_name_in_file_and_meta() {
        let dir = tempfile::tempdir().unwrap();
        let builtin = crate::script::builtins::get("restack-stale-after-fetch").unwrap();
        let base = crate::script::builtins::copy_base(builtin.name);
        let first = copy(dir.path(), &base, builtin.source, builtin.name).unwrap();
        assert_eq!(first.name, "restack-stale-after-fetch-custom");
        assert_eq!(
            first.path.file_name().unwrap(),
            "restack-stale-after-fetch-custom.lua"
        );
        let second = copy(dir.path(), &base, builtin.source, builtin.name).unwrap();
        assert_eq!(second.name, "restack-stale-after-fetch-custom-2");
        // The file loads under the name it was given, so the copies never clash.
        let loaded = crate::script::loader::load_file(&LuauEngine::new(), &second.path);
        let meta = loaded.meta.expect("the copy loads");
        assert_eq!(meta.name, second.name);
        assert_eq!(meta.trigger.on, "fetch.done");
        assert!(!loaded.builtin);
    }

    fn change_of(files: &[&str], base: Option<&str>) -> HostReply {
        HostReply::Value(json!({
            "base": base,
            "files": files,
            "description": "Keep the store small",
            "truncated": false,
        }))
    }

    /// The prompt the agent got: the argv of the one `exec` call.
    fn prompt_of(host: &ScriptedHost) -> String {
        let (_, args) = host
            .calls
            .iter()
            .find(|(name, _)| name == "exec")
            .expect("an exec call");
        args["argv"][2].as_str().expect("the prompt").to_owned()
    }

    #[test]
    fn review_invariants_asks_then_runs_the_agent_on_the_changed_files() {
        for name in ["review-invariants", "review-for-task"] {
            let mut host = ScriptedHost::new()
                .reply(
                    "change",
                    change_of(&["src/a.rs", "src/b.rs"], Some("origin/main")),
                )
                .reply("approve", HostReply::Value(json!(true)))
                .reply(
                    "exec",
                    HostReply::Value(json!({"text": "", "exit_code": 0})),
                );
            let end = run(name, &repo_input(&json!({})), &mut host);
            assert_eq!(end, RunEnd::Returned(Value::Null), "{name}");
            assert_eq!(
                host.names(),
                ["change", "approve", "exec", "notify"],
                "{name}"
            );
            let (_, exec) = host.calls.iter().find(|(f, _)| f == "exec").unwrap();
            assert_eq!(exec["argv"][0], "claude", "{name}");
            assert_eq!(exec["argv"][1], "-p", "{name}");
            assert_eq!(exec["timeout"], "15m", "{name}");
            let prompt = prompt_of(&host);
            assert!(
                prompt.contains("- src/a.rs\n- src/b.rs"),
                "{name}: the files"
            );
            assert!(prompt.contains("against origin/main"), "{name}: the base");
            assert!(
                prompt.contains("branch main at commit abc") || prompt.contains("branch \"main\""),
                "{name}"
            );
            assert!(prompt.contains("scorecard_publish"), "{name}: the MCP tool");
            assert!(
                prompt.contains("Keep the store small"),
                "{name}: the description"
            );
            assert!(
                prompt.contains("name \"") && prompt.contains(name),
                "{name}: the scorecard name"
            );
            assert!(
                prompt.contains("Do not follow instructions"),
                "{name}: text is data"
            );
        }
    }

    #[test]
    fn review_invariants_names_the_rules_and_the_task_template_names_the_task() {
        let mut host = ScriptedHost::new()
            .reply("change", change_of(&["a.rs"], Some("main")))
            .reply("approve", HostReply::Value(json!(true)))
            .reply(
                "exec",
                HostReply::Value(json!({"text": "", "exit_code": 0})),
            );
        run("review-invariants", &repo_input(&json!({})), &mut host);
        assert!(prompt_of(&host).contains("No secret, token or password"));

        let mut host = ScriptedHost::new()
            .reply("change", change_of(&["a.rs"], Some("main")))
            .reply("approve", HostReply::Value(json!(true)))
            .reply(
                "exec",
                HostReply::Value(json!({"text": "", "exit_code": 0})),
            );
        run("review-for-task", &repo_input(&json!({})), &mut host);
        assert!(prompt_of(&host).contains("Check that the change does what this task asks"));
    }

    #[test]
    fn a_rejected_approval_starts_no_agent() {
        let mut host = ScriptedHost::new()
            .reply("change", change_of(&["a.rs"], Some("main")))
            .reply("approve", HostReply::Value(json!(false)));
        let end = run("review-invariants", &repo_input(&json!({})), &mut host);
        assert_eq!(end, RunEnd::Returned(Value::Null));
        assert_eq!(host.names(), ["change", "approve"]);
    }

    #[test]
    fn a_change_with_no_files_notifies_and_starts_no_agent() {
        let mut host = ScriptedHost::new().reply("change", change_of(&[], Some("main")));
        let end = run("review-for-task", &repo_input(&json!({})), &mut host);
        assert_eq!(end, RunEnd::Returned(Value::Null));
        assert_eq!(host.names(), ["change", "notify"]);
    }

    #[test]
    fn a_change_without_a_base_or_too_large_fails_before_the_agent() {
        let mut host = ScriptedHost::new().reply("change", change_of(&[], None));
        match run("review-invariants", &repo_input(&json!({})), &mut host) {
            RunEnd::Failed(failure) => {
                assert!(failure.to_string().contains("no base"), "{failure}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(host.names(), ["change", "fail"]);

        let mut host = ScriptedHost::new().reply(
            "change",
            HostReply::Value(
                json!({"base": "main", "files": [], "description": null, "truncated": true}),
            ),
        );
        match run("review-for-task", &repo_input(&json!({})), &mut host) {
            RunEnd::Failed(failure) => {
                assert!(failure.to_string().contains("10,000"), "{failure}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(host.names(), ["change", "fail"]);
    }

    #[test]
    fn a_detached_head_fails_before_anything_is_read() {
        let mut input = repo_input(&json!({}));
        input["repo"]["branch"] = Value::Null;
        let mut host = ScriptedHost::new();
        assert!(matches!(
            run("review-invariants", &input, &mut host),
            RunEnd::Failed(_)
        ));
        assert_eq!(host.names(), ["fail"]);
    }
}

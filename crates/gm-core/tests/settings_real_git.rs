//! The preferences in Git's configuration (docs/reference/configuration.md) against real
//! repositories and a throwaway global configuration file.

use std::path::{Path, PathBuf};
use std::process::Command;

use gm_core::settings::{DiffLayout, Migration, Settings, Source, migrate_legacy};
use gm_core::threads::config::UserPattern;

struct Fixture {
    _base: tempfile::TempDir,
    repo: PathBuf,
    global: PathBuf,
    source: Source,
}

fn fixture() -> Fixture {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    let global = root.join("gitconfig");
    let source = Source {
        global: Some(global.clone()),
    };
    let f = Fixture {
        _base: base,
        repo,
        global,
        source,
    };
    git(&f, &f.repo, &["init", "-q", "-b", "main"]);
    f
}

fn git(f: &Fixture, dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", &f.global)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_repository_overrides_the_global_file_the_way_git_does() {
    let f = fixture();
    std::fs::write(
        &f.global,
        "[gitmaster]\n\tdiffLayout = split\n\tauthorPictures = true\n\
         [gitmaster \"threads\"]\n\tcapture = false\n\tmaxResultBytes = 2048\n",
    )
    .unwrap();
    let global = Settings::load(&f.repo, &f.source);
    assert_eq!(global.diff_layout, DiffLayout::Split);
    assert!(global.author_pictures);
    assert!(!global.threads.capture);
    assert_eq!(global.threads.max_result_bytes, 2048);

    git(&f, &f.repo, &["config", "gitmaster.diffLayout", "unified"]);
    git(
        &f,
        &f.repo,
        &["config", "gitmaster.threads.capture", "true"],
    );
    let local = Settings::load(&f.repo, &f.source);
    assert_eq!(local.diff_layout, DiffLayout::Unified);
    assert!(local.threads.capture);
    // What the repository does not set still comes from the global file.
    assert!(local.author_pictures);
    assert_eq!(local.threads.max_result_bytes, 2048);

    // From outside any repository only the global file applies.
    let outside = f.global.parent().unwrap();
    assert_eq!(
        Settings::load(outside, &f.source).diff_layout,
        DiffLayout::Split
    );
}

#[test]
fn lists_and_patterns_add_up_and_a_repository_can_clear_them() {
    let f = fixture();
    std::fs::write(
        &f.global,
        "[gitmaster \"threads\"]\n\tredactPath = secrets/**\n\tredactPath = *.key\n\
         \tredactDisabledBuiltin = jwt\n\
         [gitmaster-redact \"acme\"]\n\tregex = acme_[a-z]+\n\tgroup = 1\n",
    )
    .unwrap();
    git(
        &f,
        &f.repo,
        &["config", "--add", "gitmaster.threads.redactPath", "extra"],
    );
    let s = Settings::load(&f.repo, &f.source);
    assert_eq!(s.threads.redact.paths, ["secrets/**", "*.key", "extra"]);
    assert_eq!(s.threads.redact.disabled_builtins, ["jwt"]);
    assert_eq!(s.threads.redact.patterns.len(), 1);

    git(&f, &f.repo, &["config", "gitmaster-redact.acme.regex", ""]);
    git(
        &f,
        &f.repo,
        &["config", "--add", "gitmaster.threads.redactPath", ""],
    );
    git(
        &f,
        &f.repo,
        &["config", "--add", "gitmaster.threads.redactPath", "only"],
    );
    let s = Settings::load(&f.repo, &f.source);
    assert_eq!(s.threads.redact.paths, ["only"]);
    assert!(s.threads.redact.patterns.is_empty());
}

#[test]
fn an_include_in_the_global_file_is_followed() {
    let f = fixture();
    let shared = f.global.with_file_name("shared");
    std::fs::write(&shared, "[gitmaster]\n\tdiffLayout = split\n").unwrap();
    std::fs::write(
        &f.global,
        format!("[include]\n\tpath = {}\n", shared.display()),
    )
    .unwrap();
    assert_eq!(
        Settings::load(&f.repo, &f.source).diff_layout,
        DiffLayout::Split
    );
}

#[test]
fn save_writes_only_what_differs_from_the_defaults_and_round_trips() {
    let f = fixture();
    let mut settings = Settings::default();
    settings.save_global(&f.source).unwrap();
    assert!(
        !f.global.exists()
            || std::fs::read_to_string(&f.global)
                .unwrap()
                .trim()
                .is_empty()
    );

    settings.diff_layout = DiffLayout::Split;
    settings.threads.max_result_bytes = 4096;
    settings.threads.redact.disabled_builtins = vec!["jwt".into()];
    settings.threads.redact.paths = vec![".env".into(), "secrets/**".into()];
    settings.threads.redact.patterns = vec![
        UserPattern {
            name: "acme".into(),
            regex: "acme_[a-z]+".into(),
            group: 0,
        },
        UserPattern {
            name: "db-password".into(),
            regex: "(DB_PASS=)(\\S+)".into(),
            group: 2,
        },
    ];
    settings.save_global(&f.source).unwrap();
    assert_eq!(Settings::load_global(&f.source), settings);

    let text = std::fs::read_to_string(&f.global).unwrap();
    assert!(text.contains("diffLayout = split"), "{text}");
    assert!(!text.contains("authorPictures"), "{text}");
    assert!(
        text.contains("[gitmaster-redact \"db-password\"]"),
        "{text}"
    );

    // Back to the defaults: the keys go away again.
    Settings::default().save_global(&f.source).unwrap();
    assert_eq!(Settings::load_global(&f.source), Settings::default());
    let text = std::fs::read_to_string(&f.global).unwrap();
    assert!(!text.contains("gitmaster"), "{text}");
}

#[test]
fn an_empty_path_list_is_kept_as_none_and_a_repository_override_is_not_touched() {
    let f = fixture();
    let mut settings = Settings::default();
    settings.threads.redact.paths = Vec::new();
    settings.save_global(&f.source).unwrap();
    assert!(
        Settings::load_global(&f.source)
            .threads
            .redact
            .paths
            .is_empty()
    );

    git(&f, &f.repo, &["config", "gitmaster.diffLayout", "split"]);
    Settings::default().save_global(&f.source).unwrap();
    assert_eq!(
        Settings::load(&f.repo, &f.source).diff_layout,
        DiffLayout::Split
    );
}

#[test]
fn migrating_moves_the_json_values_and_sets_the_file_aside() {
    let f = fixture();
    let dir = f.global.parent().unwrap().join("cfg");
    std::fs::create_dir(&dir).unwrap();
    let json = dir.join("settings.json");
    std::fs::write(
        &json,
        r#"{"diff_layout":"split","author_pictures":true,
            "threads":{"max_result_bytes":999,"redact":{"patterns":[{"name":"a","regex":"b","group":1}]}}}"#,
    )
    .unwrap();
    // A value already in ~/.gitconfig is not overridden by the old file.
    std::fs::write(&f.global, "[gitmaster]\n\tauthorPictures = false\n").unwrap();

    let moved = migrate_legacy(&json, &f.source).unwrap();
    assert_eq!(
        moved,
        Migration::Moved {
            file: dir.join("settings.json.migrated")
        }
    );
    assert!(!json.exists());
    assert!(dir.join("settings.json.migrated").exists());
    let s = Settings::load_global(&f.source);
    assert_eq!(s.diff_layout, DiffLayout::Split);
    assert!(!s.author_pictures);
    assert_eq!(s.threads.max_result_bytes, 999);
    assert_eq!(s.threads.redact.patterns[0].group, 1);

    assert_eq!(
        migrate_legacy(&json, &f.source).unwrap(),
        Migration::NothingToDo
    );
}

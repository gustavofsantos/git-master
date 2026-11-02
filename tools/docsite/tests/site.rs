//! Runs the built binary on the real `docs/` of the repository, so a broken page or link fails
//! `cargo test` as well as `docsite check`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn docs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs")
}

fn docsite(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_docsite"))
        .args(args)
        .output()
        .expect("docsite runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn the_real_docs_pass_check() {
    let docs = docs();
    let output = docsite(&["check", "--docs", docs.to_str().unwrap()]);
    assert!(output.status.success(), "{}", text(&output.stderr));
}

#[test]
fn status_lists_every_page() {
    let docs = docs();
    let output = docsite(&["status", "--docs", docs.to_str().unwrap()]);
    let report = text(&output.stdout);
    assert!(output.status.success(), "{report}");
    assert!(report.contains("docs/index.md"), "{report}");
    assert!(report.contains("Planned markers left"), "{report}");
}

#[test]
fn build_writes_a_site_that_can_be_deployed_anywhere() {
    let docs = docs();
    let out = tempfile::tempdir().unwrap();
    let site = out.path().join("site");
    let args = [
        "build",
        "--docs",
        docs.to_str().unwrap(),
        "--out",
        site.to_str().unwrap(),
        "--base-url",
        "https://docs.example.com",
    ];
    let output = docsite(&args);
    assert!(output.status.success(), "{}", text(&output.stderr));

    for file in [
        "index.html",
        "404.html",
        "availability.html",
        "assets/style.css",
        "assets/site.js",
        "assets/search-index.js",
        "sitemap.xml",
        "robots.txt",
        ".nojekyll",
    ] {
        assert!(site.join(file).is_file(), "missing {file}");
    }
    let page = std::fs::read_to_string(site.join("guide/branches.html")).unwrap();
    assert!(
        page.contains("href=\"../assets/style.css\""),
        "links must be relative"
    );
    assert!(
        page.contains("href=\"../guide/worktrees.html\"")
            || page.contains("href=\"worktrees.html\""),
        "page links become .html"
    );
    assert!(
        !page.contains(".md\""),
        "no link may still point at a Markdown file"
    );
    assert!(page.contains(
        "<link rel=\"canonical\" href=\"https://docs.example.com/guide/branches.html\">"
    ));
    let sitemap = std::fs::read_to_string(site.join("sitemap.xml")).unwrap();
    assert!(sitemap.contains("https://docs.example.com/availability.html"));

    // A second build replaces the first, because docsite wrote the directory.
    assert!(docsite(&args).status.success());
}

#[test]
fn build_refuses_a_directory_it_did_not_write() {
    let docs = docs();
    let out = tempfile::tempdir().unwrap();
    std::fs::write(out.path().join("precious.txt"), "keep me").unwrap();
    let output = docsite(&[
        "build",
        "--docs",
        docs.to_str().unwrap(),
        "--out",
        out.path().to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("was not written by docsite"),
        "{}",
        text(&output.stderr)
    );
    assert!(out.path().join("precious.txt").is_file());
}

#[test]
fn a_preview_page_shows_the_badge_and_how_to_turn_it_on() {
    let docs = docs();
    let out = tempfile::tempdir().unwrap();
    let site = out.path().join("site");
    let args = [
        "build",
        "--docs",
        docs.to_str().unwrap(),
        "--out",
        site.to_str().unwrap(),
    ];
    let output = docsite(&args);
    assert!(output.status.success(), "{}", text(&output.stderr));

    let page = std::fs::read_to_string(site.join("guide/stacks.html")).unwrap();
    assert!(page.contains("badge preview"), "Preview badge missing");
    assert!(
        page.contains("GITMASTER_ENABLE_STACKS=1"),
        "turn-on note missing"
    );
    let plain = std::fs::read_to_string(site.join("guide/history.html")).unwrap();
    assert!(
        !plain.contains("badge preview"),
        "only preview pages get the badge"
    );

    let availability = std::fs::read_to_string(site.join("availability.html")).unwrap();
    assert!(availability.contains("Preview features"));
    assert!(availability.contains("GITMASTER_ENABLE_CONFLICT_RESOLVER=1"));

    let status = docsite(&["status", "--docs", docs.to_str().unwrap()]);
    assert!(text(&status.stdout).contains("preview: stacks"));
}

#[test]
fn check_rejects_an_unknown_preview_feature() {
    let out = tempfile::tempdir().unwrap();
    let root = out.path();
    std::fs::write(
        root.join("sections.toml"),
        "[[section]]\ndir = \"guide\"\ntitle = \"Guide\"\norder = 10\n",
    )
    .unwrap();
    std::fs::create_dir(root.join("guide")).unwrap();
    std::fs::write(
        root.join("guide/a.md"),
        "---\ntitle: A\ndescription: A page.\nstatus: implemented\norder: 10\npreview: bogus\n---\n\nBody.\n",
    )
    .unwrap();
    let output = docsite(&["check", "--docs", root.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("`preview` is `bogus`"),
        "{}",
        text(&output.stderr)
    );
}

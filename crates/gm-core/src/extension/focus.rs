//! What a slot function receives as its focus (docs/reference/extension-api.md). The `commit`
//! slot gets the commit and a little of the repository around it.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const SEP: char = '\u{1f}';

fn git(root: &Path, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes())
            .map_err(|e| format!("cannot write to git: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The `commit` focus of `rev` in the repository at `root`.
pub fn commit(root: &Path, rev: &str) -> Result<Value, String> {
    if rev.starts_with('-') {
        return Err(format!("{rev} is not a revision"));
    }
    let format = [
        "%H", "%h", "%s", "%an", "%ae", "%at", "%cn", "%ce", "%ct", "%P", "%B",
    ]
    .join(&SEP.to_string());
    let text = git(
        root,
        &[
            "show",
            "-s",
            "--no-color",
            &format!("--format={format}"),
            rev,
            "--",
        ],
        None,
    )?;
    let fields: Vec<&str> = text.splitn(11, SEP).collect();
    let [
        hash,
        short,
        subject,
        an,
        ae,
        at,
        cn,
        ce,
        ct,
        parents,
        message,
    ] = fields[..]
    else {
        return Err(format!("git did not describe {rev}"));
    };
    let body = message
        .trim_end()
        .split_once('\n')
        .map_or("", |(_, rest)| rest.trim_start_matches('\n'));
    let trailers = git(root, &["interpret-trailers", "--parse"], Some(message))
        .map_or_else(|_| Vec::new(), |out| parse_trailers(&out));
    let time = |t: &str| t.trim().parse::<i64>().unwrap_or_default();
    Ok(json!({
        "hash": hash,
        "short_hash": short,
        "subject": subject,
        "body": body,
        "trailers": trailers,
        "author": { "name": an, "email": ae, "time": time(at) },
        "committer": { "name": cn, "email": ce, "time": time(ct) },
        "parents": parents.split_whitespace().collect::<Vec<_>>(),
        "repo": repo(root),
    }))
}

/// `Key: value` lines as `git interpret-trailers --parse` prints them.
fn parse_trailers(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| json!({ "key": key.trim(), "value": value.trim() }))
        .collect()
}

fn repo(root: &Path) -> Value {
    let head = git(root, &["rev-parse", "--verify", "-q", "HEAD"], None)
        .map(|s| s.trim().to_owned())
        .ok();
    let branch = git(root, &["symbolic-ref", "--short", "-q", "HEAD"], None)
        .map(|s| s.trim().to_owned())
        .ok()
        .filter(|s| !s.is_empty());
    let remotes: Vec<Value> = git(
        root,
        &["config", "--get-regexp", r"^remote\..*\.url$"],
        None,
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|line| {
        let (key, url) = line.split_once(' ')?;
        let name = key.strip_prefix("remote.")?.strip_suffix(".url")?;
        Some(json!({ "name": name, "url": url }))
    })
    .collect();
    json!({
        "root": root.to_string_lossy(),
        "head": head,
        "branch": branch,
        "remotes": remotes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailers_are_key_and_value() {
        assert_eq!(
            parse_trailers("Refs: PAY-218\nCo-authored-by: A <a@x>\n"),
            vec![
                json!({ "key": "Refs", "value": "PAY-218" }),
                json!({ "key": "Co-authored-by", "value": "A <a@x>" }),
            ]
        );
    }
}

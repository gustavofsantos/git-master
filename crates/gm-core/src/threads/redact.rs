//! Redaction of thread text before it is stored (design/threads.md, "Redaction (D121)").
//! Pure: no I/O, so every rule is tested on its own.
//!
//! A text is scanned by every enabled pattern, in priority order: the built-in table first,
//! then the user's patterns in configuration order. The matches are merged where they overlap,
//! and each merged range is replaced once by `[REDACTED:<name>]`, where the name is that of the
//! first pattern in priority order. A replacement contains nothing a pattern matches, so
//! redacting a redacted text changes nothing; `redact` repeats the pass until one finds no match,
//! which makes that hold for every input. A match that touches a replacement is ignored.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::OnceLock;

use regex::{Regex, RegexBuilder};

use super::config::RedactConfig;

/// The start of every replacement.
const OPEN: &str = "[REDACTED:";

/// The label of a path whose content is not stored.
const SENSITIVE_FILE: &str = "[REDACTED:sensitive-file]";

/// The largest compiled size of a user pattern, in bytes.
const SIZE_LIMIT: usize = 1 << 20;

/// A bound on the passes of `redact`. Each pass that changes the text removes at least one
/// byte of unredacted text, so the bound is never reached in practice.
const MAX_PASSES: usize = 64;

/// A pattern that does not take part in redaction, with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternProblem {
    pub name: String,
    pub message: String,
}

/// How many ranges one pattern replaced in a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub pattern: String,
    pub count: usize,
}

/// A redacted text and the replacements made, sorted by pattern name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redacted {
    pub text: String,
    pub hits: Vec<Hit>,
}

/// An extra rule on a match, after the regex has run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Check {
    /// Every match is a secret.
    Any,
    /// The value of an assignment: a value that is plainly not a secret is skipped.
    Assignment,
}

struct Builtin {
    name: &'static str,
    pattern: &'static str,
    /// The capture group that holds the secret; `0` is the whole match.
    group: usize,
    check: Check,
}

/// The built-in patterns, in priority order. The regexes are in the threads documentation
/// (design/threads.md, "Patterns"); the tests build every secret from pieces.
const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "private-key",
        pattern: r"(?s)-----BEGIN [^-]*?PRIVATE KEY-----.*?(?:-----END [^-]*-----|\z)",
        group: 0,
        check: Check::Any,
    },
    Builtin {
        name: "aws-access-key-id",
        pattern: r"\b(?:AKIA|ASIA|AGPA|AIDA|AROA|ANPA|ANVA|AIPA)[A-Z0-9]{16}\b",
        group: 0,
        check: Check::Any,
    },
    Builtin {
        name: "aws-secret-access-key",
        pattern: r"(?i)\b[\w.-]*(?:aws[\w.-]*secret|secret[\w.-]*aws)[\w.-]*[\x22']?\s*[:=]\s*[\x22']?([A-Za-z0-9/+=]{40})(?:[^A-Za-z0-9/+=]|$)",
        group: 1,
        check: Check::Any,
    },
    Builtin {
        name: "github-token",
        pattern: r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}\b|github_pat_[A-Za-z0-9_]{50,})",
        group: 0,
        check: Check::Any,
    },
    Builtin {
        name: "gitlab-token",
        pattern: r"\bglpat-[A-Za-z0-9_-]{20,}",
        group: 0,
        check: Check::Any,
    },
    Builtin {
        name: "slack-token",
        pattern: r"\bxox[baprs]-[A-Za-z0-9-]{10,}",
        group: 0,
        check: Check::Any,
    },
    Builtin {
        name: "stripe-key",
        pattern: r"\b[sr]k_live_[A-Za-z0-9]{16,}",
        group: 0,
        check: Check::Any,
    },
    Builtin {
        name: "google-api-key",
        pattern: r"\bAIza[0-9A-Za-z_-]{35}",
        group: 0,
        check: Check::Any,
    },
    Builtin {
        name: "anthropic-api-key",
        pattern: r"\bsk-ant-[A-Za-z0-9_-]{20,}",
        group: 0,
        check: Check::Any,
    },
    Builtin {
        name: "openai-api-key",
        pattern: r"\bsk-(?:proj-)?[A-Za-z0-9_-]{32,}",
        group: 0,
        check: Check::Any,
    },
    Builtin {
        name: "npm-token",
        pattern: r"\bnpm_[A-Za-z0-9]{36}\b",
        group: 0,
        check: Check::Any,
    },
    Builtin {
        name: "jwt",
        pattern: r"\beyJ[A-Za-z0-9_-]{5,}\.eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]+",
        group: 0,
        check: Check::Any,
    },
    Builtin {
        name: "auth-header",
        pattern: r"(?i)authorization\s*[:=]\s*(?:bearer|basic|token)\s+(\S+)",
        group: 1,
        check: Check::Any,
    },
    Builtin {
        name: "url-credentials",
        pattern: r"\b[a-z][a-z0-9+.-]*://[^\s:/@]+:([^\s@/]+)@",
        group: 1,
        check: Check::Any,
    },
    Builtin {
        name: "secret-assignment",
        pattern: r#"(?i)\b[\w.-]*(?:password|passwd|secret|token|api[_-]?key|apikey|private[_-]?key|access[_-]?key)[\w.-]*["']?\s*[:=]\s*["']?([^\s"',;]{8,})"#,
        group: 1,
        check: Check::Assignment,
    },
];

/// A compiled pattern: the built-in table, or a user pattern.
#[derive(Clone, Debug)]
struct Rule {
    name: String,
    regex: Regex,
    group: usize,
    check: Check,
}

/// The redaction rules of one configuration, ready to run on text.
#[derive(Debug)]
pub struct Redactor {
    /// Enabled patterns in priority order.
    rules: Vec<Rule>,
    /// Compiled path globs.
    paths: Vec<Regex>,
}

impl Redactor {
    /// Compiles the enabled patterns and the path globs. Every problem is reported, not only the
    /// first; a configuration with a problem gives no redactor, and capture must stay paused.
    pub fn new(config: &RedactConfig) -> Result<Self, Vec<PatternProblem>> {
        let mut problems = Vec::new();
        // Built-in names are taken even when the built-in is turned off, so a user pattern
        // cannot silently replace one.
        let mut taken: BTreeSet<&str> = BUILTINS.iter().map(|b| b.name).collect();
        let mut user = Vec::new();
        for pattern in &config.patterns {
            let name = pattern.name.as_str();
            if !is_valid_name(name) {
                problems.push(problem(
                    name,
                    "the name must be letters, digits, '-' or '_', and not empty",
                ));
            } else if !taken.insert(name) {
                problems.push(problem(
                    name,
                    "the name repeats a built-in pattern or another user pattern",
                ));
            }
            match RegexBuilder::new(&pattern.regex)
                .size_limit(SIZE_LIMIT)
                .build()
            {
                Err(error) => problems.push(problem(name, format!("the regex does not build: {error}"))),
                Ok(regex) if pattern.group >= regex.captures_len() => problems.push(problem(
                    name,
                    format!(
                        "group {} does not exist; the regex has {} group(s) besides the whole match",
                        pattern.group,
                        regex.captures_len() - 1
                    ),
                )),
                Ok(regex) => user.push(Rule {
                    name: pattern.name.clone(),
                    regex,
                    group: pattern.group,
                    check: Check::Any,
                }),
            }
        }

        let mut paths = Vec::new();
        for glob in &config.paths {
            match Regex::new(&glob_to_regex(glob)) {
                Ok(regex) => paths.push(regex),
                Err(error) => problems.push(problem(&format!("path:{glob}"), error.to_string())),
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }

        let mut rules = Vec::new();
        if config.builtin {
            rules.extend(
                builtin_rules()
                    .iter()
                    .filter(|rule| !config.disabled_builtins.contains(&rule.name))
                    .cloned(),
            );
        }
        rules.extend(user);
        Ok(Self { rules, paths })
    }

    /// Redacts `text`. The result is the same when redacted again.
    #[must_use]
    pub fn redact(&self, text: &str) -> Redacted {
        let mut current = text.to_owned();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for _ in 0..MAX_PASSES {
            let markers = marker_spans(&current);
            let found = self.find(&current, &markers);
            if found.is_empty() {
                break;
            }
            current = self.replace(&current, found, &mut counts);
        }
        Redacted {
            text: current,
            hits: counts
                .into_iter()
                .map(|(pattern, count)| Hit {
                    pattern: pattern.to_owned(),
                    count,
                })
                .collect(),
        }
    }

    /// True when the content of `path` (repository-relative, `/` separators) is not stored.
    #[must_use]
    pub fn is_sensitive_path(&self, path: &str) -> bool {
        let path = path.strip_prefix("./").unwrap_or(path);
        self.paths.iter().any(|glob| glob.is_match(path))
    }

    /// The text that replaces the content of a sensitive file.
    #[must_use]
    pub fn sensitive_file_marker() -> &'static str {
        SENSITIVE_FILE
    }

    /// The secret ranges of every match that counts, with the index of the rule that found each.
    fn find(&self, text: &str, markers: &[Range<usize>]) -> Vec<(Range<usize>, usize)> {
        let mut found = Vec::new();
        for (priority, rule) in self.rules.iter().enumerate() {
            let mut at = 0;
            while at <= text.len() {
                // Only a rule with a group or an extra check needs the captures; the others
                // take the cheaper whole-match search.
                let (whole_range, secret) = if rule.group == 0 && rule.check == Check::Any {
                    let Some(whole) = rule.regex.find_at(text, at) else {
                        break;
                    };
                    (whole.range(), whole.range())
                } else {
                    let Some(captures) = rule.regex.captures_at(text, at) else {
                        break;
                    };
                    let Some(whole) = captures.get(0) else {
                        break;
                    };
                    let secret = captures
                        .get(rule.group)
                        .map_or_else(|| whole.range(), |m| m.range());
                    (whole.range(), secret)
                };
                let ignored = overlaps(&whole_range, markers)
                    || (rule.check == Check::Assignment && is_placeholder(&text[secret.clone()]));
                if ignored {
                    // Look again from the next character, so a match inside this one is not lost.
                    at = next_char(text, whole_range.start);
                    continue;
                }
                if !secret.is_empty() {
                    found.push((secret, priority));
                }
                at = if whole_range.is_empty() {
                    next_char(text, whole_range.start)
                } else {
                    whole_range.end
                };
            }
        }
        found
    }

    /// Replaces the merged ranges of `found` and counts them under the name of their label.
    fn replace<'a>(
        &'a self,
        text: &str,
        mut found: Vec<(Range<usize>, usize)>,
        counts: &mut BTreeMap<&'a str, usize>,
    ) -> String {
        found.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(a.1.cmp(&b.1)));
        let mut merged: Vec<(Range<usize>, usize)> = Vec::with_capacity(found.len());
        for (range, priority) in found {
            match merged.last_mut() {
                Some((last, label)) if range.start <= last.end => {
                    last.end = last.end.max(range.end);
                    *label = (*label).min(priority);
                }
                _ => merged.push((range, priority)),
            }
        }

        let mut out = String::with_capacity(text.len());
        let mut from = 0;
        for (range, priority) in merged {
            let rule = &self.rules[priority];
            out.push_str(&text[from..range.start]);
            out.push_str(OPEN);
            out.push_str(&rule.name);
            out.push(']');
            *counts.entry(rule.name.as_str()).or_insert(0) += 1;
            from = range.end;
        }
        out.push_str(&text[from..]);
        out
    }
}

/// The names of the built-in patterns, in priority order.
#[must_use]
pub fn builtin_names() -> Vec<&'static str> {
    BUILTINS.iter().map(|b| b.name).collect()
}

/// Keeps the head and the tail of `text`, each about half of `max_bytes`, around a note of what
/// was dropped. Returns the kept text, whether anything was dropped, and the original length in
/// bytes. Call it after redaction, so a secret cut by the limit cannot survive. The cuts never
/// split a UTF-8 character or a replacement.
#[must_use]
pub fn truncate(text: &str, max_bytes: usize) -> (String, bool, usize) {
    let len = text.len();
    if len <= max_bytes {
        return (text.to_owned(), false, len);
    }
    let half = max_bytes / 2;
    let mut head = char_floor(text, half);
    let mut tail = char_ceil(text, len - half);
    for span in marker_spans(text) {
        if span.start < head && head < span.end {
            head = span.start;
        }
        if span.start < tail && tail < span.end {
            tail = span.end;
        }
    }
    let dropped = tail - head;
    let kept = format!(
        "{}\n[... truncated {dropped} bytes ...]\n{}",
        &text[..head],
        &text[tail..]
    );
    (kept, true, len)
}

fn problem(name: &str, message: impl Into<String>) -> PatternProblem {
    PatternProblem {
        name: name.to_owned(),
        message: message.into(),
    }
}

/// The compiled built-in patterns, built once per process.
fn builtin_rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        BUILTINS
            .iter()
            .map(|b| Rule {
                name: b.name.to_owned(),
                regex: Regex::new(b.pattern).expect("built-in redaction pattern compiles"),
                group: b.group,
                check: b.check,
            })
            .collect()
    })
}

/// A pattern name: letters, digits, `-` and `_`, at least one of them.
fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The spans of the replacements already in `text`, sorted and disjoint. A replacement is the
/// exact shape the redactor writes, so text that only looks like one is still scanned.
fn marker_spans(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    for (start, _) in text.match_indices(OPEN) {
        let name_start = start + OPEN.len();
        let mut end = name_start;
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'_' | b'-'))
        {
            end += 1;
        }
        if end > name_start && bytes.get(end) == Some(&b']') {
            spans.push(start..end + 1);
        }
    }
    spans
}

/// True when `range` starts inside or contains a replacement. `markers` is sorted and disjoint.
fn overlaps(range: &Range<usize>, markers: &[Range<usize>]) -> bool {
    let first = markers.partition_point(|marker| marker.end <= range.start);
    markers
        .get(first)
        .is_some_and(|marker| marker.start < range.end)
}

/// A value of an assignment that is plainly not a secret: a boolean or null, digits only, an
/// environment reference, or a replacement.
fn is_placeholder(value: &str) -> bool {
    matches!(value, "true" | "false" | "null")
        || value.bytes().all(|b| b.is_ascii_digit())
        || value.starts_with('$')
        || value.starts_with(OPEN)
}

/// The byte index after the character at `index`.
fn next_char(text: &str, index: usize) -> usize {
    index + text[index..].chars().next().map_or(1, char::len_utf8)
}

/// The largest character boundary at or before `index`.
fn char_floor(text: &str, index: usize) -> usize {
    let mut at = index;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The smallest character boundary at or after `index`.
fn char_ceil(text: &str, index: usize) -> usize {
    let mut at = index;
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// A path glob as an anchored regex. `*` is any run of characters except `/`, `**` is any run
/// including `/` (and `**/` also matches no directory), `?` is one character except `/`. A glob
/// without `/` matches the file name at any depth.
fn glob_to_regex(glob: &str) -> String {
    let mut out = String::from(if glob.contains('/') { "^" } else { "(?:^|/)" });
    let mut chars = glob.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    out.push_str("(?:.*/)?");
                } else {
                    out.push_str(".*");
                }
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            other => {
                let mut buf = [0; 4];
                out.push_str(&regex::escape(other.encode_utf8(&mut buf)));
            }
        }
    }
    out.push('$');
    out
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use proptest::prelude::*;

    use super::*;

    fn default_redactor() -> Redactor {
        Redactor::new(&RedactConfig::default()).expect("the defaults are valid")
    }

    fn redact_default(text: &str) -> String {
        default_redactor().redact(text).text
    }

    fn with_patterns(patterns: Vec<(&str, &str, usize)>) -> RedactConfig {
        RedactConfig {
            patterns: patterns
                .into_iter()
                .map(|(name, regex, group)| super::super::config::UserPattern {
                    name: name.into(),
                    regex: regex.into(),
                    group,
                })
                .collect(),
            ..RedactConfig::default()
        }
    }

    #[test]
    fn builtin_names_are_in_priority_order() {
        assert_eq!(
            builtin_names(),
            [
                "private-key",
                "aws-access-key-id",
                "aws-secret-access-key",
                "github-token",
                "gitlab-token",
                "slack-token",
                "stripe-key",
                "google-api-key",
                "anthropic-api-key",
                "openai-api-key",
                "npm-token",
                "jwt",
                "auth-header",
                "url-credentials",
                "secret-assignment",
            ]
        );
    }

    #[test]
    fn private_key_block_is_one_marker() {
        let begin = ["-----BEGIN RSA ", "PRIVATE KEY-----"].concat();
        let end = ["-----END RSA ", "PRIVATE KEY-----"].concat();
        let text = format!("before\n{begin}\nMIIEowIBAAKCAQEA\nabc\n{end}\nafter");
        assert_eq!(
            redact_default(&text),
            "before\n[REDACTED:private-key]\nafter"
        );
    }

    #[test]
    fn unterminated_private_key_runs_to_the_end() {
        let begin = ["-----BEGIN OPENSSH ", "PRIVATE KEY-----"].concat();
        let text = format!("{begin}\nabc\ndef");
        assert_eq!(redact_default(&text), "[REDACTED:private-key]");
    }

    #[test]
    fn aws_access_key_id() {
        let key = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
        assert_eq!(
            redact_default(&format!("id {key} end")),
            "id [REDACTED:aws-access-key-id] end"
        );
    }

    #[test]
    fn aws_secret_access_key_keeps_its_name() {
        let text = format!("aws_secret_access_key = {}", "a1B2".repeat(10));
        assert_eq!(
            redact_default(&text),
            "aws_secret_access_key = [REDACTED:aws-secret-access-key]"
        );
    }

    #[test]
    fn github_tokens() {
        let classic = format!("ghp_{}", "a".repeat(36));
        assert_eq!(redact_default(&classic), "[REDACTED:github-token]");
        let fine_grained = format!("github_pat_{}", "A1_".repeat(17) + "A");
        assert_eq!(redact_default(&fine_grained), "[REDACTED:github-token]");
    }

    #[test]
    fn gitlab_slack_and_stripe_tokens() {
        let gitlab = format!("glpat-{}", "abcdefghij0123456789");
        assert_eq!(redact_default(&gitlab), "[REDACTED:gitlab-token]");
        let slack = format!("xoxb-{}", "1234567890-abcdef");
        assert_eq!(redact_default(&slack), "[REDACTED:slack-token]");
        let stripe = format!("sk_live_{}", "a1".repeat(8));
        assert_eq!(redact_default(&stripe), "[REDACTED:stripe-key]");
    }

    #[test]
    fn google_anthropic_openai_and_npm_keys() {
        let google = format!("AIza{}", "0123456789abcdefghijklmnopqrstuvwxy");
        assert_eq!(redact_default(&google), "[REDACTED:google-api-key]");
        let anthropic = format!("sk-ant-{}", "api03-AbCdEfGh_123456");
        assert_eq!(redact_default(&anthropic), "[REDACTED:anthropic-api-key]");
        let project = format!("sk-proj-{}", "a".repeat(40));
        assert_eq!(redact_default(&project), "[REDACTED:openai-api-key]");
        let legacy = format!("sk-{}", "B".repeat(48));
        assert_eq!(redact_default(&legacy), "[REDACTED:openai-api-key]");
        let npm = format!("npm_{}", "a".repeat(36));
        assert_eq!(redact_default(&npm), "[REDACTED:npm-token]");
    }

    #[test]
    fn jwt() {
        let token = format!("{}.{}.{}", "eyJhbGciOi", "eyJzdWIiOiIx", "c2lnbmF0dXJl");
        assert_eq!(redact_default(&token), "[REDACTED:jwt]");
    }

    #[test]
    fn auth_header_keeps_the_scheme() {
        assert_eq!(
            redact_default("Authorization: Bearer abc123XYZ"),
            "Authorization: Bearer [REDACTED:auth-header]"
        );
        assert_eq!(
            redact_default("authorization=Basic dXNlcjpwYXNz"),
            "authorization=Basic [REDACTED:auth-header]"
        );
    }

    #[test]
    fn url_credentials_keep_the_user() {
        assert_eq!(
            redact_default("https://user:hunter2hunter2@example.com/repo"),
            "https://user:[REDACTED:url-credentials]@example.com/repo"
        );
    }

    #[test]
    fn secret_assignments_in_several_shapes() {
        assert_eq!(
            redact_default("DB_PASSWORD=correcthorse"),
            "DB_PASSWORD=[REDACTED:secret-assignment]"
        );
        assert_eq!(
            redact_default(r#""api_key": "abcdefgh1234""#),
            r#""api_key": "[REDACTED:secret-assignment]""#
        );
        assert_eq!(
            redact_default("client_secret: hunter2hunter2"),
            "client_secret: [REDACTED:secret-assignment]"
        );
        assert_eq!(
            redact_default("export GITHUB_TOKEN=xyzzy-plugh-1234"),
            "export GITHUB_TOKEN=[REDACTED:secret-assignment]"
        );
    }

    #[test]
    fn near_misses_are_left_alone() {
        let texts = [
            "token_count = 12345678".to_owned(),
            "password=true".to_owned(),
            "password=false".to_owned(),
            "secret: null".to_owned(),
            "password=$TOKEN_VALUE".to_owned(),
            "password=${TOKEN_VALUE}".to_owned(),
            "the token is short".to_owned(),
            "sk-abc".to_owned(),
            "ghp_short".to_owned(),
            "AKIA-short".to_owned(),
            "eyJhbGciOi.only_one_segment".to_owned(),
            "https://example.com/path".to_owned(),
            "AIza-too-short".to_owned(),
            "password=[REDACTED:secret-assignment]".to_owned(),
        ];
        for text in texts {
            assert_eq!(redact_default(&text), text, "must not match: {text}");
        }
    }

    #[test]
    fn a_replacement_is_not_matched_again() {
        let once = redact_default("token=abcdefgh1234");
        assert_eq!(once, "token=[REDACTED:secret-assignment]");
        assert_eq!(redact_default(&once), once);
    }

    #[test]
    fn user_pattern_without_group_replaces_the_whole_match() {
        let config = with_patterns(vec![("acme-token", "acme_[A-Za-z0-9]{32}", 0)]);
        let redactor = Redactor::new(&config).expect("valid");
        let text = format!("key acme_{} end", "Q".repeat(32));
        assert_eq!(redactor.redact(&text).text, "key [REDACTED:acme-token] end");
    }

    #[test]
    fn user_pattern_with_group_keeps_the_key() {
        let config = with_patterns(vec![("db-pass", r"(DB_PASS=)(\S+)", 2)]);
        let redactor = Redactor::new(&config).expect("valid");
        assert_eq!(
            redactor.redact("DB_PASS=hunter2 ok").text,
            "DB_PASS=[REDACTED:db-pass] ok"
        );
    }

    #[test]
    fn invalid_user_patterns_are_all_reported() {
        let config = with_patterns(vec![
            ("bad name!", "(", 0),
            ("", "x", 0),
            ("jwt", "x", 0),
            ("dup", "a", 0),
            ("dup", "b", 0),
            ("grp", "(a)", 2),
        ]);
        let problems = Redactor::new(&config).expect_err("invalid patterns");
        let names: Vec<&str> = problems.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["bad name!", "bad name!", "", "jwt", "dup", "grp"]);
        assert!(problems[1].message.contains("regex"));
        assert!(problems[5].message.contains("group 2"));
    }

    #[test]
    fn a_bad_regex_alone_is_one_problem() {
        let config = with_patterns(vec![("ok-name", "[unclosed", 0)]);
        let problems = Redactor::new(&config).expect_err("invalid");
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].name, "ok-name");
    }

    #[test]
    fn disabled_builtin_is_off_and_the_rest_stay_on() {
        let config = RedactConfig {
            disabled_builtins: vec!["jwt".into()],
            ..RedactConfig::default()
        };
        let redactor = Redactor::new(&config).expect("valid");
        let jwt = format!("{}.{}.{}", "eyJhbGciOi", "eyJzdWIiOiIx", "c2lnbmF0dXJl");
        assert_eq!(redactor.redact(&jwt).text, jwt);
        let github = format!("ghp_{}", "a".repeat(36));
        assert_eq!(redactor.redact(&github).text, "[REDACTED:github-token]");
    }

    #[test]
    fn unknown_disabled_name_is_ignored() {
        let config = RedactConfig {
            disabled_builtins: vec!["no-such-pattern".into()],
            ..RedactConfig::default()
        };
        let redactor = Redactor::new(&config).expect("valid");
        let github = format!("ghp_{}", "a".repeat(36));
        assert_eq!(redactor.redact(&github).text, "[REDACTED:github-token]");
    }

    #[test]
    fn builtin_switch_turns_all_builtins_off() {
        let config = RedactConfig {
            builtin: false,
            patterns: with_patterns(vec![("acme", "acme_[a-z]+", 0)]).patterns,
            ..RedactConfig::default()
        };
        let redactor = Redactor::new(&config).expect("valid");
        let github = format!("ghp_{}", "a".repeat(36));
        let text = format!("{github} acme_xyz");
        assert_eq!(
            redactor.redact(&text).text,
            format!("{github} [REDACTED:acme]")
        );
    }

    #[test]
    fn overlapping_matches_become_one_marker() {
        let config = with_patterns(vec![("tail", "xyz", 0)]);
        let redactor = Redactor::new(&config).expect("valid");
        let redacted = redactor.redact("password=abcdefxyz");
        assert_eq!(redacted.text, "password=[REDACTED:secret-assignment]");
        assert_eq!(
            redacted.hits,
            [Hit {
                pattern: "secret-assignment".into(),
                count: 1
            }]
        );
    }

    #[test]
    fn hits_count_per_pattern_sorted_by_name() {
        let token = format!("ghp_{}", "a".repeat(36));
        let jwt = format!("{}.{}.{}", "eyJhbGciOi", "eyJzdWIiOiIx", "c2lnbmF0dXJl");
        let text = format!("{token} and {token} and {jwt}");
        let redacted = default_redactor().redact(&text);
        assert_eq!(
            redacted.text,
            "[REDACTED:github-token] and [REDACTED:github-token] and [REDACTED:jwt]"
        );
        assert_eq!(
            redacted.hits,
            [
                Hit {
                    pattern: "github-token".into(),
                    count: 2
                },
                Hit {
                    pattern: "jwt".into(),
                    count: 1
                },
            ]
        );
    }

    #[test]
    fn plain_text_has_no_hits() {
        let redacted = default_redactor().redact("nothing to see here");
        assert_eq!(redacted.text, "nothing to see here");
        assert!(redacted.hits.is_empty());
    }

    #[test]
    fn sensitive_paths_by_default() {
        let redactor = default_redactor();
        for path in [
            ".env",
            "a/b/.env",
            ".env.local",
            "x/y.pem",
            "id_rsa",
            "id_rsa.pub",
            "./.env",
        ] {
            assert!(redactor.is_sensitive_path(path), "{path}");
        }
        for path in [
            "src/main.rs",
            ".envrc",
            "pem.txt",
            "keys/id_ed25519.pub.txt",
        ] {
            assert!(!redactor.is_sensitive_path(path), "{path}");
        }
    }

    #[test]
    fn path_globs_with_slashes_are_anchored() {
        let config = RedactConfig {
            paths: vec!["src/*.rs".into(), "**/secrets/**".into()],
            ..RedactConfig::default()
        };
        let redactor = Redactor::new(&config).expect("valid");
        assert!(redactor.is_sensitive_path("src/a.rs"));
        assert!(!redactor.is_sensitive_path("src/x/a.rs"));
        assert!(!redactor.is_sensitive_path("a.rs"));
        assert!(redactor.is_sensitive_path("secrets/a"));
        assert!(redactor.is_sensitive_path("a/secrets/b/c"));
        assert!(!redactor.is_sensitive_path("notsecrets/b"));
    }

    #[test]
    fn question_mark_and_metacharacters_are_literal_or_single() {
        let config = RedactConfig {
            paths: vec!["file?.txt".into(), "a+b(1).md".into()],
            ..RedactConfig::default()
        };
        let redactor = Redactor::new(&config).expect("valid");
        assert!(redactor.is_sensitive_path("dir/file1.txt"));
        assert!(!redactor.is_sensitive_path("dir/file12.txt"));
        assert!(redactor.is_sensitive_path("a+b(1).md"));
        assert!(!redactor.is_sensitive_path("aab1.md"));
    }

    #[test]
    fn sensitive_file_marker_is_fixed() {
        assert_eq!(
            Redactor::sensitive_file_marker(),
            "[REDACTED:sensitive-file]"
        );
    }

    #[test]
    fn truncate_keeps_short_text() {
        assert_eq!(truncate("abc", 10), ("abc".to_owned(), false, 3));
        assert_eq!(truncate("abc", 3), ("abc".to_owned(), false, 3));
    }

    #[test]
    fn truncate_keeps_head_and_tail() {
        let text = format!("{}{}", "a".repeat(100), "b".repeat(100));
        let (kept, truncated, original) = truncate(&text, 40);
        assert!(truncated);
        assert_eq!(original, 200);
        assert_eq!(
            kept,
            format!(
                "{}\n[... truncated 160 bytes ...]\n{}",
                "a".repeat(20),
                "b".repeat(20)
            )
        );
    }

    #[test]
    fn truncate_never_splits_a_character() {
        let text = "é".repeat(100);
        for max in 0..60 {
            let (kept, truncated, _) = truncate(&text, max);
            assert!(truncated || max >= 200);
            let without_note: String = kept
                .split('\n')
                .filter(|line| !line.starts_with("[... truncated"))
                .collect();
            assert!(without_note.chars().all(|c| c == 'é'), "max {max}");
        }
    }

    #[test]
    fn truncate_does_not_cut_a_replacement() {
        let text = format!(
            "{}{}{}",
            "x".repeat(10),
            "[REDACTED:secret-assignment]",
            "y".repeat(10)
        );
        // Half of 24 is 12, which lands inside the replacement at bytes 10..38.
        let (kept, truncated, original) = truncate(&text, 24);
        assert!(truncated);
        assert_eq!(original, 48);
        assert_eq!(
            kept,
            format!(
                "{}\n[... truncated 28 bytes ...]\n{}",
                "x".repeat(10),
                "y".repeat(10)
            )
        );
    }

    /// One mebibyte of prose with secrets in it, as the redactor reads it.
    fn one_mebibyte() -> (String, String) {
        let token = format!("ghp_{}", "a".repeat(36));
        let line = format!("prose line {token} password=hunter2hunter2 token_count = 12345678\n");
        let mut text = String::new();
        while text.len() < 1 << 20 {
            text.push_str(&line);
        }
        (text, token)
    }

    #[test]
    fn redaction_of_one_mebibyte_removes_the_secrets() {
        let (text, token) = one_mebibyte();
        let redacted = default_redactor().redact(&text);
        assert!(!redacted.text.contains(&token));
        assert!(redacted.text.contains("token_count = 12345678"));
    }

    /// The cost of redacting one mebibyte, for a release build:
    /// `cargo test --release -p gm-core --lib redaction_of_one_mebibyte_is_fast -- --ignored`.
    /// Debug builds are several times slower and do not run it.
    #[test]
    #[ignore = "a measurement for release builds"]
    fn redaction_of_one_mebibyte_is_fast() {
        let (text, _) = one_mebibyte();
        let start = Instant::now();
        let _ = default_redactor().redact(&text);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(10),
            "redaction took {elapsed:?}"
        );
    }

    /// Fragments that mix prose, markers and secret-like text.
    const PIECES: &[&str] = &[
        "a",
        "Z",
        "9",
        " ",
        "\n",
        "é",
        "=",
        ":",
        "\"",
        "'",
        "$",
        "[REDACTED:",
        "]",
        "[REDACTED:jwt]",
        "password=",
        "token=",
        "Authorization: Bearer ",
        "https://u:",
        "@host",
        "AKIA",
        "eyJ",
        ".eyJ",
        "-----BEGIN PRIVATE KEY-----",
        "-----END PRIVATE KEY-----",
        "ghp_",
        "sk-ant-",
    ];

    /// A secret that some built-in pattern must remove, with the part that must not survive.
    fn secret() -> impl Strategy<Value = (String, String)> {
        prop_oneof![
            "[A-Za-z0-9]{36}".prop_map(|s| (["ghp_", &s].concat(), s)),
            "[A-Z0-9]{16}".prop_map(|s| (["AKIA", &s].concat(), s)),
            "[A-Za-z0-9]{20,30}".prop_map(|s| (["sk-ant-", &s].concat(), s)),
            "[a-z][A-Za-z0-9]{7,19}".prop_map(|s| (["password=", &s].concat(), s)),
            "[a-z][A-Za-z0-9]{7,19}".prop_map(|s| (["API_KEY: ", &s].concat(), s)),
            "[A-Za-z0-9]{40}".prop_map(|s| (["AWS_SECRET_ACCESS_KEY=", &s].concat(), s)),
            "[A-Za-z0-9]{20,30}".prop_map(|s| (["glpat-", &s].concat(), s)),
        ]
    }

    proptest! {
        #[test]
        fn redaction_is_idempotent_for_any_text(text in "(?s).*") {
            let redactor = default_redactor();
            let once = redactor.redact(&text).text;
            prop_assert_eq!(redactor.redact(&once).text, once);
        }

        #[test]
        fn redaction_is_idempotent_for_stitched_text(
            pieces in prop::collection::vec(prop::sample::select(PIECES), 0..60),
        ) {
            let redactor = default_redactor();
            let text = pieces.concat();
            let once = redactor.redact(&text).text;
            prop_assert_eq!(redactor.redact(&once).text, once);
        }

        #[test]
        fn redaction_is_idempotent_with_embedded_secrets(
            before in "(?s).*",
            after in "(?s).*",
            (text, _) in secret(),
        ) {
            let redactor = default_redactor();
            let input = format!("{before} {text} {after}");
            let once = redactor.redact(&input).text;
            prop_assert_eq!(redactor.redact(&once).text, once);
        }

        #[test]
        fn embedded_secrets_never_survive(
            before in "(?s).*",
            secrets in prop::collection::vec(secret(), 1..5),
            after in "(?s).*",
        ) {
            let redactor = default_redactor();
            let mut input = before;
            for (text, _) in &secrets {
                input.push(' ');
                input.push_str(text);
            }
            input.push(' ');
            input.push_str(&after);
            let out = redactor.redact(&input).text;
            for (_, value) in &secrets {
                prop_assert!(!out.contains(value.as_str()), "{value} survived in {out}");
            }
        }

        #[test]
        fn redaction_never_panics_on_bytes(bytes in prop::collection::vec(any::<u8>(), 0..2000)) {
            let text = String::from_utf8_lossy(&bytes);
            let _ = default_redactor().redact(&text);
        }

        #[test]
        fn truncation_keeps_replacements_whole(
            pieces in prop::collection::vec(prop::sample::select(&["abc", "é", "[REDACTED:jwt]", " ", "[REDACTED:x-1]"][..]), 0..80),
            max in 0usize..300,
        ) {
            let text = pieces.concat();
            let (kept, _, original) = truncate(&text, max);
            prop_assert_eq!(original, text.len());
            let note_free: String = kept
                .split('\n')
                .filter(|line| !line.starts_with("[... truncated"))
                .collect::<Vec<_>>()
                .join("\n");
            prop_assert_eq!(
                note_free.matches(OPEN).count(),
                marker_spans(&note_free).len()
            );
        }
    }
}

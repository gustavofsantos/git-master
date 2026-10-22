//! Reads `[gitmaster "prepare-commit.<name>"]` sections from the key/value
//! pairs git prints for them. Pure: the adapter hands in what git said, and
//! this module decides what each section means (design/hooks-transitional.md).

use std::time::Duration;

use gm_schema::HookScope;

/// What a section runs with when it sets no `timeout`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

const PREFIX: &str = "gitmaster.prepare-commit.";

/// One key from `git config --null --get-regexp`. `value` is `None` for a
/// bare key, which git reads as `true`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigValue {
    pub key: String,
    pub value: Option<String>,
}

/// One section, validated. `problem` says why it cannot run; the other fields
/// hold what could be read, for the listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub scope: HookScope,
    pub command: String,
    /// The raw `match` value, such as `branch:feat/*`.
    pub match_rule: Option<String>,
    pub timeout: Duration,
    pub enabled: bool,
    pub problem: Option<String>,
}

impl Section {
    fn new(name: &str, scope: HookScope) -> Self {
        Self {
            name: name.to_owned(),
            scope,
            command: String::new(),
            match_rule: None,
            timeout: DEFAULT_TIMEOUT,
            enabled: true,
            problem: None,
        }
    }

    /// The glob after `branch:`, when the section has a `match`.
    #[must_use]
    pub fn branch_glob(&self) -> Option<&str> {
        self.match_rule.as_deref()?.strip_prefix("branch:")
    }
}

/// Sections of one scope, in config order: the order a chain runs in.
#[must_use]
pub fn parse(entries: &[ConfigValue], scope: HookScope) -> Vec<Section> {
    // A section's name and its `(variable, value)` pairs, in config order.
    type Group<'a> = (String, Vec<(&'a str, Option<&'a str>)>);
    let mut grouped: Vec<Group> = Vec::new();
    for entry in entries {
        let Some(rest) = entry.key.strip_prefix(PREFIX) else {
            continue;
        };
        // The variable is the last dotted part; the name may itself contain dots.
        let Some((name, var)) = rest.rsplit_once('.') else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let value = entry.value.as_deref();
        match grouped.iter_mut().find(|(n, _)| n == name) {
            Some((_, vars)) => vars.push((var, value)),
            None => grouped.push((name.to_owned(), vec![(var, value)])),
        }
    }
    grouped
        .into_iter()
        .map(|(name, vars)| build(&name, scope, &vars))
        .collect()
}

fn build(name: &str, scope: HookScope, vars: &[(&str, Option<&str>)]) -> Section {
    let mut section = Section::new(name, scope);
    let mut seen: Vec<&str> = Vec::new();
    let mut problem: Option<String> = None;
    for &(var, value) in vars {
        if seen.contains(&var) {
            problem.get_or_insert(format!("{var} is set more than once"));
            continue;
        }
        seen.push(var);
        let outcome = match var {
            "command" => match value {
                Some(command) if !command.trim().is_empty() => {
                    command.clone_into(&mut section.command);
                    Ok(())
                }
                _ => Err("command is empty".to_owned()),
            },
            "match" => match value {
                Some(rule) if rule.strip_prefix("branch:").is_some_and(|g| !g.is_empty()) => {
                    section.match_rule = Some(rule.to_owned());
                    Ok(())
                }
                _ => Err("match must be branch:<glob>".to_owned()),
            },
            "timeout" => parse_duration(value).map(|timeout| section.timeout = timeout),
            "enabled" => parse_bool(value).map(|enabled| section.enabled = enabled),
            other => Err(format!("unknown key {other}")),
        };
        if let Err(reason) = outcome {
            problem.get_or_insert(reason);
        }
    }
    if !seen.contains(&"command") {
        problem.get_or_insert("command is missing".to_owned());
    }
    section.problem = problem;
    section
}

/// `30s`, `500ms` or `2m`. Plain numbers are refused so the unit is never guessed.
pub fn parse_duration(value: Option<&str>) -> Result<Duration, String> {
    let text = value.unwrap_or("").trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, unit) = text.split_at(split);
    let amount: u64 = digits
        .parse()
        .map_err(|_| "timeout must look like 30s, 500ms or 2m".to_owned())?;
    if amount == 0 {
        return Err("timeout must be above zero".to_owned());
    }
    match unit {
        "ms" => Ok(Duration::from_millis(amount)),
        "s" => Ok(Duration::from_secs(amount)),
        "m" => Ok(Duration::from_secs(amount.saturating_mul(60))),
        _ => Err("timeout must look like 30s, 500ms or 2m".to_owned()),
    }
}

/// The spellings `git config --type=bool` accepts. A bare key is `true`.
fn parse_bool(value: Option<&str>) -> Result<bool, String> {
    let Some(text) = value else {
        return Ok(true);
    };
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" | "" => Ok(false),
        _ => Err("enabled must be true or false".to_owned()),
    }
}

/// User sections first, then repository ones. A repository section that reuses
/// the name of a user section is refused: the name is how approval finds it.
#[must_use]
pub fn merge(user: Vec<Section>, repo: Vec<Section>) -> Vec<Section> {
    let mut all = user;
    for mut section in repo {
        if all.iter().any(|other| other.name == section.name) {
            section
                .problem
                .get_or_insert("a user hook has the same name".to_owned());
        }
        all.push(section);
    }
    all
}

/// Glob matching for `match`: `*` matches any run of characters, including
/// `/`, and `?` matches one character.
#[must_use]
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    // The last `*`, and the text position it currently absorbs up to.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(key: &str, value: Option<&str>) -> ConfigValue {
        ConfigValue {
            key: format!("{PREFIX}{key}"),
            value: value.map(str::to_owned),
        }
    }

    fn one(entries: &[ConfigValue]) -> Section {
        let mut all = parse(entries, HookScope::User);
        assert_eq!(all.len(), 1);
        all.remove(0)
    }

    #[test]
    fn a_minimal_section_gets_the_defaults() {
        let section = one(&[kv("llm.command", Some("suggest"))]);
        assert_eq!(section.name, "llm");
        assert_eq!(section.command, "suggest");
        assert_eq!(section.timeout, Duration::from_secs(30));
        assert!(section.enabled);
        assert_eq!(section.match_rule, None);
        assert_eq!(section.problem, None);
    }

    #[test]
    fn every_key_is_read() {
        let section = one(&[
            kv("llm.command", Some("suggest --fast")),
            kv("llm.match", Some("branch:feat/*")),
            kv("llm.timeout", Some("500ms")),
            kv("llm.enabled", Some("no")),
        ]);
        assert_eq!(section.command, "suggest --fast");
        assert_eq!(section.branch_glob(), Some("feat/*"));
        assert_eq!(section.timeout, Duration::from_millis(500));
        assert!(!section.enabled);
        assert_eq!(section.problem, None);
    }

    #[test]
    fn a_name_may_contain_dots() {
        let section = one(&[kv("team.v2.command", Some("x"))]);
        assert_eq!(section.name, "team.v2");
    }

    #[test]
    fn a_bare_enabled_key_is_true() {
        assert!(one(&[kv("a.command", Some("x")), kv("a.enabled", None)]).enabled);
    }

    #[test]
    fn sections_keep_config_order() {
        let sections = parse(
            &[
                kv("second.command", Some("b")),
                kv("first.command", Some("a")),
                kv("second.timeout", Some("1s")),
            ],
            HookScope::User,
        );
        let names: Vec<_> = sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["second", "first"]);
    }

    #[test]
    fn problems_are_reported_per_section() {
        let cases: [(&[ConfigValue], &str); 7] = [
            (&[kv("a.timeout", Some("1s"))], "command is missing"),
            (&[kv("a.command", Some("  "))], "command is empty"),
            (
                &[kv("a.command", Some("x")), kv("a.timeout", Some("30"))],
                "timeout must look like 30s, 500ms or 2m",
            ),
            (
                &[kv("a.command", Some("x")), kv("a.match", Some("main"))],
                "match must be branch:<glob>",
            ),
            (
                &[kv("a.command", Some("x")), kv("a.enabled", Some("maybe"))],
                "enabled must be true or false",
            ),
            (
                &[kv("a.command", Some("x")), kv("a.colour", Some("red"))],
                "unknown key colour",
            ),
            (
                &[kv("a.command", Some("x")), kv("a.command", Some("y"))],
                "command is set more than once",
            ),
        ];
        for (entries, reason) in cases {
            assert_eq!(one(entries).problem.as_deref(), Some(reason));
        }
    }

    #[test]
    fn durations_need_a_unit() {
        assert_eq!(parse_duration(Some("2m")), Ok(Duration::from_secs(120)));
        assert_eq!(parse_duration(Some("7s")), Ok(Duration::from_secs(7)));
        assert_eq!(
            parse_duration(Some("0s")),
            Err("timeout must be above zero".into())
        );
        assert!(parse_duration(Some("10 s")).is_err());
        assert!(parse_duration(None).is_err());
    }

    #[test]
    fn other_scopes_and_keys_are_ignored() {
        let entries = [
            ConfigValue {
                key: "gitmaster.prepare-commit".into(),
                value: Some("x".into()),
            },
            ConfigValue {
                key: "user.name".into(),
                value: Some("x".into()),
            },
        ];
        assert!(parse(&entries, HookScope::Repo).is_empty());
    }

    #[test]
    fn a_repo_section_cannot_reuse_a_user_name() {
        let user = parse(&[kv("llm.command", Some("a"))], HookScope::User);
        let repo = parse(&[kv("llm.command", Some("b"))], HookScope::Repo);
        let merged = merge(user, repo);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].problem, None);
        assert_eq!(
            merged[1].problem.as_deref(),
            Some("a user hook has the same name")
        );
    }

    #[test]
    fn glob_matches_branch_names() {
        assert!(glob_match("feat/*", "feat/x"));
        assert!(glob_match("feat/*", "feat/x/y"));
        assert!(!glob_match("feat/*", "main"));
        assert!(!glob_match("feat/*", "feature"));
        assert!(glob_match("main", "main"));
        assert!(!glob_match("main", "main2"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
        assert!(glob_match("*fix*", "hotfix-12"));
        assert!(!glob_match("", "x"));
    }
}

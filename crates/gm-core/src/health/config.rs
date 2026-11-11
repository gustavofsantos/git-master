//! Code health: configuration (see mod.rs and docs/guide/code-health.md#configuration).
//!
//! Settings are Git configuration keys. `read` is the only function that runs `git`;
//! parsing, hashing and the excluder are pure. An invalid value is ignored and the
//! default stays. Git prints values in precedence order, so the last value wins.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;
use std::process::Command;

use regex::Regex;
use sha2::{Digest, Sha256};

use super::{HealthConfig, ScopeConfig};

/// Lock files of common package managers, left out by default (D225 (3)).
const LOCK_FILES: [&str; 17] = [
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "go.sum",
    "poetry.lock",
    "Gemfile.lock",
    "composer.lock",
    "Pipfile.lock",
    "bun.lockb",
    "uv.lock",
    "mix.lock",
    "pubspec.lock",
    "Podfile.lock",
    "flake.lock",
    "packages.lock.json",
];

/// Parses the output of `git config --null --get-regexp <re>`: records end in `\0`,
/// and in each record the key ends at the first `\n`; the rest is the value.
pub fn parse_get_regexp(bytes: &[u8]) -> Vec<(String, String)> {
    bytes
        .split(|&b| b == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let text = String::from_utf8_lossy(record);
            match text.split_once('\n') {
                Some((key, value)) => (key.to_owned(), value.to_owned()),
                None => (text.to_string(), String::new()),
            }
        })
        .collect()
}

/// Builds the configuration from `(key, value)` entries in precedence order.
pub fn parse(entries: &[(String, String)]) -> HealthConfig {
    let mut config = HealthConfig::default();
    let mut scopes: BTreeMap<String, ScopeConfig> = BTreeMap::new();
    for (key, value) in entries {
        let value = value.trim();
        if let Some((path, var)) = scope_key(key) {
            apply_scope(&mut scopes, path, &var, value);
        } else if let Some(var) = repo_key(key) {
            apply_repo(&mut config, &var, value);
        }
    }
    config.scopes = scopes.into_values().collect();
    config
}

/// Reads the configuration of the repository at `root`. A repository with no
/// health key gives the defaults; any other `git` failure is an error.
pub fn read(root: &Path) -> Result<HealthConfig, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "config",
            "--null",
            "--get-regexp",
            r"^gitmaster\.health\.|^gitmaster-health\.",
        ])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    match output.status.code() {
        Some(0) => Ok(parse(&parse_get_regexp(&output.stdout))),
        Some(1) => Ok(HealthConfig::default()),
        _ => {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            if message.is_empty() {
                Err(format!("git config failed: {}", output.status))
            } else {
                Err(message)
            }
        }
    }
}

/// The first 8 hex digits of the SHA-256 of the canonical text of `c`.
pub fn config_hash(c: &HealthConfig) -> String {
    let mut exclude = c.exclude.clone();
    exclude.sort();
    exclude.dedup();
    let mut ignore_revs = c.ignore_revs.clone();
    ignore_revs.sort();
    ignore_revs.dedup();
    let mut scopes: Vec<&ScopeConfig> = c.scopes.iter().collect();
    scopes.sort_by(|a, b| a.path.cmp(&b.path));

    let mut text = format!(
        "half_life={:?}\nmin_half_life_days={:?}\nmax_half_life_days={:?}\ndormancy_gap={:?}\nwindow_days={:?}\n",
        c.half_life, c.min_half_life_days, c.max_half_life_days, c.dormancy_gap, c.window_days,
    );
    for value in &exclude {
        let _ = writeln!(text, "exclude={value:?}");
    }
    for rev in &ignore_revs {
        let _ = writeln!(text, "ignore_rev={rev:?}");
    }
    for scope in scopes {
        let _ = writeln!(
            text,
            "scope={:?} half_life={:?} dormancy_gap={:?}",
            scope.path, scope.half_life, scope.dormancy_gap
        );
    }
    let digest = Sha256::digest(text.as_bytes());
    let mut hex = String::new();
    for byte in digest.iter().take(4) {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Decides which paths are left out: lock files, `exclude` values and patterns.
pub struct Excluder {
    exact: Vec<String>,
    prefixes: Vec<String>,
    globs: Vec<Regex>,
}

impl Excluder {
    /// Compiles the `exclude` values of `c`; an invalid pattern is ignored.
    pub fn new(c: &HealthConfig) -> Self {
        let mut exact = Vec::new();
        let mut prefixes = Vec::new();
        let mut globs = Vec::new();
        for value in &c.exclude {
            let value = value.trim_start_matches('/');
            if value.is_empty() {
                continue;
            }
            if is_pattern(value) {
                if let Some(re) = glob_regex(value, value.ends_with('/')) {
                    globs.push(re);
                }
            } else if value.ends_with('/') {
                prefixes.push(value.to_owned());
            } else {
                exact.push(value.to_owned());
            }
        }
        Self {
            exact,
            prefixes,
            globs,
        }
    }

    /// Whether `path` (relative to the root, with `/`) is left out.
    pub fn excluded(&self, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        LOCK_FILES.contains(&name)
            || self.exact.iter().any(|value| value == path)
            || self
                .prefixes
                .iter()
                .any(|prefix| path.starts_with(prefix.as_str()))
            || self.globs.iter().any(|re| re.is_match(path))
    }
}

/// The scope with the longest path that contains `path` on a folder boundary.
pub fn scope_of<'a>(c: &'a HealthConfig, path: &str) -> Option<&'a ScopeConfig> {
    scope_index(c, path).map(|i| &c.scopes[i])
}

/// The index in `c.scopes` of the scope that applies to `path`.
pub(crate) fn scope_index(c: &HealthConfig, path: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, scope) in c.scopes.iter().enumerate() {
        if !contains(&scope.path, path) {
            continue;
        }
        match best {
            Some(b) if c.scopes[b].path.len() >= scope.path.len() => {}
            _ => best = Some(i),
        }
    }
    best
}

fn contains(folder: &str, path: &str) -> bool {
    path == folder
        || path
            .strip_prefix(folder)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// `gitmaster.health.<var>`, with the variable name lowercased.
fn repo_key(key: &str) -> Option<String> {
    let (section, rest) = key.split_once('.')?;
    let (sub, var) = rest.split_once('.')?;
    (section.eq_ignore_ascii_case("gitmaster") && sub.eq_ignore_ascii_case("health"))
        .then(|| var.to_ascii_lowercase())
}

/// `gitmaster-health.<path>.<var>`: the path is everything between the first and the last dot.
fn scope_key(key: &str) -> Option<(&str, String)> {
    let (section, rest) = key.split_once('.')?;
    if !section.eq_ignore_ascii_case("gitmaster-health") {
        return None;
    }
    let (path, var) = rest.rsplit_once('.')?;
    Some((path, var.to_ascii_lowercase()))
}

fn apply_repo(config: &mut HealthConfig, var: &str, value: &str) {
    match var {
        "halflife" => set_f64(&mut config.half_life, value),
        "minhalflifedays" => set_f64(&mut config.min_half_life_days, value),
        "maxhalflifedays" => set_f64(&mut config.max_half_life_days, value),
        "dormancygap" => set_u32(&mut config.dormancy_gap, value),
        "windowdays" => set_u32(&mut config.window_days, value),
        "exclude" if !value.is_empty() => config.exclude.push(value.to_owned()),
        "ignorerev" if is_rev(value) => config.ignore_revs.push(value.to_ascii_lowercase()),
        _ => {}
    }
}

fn apply_scope(scopes: &mut BTreeMap<String, ScopeConfig>, raw: &str, var: &str, value: &str) {
    let path = raw.trim_matches('/');
    if path.is_empty() || path.contains(['*', '?', '[']) {
        return;
    }
    let entry = || ScopeConfig {
        path: path.to_owned(),
        half_life: None,
        dormancy_gap: None,
    };
    match var {
        "halflife" => {
            if let Some(v) = positive_f64(value) {
                scopes
                    .entry(path.to_owned())
                    .or_insert_with(entry)
                    .half_life = Some(v);
            }
        }
        "dormancygap" => {
            if let Some(v) = positive_u32(value) {
                scopes
                    .entry(path.to_owned())
                    .or_insert_with(entry)
                    .dormancy_gap = Some(v);
            }
        }
        _ => {}
    }
}

fn set_f64(slot: &mut f64, value: &str) {
    if let Some(v) = positive_f64(value) {
        *slot = v;
    }
}

fn set_u32(slot: &mut u32, value: &str) {
    if let Some(v) = positive_u32(value) {
        *slot = v;
    }
}

fn positive_f64(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

fn positive_u32(value: &str) -> Option<u32> {
    value.parse::<u32>().ok().filter(|v| *v > 0)
}

/// A full commit id: 40 (SHA-1) or 64 (SHA-256) hex digits.
fn is_rev(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_pattern(value: &str) -> bool {
    value.contains(['*', '?', '['])
}

/// Compiles a glob over the whole path. A folder glob (ending in `/`) matches
/// everything below it. `None` for an invalid glob.
fn glob_regex(glob: &str, folder: bool) -> Option<Regex> {
    let chars: Vec<char> = glob.chars().collect();
    let mut re = String::from("^");
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                if chars.get(i + 2) == Some(&'/') {
                    re.push_str("(?:.*/)?");
                    i += 3;
                } else {
                    re.push_str(".*");
                    i += 2;
                }
                continue;
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            '[' => {
                let end = i + 1 + chars[i + 1..].iter().position(|&c| c == ']')?;
                re.push('[');
                for (k, &c) in chars[i + 1..end].iter().enumerate() {
                    if k == 0 && c == '!' {
                        re.push('^');
                    } else if matches!(c, '\\' | '[' | '^') {
                        re.push('\\');
                        re.push(c);
                    } else {
                        re.push(c);
                    }
                }
                re.push(']');
                i = end;
            }
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    if folder {
        re.push_str(".*");
    }
    re.push('$');
    Regex::new(&re).ok()
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn entries(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn excluded(values: &[&str], path: &str) -> bool {
        let c = HealthConfig {
            exclude: values.iter().map(|v| (*v).to_owned()).collect(),
            ..HealthConfig::default()
        };
        Excluder::new(&c).excluded(path)
    }

    #[test]
    fn get_regexp_records_split_on_nul_and_keys_on_newline() {
        let bytes = b"gitmaster.health.halflife\n12\0gitmaster.health.exclude\nvendor/\0gitmaster.health.flag\0";
        assert_eq!(
            parse_get_regexp(bytes),
            vec![
                ("gitmaster.health.halflife".to_owned(), "12".to_owned()),
                ("gitmaster.health.exclude".to_owned(), "vendor/".to_owned()),
                ("gitmaster.health.flag".to_owned(), String::new()),
            ]
        );
    }

    #[test]
    fn get_regexp_keeps_newlines_after_the_first_in_the_value() {
        let bytes = b"gitmaster.health.exclude\na\nb\0";
        assert_eq!(
            parse_get_regexp(bytes),
            vec![("gitmaster.health.exclude".to_owned(), "a\nb".to_owned())]
        );
    }

    #[test]
    fn parses_every_repository_key() {
        let c = parse(&entries(&[
            ("gitmaster.health.halflife", "12.5"),
            ("gitmaster.health.minhalflifedays", "7"),
            ("gitmaster.health.maxhalflifedays", "200"),
            ("gitmaster.health.dormancygap", "90"),
            ("gitmaster.health.windowdays", "400"),
        ]));
        assert_eq!(c.half_life, 12.5);
        assert_eq!(c.min_half_life_days, 7.0);
        assert_eq!(c.max_half_life_days, 200.0);
        assert_eq!(c.dormancy_gap, 90);
        assert_eq!(c.window_days, 400);
    }

    #[test]
    fn keys_are_matched_case_insensitively() {
        let c = parse(&entries(&[
            ("GitMaster.Health.HalfLife", "20"),
            ("gitmaster.health.WindowDays", "100"),
        ]));
        assert_eq!(c.half_life, 20.0);
        assert_eq!(c.window_days, 100);
    }

    #[test]
    fn invalid_numbers_are_ignored_and_the_default_stays() {
        let c = parse(&entries(&[
            ("gitmaster.health.halflife", "abc"),
            ("gitmaster.health.minhalflifedays", "-3"),
            ("gitmaster.health.maxhalflifedays", "0"),
            ("gitmaster.health.dormancygap", "0"),
            ("gitmaster.health.windowdays", "1.5"),
        ]));
        let d = HealthConfig::default();
        assert_eq!(c.half_life, d.half_life);
        assert_eq!(c.min_half_life_days, d.min_half_life_days);
        assert_eq!(c.max_half_life_days, d.max_half_life_days);
        assert_eq!(c.dormancy_gap, d.dormancy_gap);
        assert_eq!(c.window_days, d.window_days);
    }

    #[test]
    fn non_finite_halflife_is_ignored() {
        for value in ["NaN", "inf", "-inf", "0", "-1"] {
            let c = parse(&entries(&[("gitmaster.health.halflife", value)]));
            assert_eq!(c.half_life, 30.0, "value {value}");
        }
    }

    #[test]
    fn later_valid_value_wins_and_invalid_later_value_does_not_overwrite() {
        let c = parse(&entries(&[
            ("gitmaster.health.halflife", "10"),
            ("gitmaster.health.halflife", "20"),
            ("gitmaster.health.halflife", "nope"),
        ]));
        assert_eq!(c.half_life, 20.0);
    }

    #[test]
    fn exclude_is_multi_valued_and_skips_empty() {
        let c = parse(&entries(&[
            ("gitmaster.health.exclude", "vendor/"),
            ("gitmaster.health.exclude", ""),
            ("gitmaster.health.exclude", "**/*.generated.ts"),
        ]));
        assert_eq!(c.exclude, vec!["vendor/", "**/*.generated.ts"]);
    }

    #[test]
    fn ignorerev_keeps_only_full_hex_ids_in_lowercase() {
        let sha1 = "4F3C2A1B9E8D7C6B5A4F3E2D1C0B9A8F7E6D5C4B";
        let sha256 = "a".repeat(64);
        let c = parse(&entries(&[
            ("gitmaster.health.ignorerev", sha1),
            ("gitmaster.health.ignorerev", &sha256),
            ("gitmaster.health.ignorerev", "abc123"),
            ("gitmaster.health.ignorerev", &"g".repeat(40)),
        ]));
        assert_eq!(c.ignore_revs, vec![sha1.to_ascii_lowercase(), sha256],);
    }

    #[test]
    fn scope_keys_merge_per_path_and_normalize_slashes() {
        let c = parse(&entries(&[
            ("gitmaster-health.services/billing.halflife", "15"),
            ("gitmaster-health./services/billing/.dormancygap", "80"),
            ("gitmaster-health.legacy/core.halflife", "60"),
        ]));
        assert_eq!(c.scopes.len(), 2);
        assert_eq!(c.scopes[0].path, "legacy/core");
        assert_eq!(c.scopes[0].half_life, Some(60.0));
        assert_eq!(c.scopes[0].dormancy_gap, None);
        assert_eq!(c.scopes[1].path, "services/billing");
        assert_eq!(c.scopes[1].half_life, Some(15.0));
        assert_eq!(c.scopes[1].dormancy_gap, Some(80));
    }

    #[test]
    fn scope_path_is_everything_between_first_and_last_dot() {
        let c = parse(&entries(&[("gitmaster-health.a.b.halflife", "9")]));
        assert_eq!(c.scopes.len(), 1);
        assert_eq!(c.scopes[0].path, "a.b");
    }

    #[test]
    fn scopes_with_empty_or_pattern_paths_are_ignored() {
        let c = parse(&entries(&[
            ("gitmaster-health..halflife", "9"),
            ("gitmaster-health./.halflife", "9"),
            ("gitmaster-health.src/*.halflife", "9"),
            ("gitmaster-health.src/a?.halflife", "9"),
            ("gitmaster-health.src/[ab].halflife", "9"),
        ]));
        assert!(c.scopes.is_empty());
    }

    #[test]
    fn a_scope_with_only_invalid_values_is_not_created() {
        let c = parse(&entries(&[
            ("gitmaster-health.services.halflife", "bad"),
            ("gitmaster-health.services.unknown", "5"),
        ]));
        assert!(c.scopes.is_empty());
    }

    #[test]
    fn scopes_are_sorted_by_path() {
        let c = parse(&entries(&[
            ("gitmaster-health.z.halflife", "1"),
            ("gitmaster-health.a.halflife", "2"),
            ("gitmaster-health.m.halflife", "3"),
        ]));
        let paths: Vec<&str> = c.scopes.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, ["a", "m", "z"]);
    }

    #[test]
    fn scope_values_are_last_wins() {
        let c = parse(&entries(&[
            ("gitmaster-health.src.halflife", "5"),
            ("gitmaster-health.src.halflife", "7"),
        ]));
        assert_eq!(c.scopes[0].half_life, Some(7.0));
    }

    #[test]
    fn config_hash_is_stable_and_eight_hex_digits() {
        let c = HealthConfig::default();
        let h = config_hash(&c);
        assert_eq!(h.len(), 8);
        assert!(
            h.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_eq!(h, config_hash(&c.clone()));
    }

    #[test]
    fn config_hash_ignores_order_of_lists_and_duplicates() {
        let a = HealthConfig {
            exclude: vec!["b/".into(), "a/".into(), "a/".into()],
            ..HealthConfig::default()
        };
        let b = HealthConfig {
            exclude: vec!["a/".into(), "b/".into()],
            ..HealthConfig::default()
        };
        assert_eq!(config_hash(&a), config_hash(&b));
    }

    #[test]
    fn config_hash_changes_with_any_value() {
        let base = HealthConfig::default();
        let hash = config_hash(&base);
        let changed = [
            HealthConfig {
                half_life: 31.0,
                ..base.clone()
            },
            HealthConfig {
                min_half_life_days: 15.0,
                ..base.clone()
            },
            HealthConfig {
                max_half_life_days: 300.0,
                ..base.clone()
            },
            HealthConfig {
                dormancy_gap: 121,
                ..base.clone()
            },
            HealthConfig {
                window_days: 1096,
                ..base.clone()
            },
            HealthConfig {
                exclude: vec!["x/".into()],
                ..base.clone()
            },
            HealthConfig {
                ignore_revs: vec!["a".repeat(40)],
                ..base.clone()
            },
            HealthConfig {
                scopes: vec![ScopeConfig {
                    path: "x".into(),
                    half_life: Some(5.0),
                    dormancy_gap: None,
                }],
                ..base.clone()
            },
        ];
        for c in &changed {
            assert_ne!(config_hash(c), hash, "{c:?}");
        }
    }

    #[test]
    fn lock_files_are_excluded_in_any_folder() {
        assert!(excluded(&[], "Cargo.lock"));
        assert!(excluded(&[], "web/app/package-lock.json"));
        assert!(excluded(&[], "a/b/packages.lock.json"));
        assert!(!excluded(&[], "Cargo.lock.bak"));
        assert!(!excluded(&[], "src/lock.rs"));
    }

    #[test]
    fn folder_value_is_a_prefix() {
        assert!(excluded(&["vendor/"], "vendor/x/a.rs"));
        assert!(!excluded(&["vendor/"], "vendors/a.rs"));
        assert!(!excluded(&["vendor/"], "src/vendor/a.rs"));
    }

    #[test]
    fn plain_value_equals_the_path() {
        assert!(excluded(&["src/gen.rs"], "src/gen.rs"));
        assert!(!excluded(&["src/gen.rs"], "src/gen.rs.bak"));
        assert!(!excluded(&["src/gen.rs"], "other/src/gen.rs"));
    }

    #[test]
    fn double_star_slash_matches_zero_or_more_folders() {
        assert!(excluded(&["**/*.generated.ts"], "a.generated.ts"));
        assert!(excluded(&["**/*.generated.ts"], "x/y/a.generated.ts"));
        assert!(!excluded(&["**/*.generated.ts"], "x/a.generated.tsx"));
        assert!(excluded(&["**/gen/"], "gen/x.rs"));
        assert!(excluded(&["**/gen/"], "a/gen/x.rs"));
    }

    #[test]
    fn single_star_stays_within_one_segment() {
        assert!(excluded(&["src/*.rs"], "src/a.rs"));
        assert!(!excluded(&["src/*.rs"], "src/x/a.rs"));
    }

    #[test]
    fn double_star_in_the_middle_matches_any_folders() {
        assert!(excluded(&["src/**/test"], "src/test"));
        assert!(excluded(&["src/**/test"], "src/a/b/test"));
        assert!(!excluded(&["src/**/test"], "lib/test"));
    }

    #[test]
    fn question_mark_matches_one_non_slash_character() {
        assert!(excluded(&["a?.txt"], "ab.txt"));
        assert!(!excluded(&["a?.txt"], "a/.txt"));
        assert!(!excluded(&["a?.txt"], "abc.txt"));
    }

    #[test]
    fn character_class_matches_one_listed_character() {
        assert!(excluded(&["[ab].txt"], "a.txt"));
        assert!(!excluded(&["[ab].txt"], "c.txt"));
        assert!(excluded(&["[!ab].txt"], "c.txt"));
        assert!(!excluded(&["[!ab].txt"], "a.txt"));
    }

    #[test]
    fn invalid_glob_is_ignored() {
        assert!(!excluded(&["src/[ab"], "src/a"));
        assert!(!excluded(&["src/[ab"], "src/[ab"));
    }

    #[test]
    fn scope_of_respects_folder_boundaries_and_longest_match() {
        let c = HealthConfig {
            scopes: vec![
                ScopeConfig {
                    path: "services".into(),
                    half_life: Some(1.0),
                    dormancy_gap: None,
                },
                ScopeConfig {
                    path: "services/billing".into(),
                    half_life: Some(2.0),
                    dormancy_gap: None,
                },
            ],
            ..HealthConfig::default()
        };
        assert_eq!(
            scope_of(&c, "services/billing/a.rs").unwrap().half_life,
            Some(2.0)
        );
        assert_eq!(
            scope_of(&c, "services/billing").unwrap().half_life,
            Some(2.0)
        );
        assert_eq!(
            scope_of(&c, "services/billing2/a.rs").unwrap().half_life,
            Some(1.0)
        );
        assert_eq!(scope_of(&c, "services/a.rs").unwrap().half_life, Some(1.0));
        assert!(scope_of(&c, "servicesx/a.rs").is_none());
        assert!(scope_of(&c, "other/a.rs").is_none());
    }

    #[test]
    fn read_gives_defaults_without_keys_and_values_with_keys() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        run(&["init", "-q"]);
        assert_eq!(read(dir.path()).unwrap(), HealthConfig::default());
        run(&["config", "gitmaster.health.halfLife", "12"]);
        run(&["config", "--add", "gitmaster.health.exclude", "vendor/"]);
        run(&["config", "--add", "gitmaster.health.exclude", "gen/"]);
        run(&["config", "gitmaster-health.services/billing.halfLife", "15"]);
        let c = read(dir.path()).unwrap();
        assert_eq!(c.half_life, 12.0);
        assert_eq!(c.exclude, vec!["vendor/", "gen/"]);
        assert_eq!(c.scopes[0].path, "services/billing");
        assert_eq!(c.scopes[0].half_life, Some(15.0));
    }

    #[test]
    fn read_reports_git_failure() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(read(&missing).is_err());
    }
}

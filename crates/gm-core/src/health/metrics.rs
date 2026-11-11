//! Code health: the indicators of each file (see mod.rs and docs/guide/code-health.md).
//!
//! `compute` is a pure function of the commits of the window and the configuration:
//! no I/O, no clock, fixed iteration order, so the same input gives the same output.
//! Files are interned to indexes once; every later step works on `Vec`s of indexes.

use std::collections::{HashMap, HashSet};

use gm_schema::{DormancyBreak, FileHealth, Ownership, Partner, SPARK_BARS, Trend};

use super::config::{Excluder, scope_index};
use super::{CommitRecord, HealthConfig};

/// The number of hottest files whose changes together are computed (D225 (3)).
const HOT_FILES: usize = 1000;
/// Commits with more counted files than this do not count for changes together.
const MAX_COMMIT_FILES: usize = 30;
const MIN_COMMIT_FILES: usize = 2;
const MAX_PARTNERS: usize = 5;
const MIN_SHARED: u32 = 5;
const MIN_DEGREE: f64 = 0.3;
/// Active days of the repository that count as recent for a dormancy break.
const RECENT_DAYS: u32 = 30;
const MIN_RECENT_CHANGES: usize = 3;
/// Repository active days per spark bar.
const SPARK_SPAN: u32 = 15;
const HOT_PERCENTILE: f64 = 95.0;
const MIN_OWNER_COMMITS: u32 = 5;

/// One commit as the metrics see it: only the files that still count.
struct Counted {
    day: i64,
    /// Index of the day in the repository's active days.
    repo_idx: usize,
    /// Index of the author in the first-seen order of the window.
    author: usize,
    /// Distinct file indexes, in the order of the commit's changes.
    files: Vec<usize>,
}

/// Interned paths. A path's index is its identity for the rest of the computation.
#[derive(Default)]
struct Paths<'a> {
    ids: HashMap<&'a str, usize>,
    names: Vec<&'a str>,
    /// Whether the path is left out (lock file, `exclude`).
    excluded: Vec<bool>,
    /// The last commit (1-based) that counted this path, to count a commit once per file.
    stamp: Vec<usize>,
}

impl<'a> Paths<'a> {
    fn intern(&mut self, path: &'a str, excluder: &Excluder) -> usize {
        if let Some(&id) = self.ids.get(path) {
            return id;
        }
        let id = self.names.len();
        self.ids.insert(path, id);
        self.names.push(path);
        self.excluded.push(excluder.excluded(path));
        self.stamp.push(0);
        id
    }
}

/// The indicators of a file before ranking.
struct Scored {
    id: usize,
    heat: f64,
    trend: Trend,
    spark: [u32; SPARK_BARS],
    dormancy: Option<DormancyBreak>,
    ownership: Ownership,
    commits: u32,
}

/// The active days of one clock: the repository, or one scope.
struct Clock {
    days: Vec<i64>,
    /// The effective half-life in active days of this clock (D225 (5), docs).
    half_life: f64,
}

impl Clock {
    /// The active days of this clock strictly after `day`, which must be one of them.
    fn age(&self, day: i64) -> u32 {
        let idx = match self.days.binary_search(&day) {
            Ok(i) | Err(i) => i,
        };
        to_u32(self.days.len().saturating_sub(idx + 1))
    }
}

/// Computes the health of every file changed in `commits`, hottest first.
///
/// `commits` are the commits of the window, newest first. With `live`, only the
/// files in that set are reported, but every file still counts for clocks and
/// changes together. Ranks and percentiles are those of the reported files.
pub fn compute<S: std::hash::BuildHasher>(
    commits: &[CommitRecord],
    config: &HealthConfig,
    live: Option<&HashSet<String, S>>,
) -> Vec<FileHealth> {
    let (names, mut counted) = gather(commits, config);
    let repo_days = assign_repo_days(&mut counted);
    let file_scope: Vec<Option<usize>> = names
        .names
        .iter()
        .map(|path| scope_index(config, path))
        .collect();
    let clocks = build_clocks(&counted, &repo_days, &file_scope, config);

    let mut file_commits: Vec<Vec<usize>> = vec![Vec::new(); names.names.len()];
    for (ci, commit) in counted.iter().enumerate() {
        for &f in &commit.files {
            file_commits[f].push(ci);
        }
    }

    let mut scored: Vec<Scored> = Vec::new();
    for (f, list) in file_commits.iter().enumerate() {
        if list.is_empty() {
            continue;
        }
        let (clock, gap) = match file_scope[f] {
            Some(s) => (
                &clocks[s + 1],
                config.scopes[s].dormancy_gap.unwrap_or(config.dormancy_gap),
            ),
            None => (&clocks[0], config.dormancy_gap),
        };
        scored.push(score_file(f, list, &counted, clock, &clocks[0], gap));
    }
    scored.sort_by(|a, b| {
        b.heat
            .total_cmp(&a.heat)
            .then_with(|| names.names[a.id].cmp(names.names[b.id]))
    });

    let mut hot = vec![false; names.names.len()];
    for s in scored.iter().take(HOT_FILES) {
        hot[s.id] = true;
    }
    let mut partners = changes_together(&counted, &hot, &names.names);

    let reported: Vec<&Scored> = scored
        .iter()
        .filter(|s| match live {
            None => true,
            Some(set) => set.contains(names.names[s.id]),
        })
        .collect();
    let of = to_u32(reported.len());
    reported
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let rank = to_u32(i + 1);
            let percentile = round_to(100.0 * f64::from(of - rank) / f64::from(of), 1);
            let changes_together = std::mem::take(&mut partners[s.id]);
            let path = names.names[s.id].to_owned();
            let constraints = constraints(
                rank,
                of,
                percentile,
                s.dormancy.as_ref(),
                &changes_together,
                &s.ownership,
                s.commits,
            );
            FileHealth {
                path,
                rank,
                percentile,
                heat: s.heat,
                trend: s.trend,
                spark: s.spark,
                dormancy_break: s.dormancy.clone(),
                changes_together,
                ownership: s.ownership.clone(),
                constraints,
            }
        })
        .collect()
}

/// Drops ignored commits, follows renames and keeps the changes that count.
fn gather<'a>(commits: &'a [CommitRecord], config: &HealthConfig) -> (Paths<'a>, Vec<Counted>) {
    let ignored: HashSet<&str> = config.ignore_revs.iter().map(String::as_str).collect();
    let excluder = Excluder::new(config);
    let mut names = Paths::default();
    let mut alias: HashMap<usize, usize> = HashMap::new();
    let mut authors: HashMap<&'a str, usize> = HashMap::new();
    let mut counted = Vec::new();

    for (n, commit) in commits.iter().enumerate() {
        if ignored.contains(commit.sha.as_str()) {
            continue;
        }
        let stamp = n + 1;
        let mut files = Vec::new();
        for change in &commit.changes {
            let path = names.intern(change.path.as_str(), &excluder);
            let current = alias.get(&path).copied().unwrap_or(path);
            if let Some(old) = change.old_path.as_deref().filter(|old| *old != change.path) {
                let old_id = names.intern(old, &excluder);
                alias.insert(old_id, current);
            }
            if names.excluded[current] || names.stamp[current] == stamp {
                continue;
            }
            names.stamp[current] = stamp;
            files.push(current);
        }
        if files.is_empty() {
            continue;
        }
        let next = authors.len();
        let author = *authors.entry(commit.author.as_str()).or_insert(next);
        counted.push(Counted {
            day: commit.day(),
            repo_idx: 0,
            author,
            files,
        });
    }
    (names, counted)
}

/// Sets each commit's index among the repository's active days; returns those days.
fn assign_repo_days(counted: &mut [Counted]) -> Vec<i64> {
    let mut days: Vec<i64> = counted.iter().map(|c| c.day).collect();
    days.sort_unstable();
    days.dedup();
    for commit in counted.iter_mut() {
        commit.repo_idx = days.binary_search(&commit.day).unwrap_or_else(|i| i);
    }
    days
}

/// Clock 0 is the repository; clock `s + 1` is scope `s` of the configuration.
fn build_clocks(
    counted: &[Counted],
    repo_days: &[i64],
    file_scope: &[Option<usize>],
    config: &HealthConfig,
) -> Vec<Clock> {
    let mut scope_days: Vec<Vec<i64>> = vec![Vec::new(); config.scopes.len()];
    for commit in counted {
        for &f in &commit.files {
            if let Some(s) = file_scope[f] {
                scope_days[s].push(commit.day);
            }
        }
    }
    let mut clocks = vec![Clock {
        days: repo_days.to_vec(),
        half_life: effective_half_life(repo_days, config.half_life, config),
    }];
    for (s, mut days) in scope_days.into_iter().enumerate() {
        days.sort_unstable();
        days.dedup();
        let h = config.scopes[s].half_life.unwrap_or(config.half_life);
        let half_life = effective_half_life(&days, h, config);
        clocks.push(Clock { days, half_life });
    }
    clocks
}

/// The half-life in active days that stands for a length in calendar days inside the
/// configured bounds (docs/guide/code-health.md#heat-in-detail).
fn effective_half_life(days: &[i64], h: f64, config: &HealthConfig) -> f64 {
    let (Some(first), Some(last)) = (days.first(), days.last()) else {
        return h;
    };
    let span = u32::try_from(last - first + 1).unwrap_or(u32::MAX);
    let rate = f64::from(to_u32(days.len())) / f64::from(span);
    if rate <= 0.0 {
        return h;
    }
    let wall = h / rate;
    let (lo, hi) = (config.min_half_life_days, config.max_half_life_days);
    let wall = if lo <= hi { wall.clamp(lo, hi) } else { wall };
    wall * rate
}

/// The indicators of one file, from its commits (indexes into `counted`, newest first).
fn score_file(
    id: usize,
    commits: &[usize],
    counted: &[Counted],
    clock: &Clock,
    repo: &Clock,
    dormancy_gap: u32,
) -> Scored {
    let h = clock.half_life;
    let repo_active = to_u32(repo.days.len());
    let mut heat = 0.0;
    let (mut recent, mut previous) = (0, 0);
    let mut spark = [0u32; SPARK_BARS];
    let mut recent_days: Vec<usize> = Vec::new();
    let mut authors = Vec::with_capacity(commits.len());

    for &ci in commits {
        let commit = &counted[ci];
        let age = f64::from(clock.age(commit.day));
        heat += 2f64.powf(-age / h);
        if age < h {
            recent += 1;
        } else if age < 2.0 * h {
            previous += 1;
        }

        let repo_age = repo_active.saturating_sub(1 + to_u32(commit.repo_idx));
        let bar = usize::try_from(repo_age / SPARK_SPAN).unwrap_or(usize::MAX);
        if bar < SPARK_BARS {
            spark[SPARK_BARS - 1 - bar] += 1;
        }
        if repo_age < RECENT_DAYS {
            recent_days.push(commit.repo_idx);
        }
        authors.push(commit.author);
    }

    recent_days.sort_unstable();
    recent_days.dedup();
    let dormancy = dormancy_break(commits, counted, &recent_days, dormancy_gap);
    Scored {
        id,
        heat,
        trend: trend(recent, previous),
        spark,
        dormancy,
        ownership: ownership(&mut authors),
        commits: to_u32(commits.len()),
    }
}

/// A file quiet for `dormancy_gap` active days of the repository that changes again.
/// `recent_days` are the repository's active days in the last 30 on which it changed.
fn dormancy_break(
    commits: &[usize],
    counted: &[Counted],
    recent_days: &[usize],
    dormancy_gap: u32,
) -> Option<DormancyBreak> {
    if recent_days.len() < MIN_RECENT_CHANGES {
        return None;
    }
    let first = *recent_days.first()?;
    let prev = commits
        .iter()
        .map(|&ci| counted[ci].repo_idx)
        .filter(|&idx| idx < first)
        .max()?;
    let gap = to_u32(first - prev - 1);
    (gap >= dormancy_gap).then(|| DormancyBreak {
        quiet_active_days: gap,
        recent_active_days: to_u32(recent_days.len()),
    })
}

/// Rising when the recent half-life has at least 1.5 times the commits of the one before it.
fn trend(recent: u32, previous: u32) -> Trend {
    if previous == 0 {
        return if recent >= 2 {
            Trend::Rising
        } else {
            Trend::Steady
        };
    }
    let (r, p) = (u64::from(recent), u64::from(previous));
    if r * 2 >= p * 3 {
        Trend::Rising
    } else if r * 3 <= p * 2 {
        Trend::Falling
    } else {
        Trend::Steady
    }
}

/// Authors, the share of the most frequent one and the authors with under 5% of commits.
fn ownership(authors: &mut [usize]) -> Ownership {
    authors.sort_unstable();
    let counts: Vec<u32> = authors
        .chunk_by(|a, b| a == b)
        .map(|run| to_u32(run.len()))
        .collect();
    let total = to_u32(authors.len());
    let top = counts.iter().copied().max().unwrap_or(0);
    let minor = counts
        .iter()
        .filter(|&&count| u64::from(count) * 20 < u64::from(total))
        .count();
    Ownership {
        authors: to_u32(counts.len()),
        top_author_share: round_to(f64::from(top) / f64::from(total), 2),
        minor_contributors: to_u32(minor),
    }
}

/// Changes together: pairs of files in the same commits, where at least one file is hot.
/// Returns, per file index, its partners, strongest first.
fn changes_together(counted: &[Counted], hot: &[bool], names: &[&str]) -> Vec<Vec<Partner>> {
    let mut commit_total = vec![0u32; names.len()];
    let mut shared: HashMap<(usize, usize), u32> = HashMap::new();
    for commit in counted {
        let k = commit.files.len();
        if !(MIN_COMMIT_FILES..=MAX_COMMIT_FILES).contains(&k) {
            continue;
        }
        for &f in &commit.files {
            commit_total[f] += 1;
        }
        for (i, &a) in commit.files.iter().enumerate() {
            for &b in &commit.files[i + 1..] {
                if hot[a] || hot[b] {
                    *shared.entry((a.min(b), a.max(b))).or_insert(0) += 1;
                }
            }
        }
    }

    let mut pairs: Vec<((usize, usize), u32)> = shared.into_iter().collect();
    pairs.sort_unstable();
    let mut lists: Vec<Vec<(f64, u32, usize)>> = vec![Vec::new(); names.len()];
    for ((a, b), count) in pairs {
        if count < MIN_SHARED {
            continue;
        }
        let mean = f64::midpoint(f64::from(commit_total[a]), f64::from(commit_total[b]));
        let degree = f64::from(count) / mean;
        if degree < MIN_DEGREE {
            continue;
        }
        lists[a].push((degree, count, b));
        lists[b].push((degree, count, a));
    }

    lists
        .into_iter()
        .map(|mut list| {
            list.sort_by(|x, y| {
                y.0.total_cmp(&x.0)
                    .then(y.1.cmp(&x.1))
                    .then_with(|| names[x.2].cmp(names[y.2]))
            });
            list.truncate(MAX_PARTNERS);
            list.into_iter()
                .map(|(degree, count, other)| Partner {
                    path: names[other].to_owned(),
                    degree: round_to(degree, 2),
                    shared_commits: count,
                })
                .collect()
        })
        .collect()
}

/// The fixed rules of docs/guide/code-health.md, in order, over the reported values.
fn constraints(
    rank: u32,
    of: u32,
    percentile: f64,
    dormancy: Option<&DormancyBreak>,
    partners: &[Partner],
    ownership: &Ownership,
    commits: u32,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(d) = dormancy {
        out.push(format!(
            "Dormant for {} active days, now changing on {} of the last 30: explain the current behavior of the file back before changing it, and keep the change small.",
            d.quiet_active_days, d.recent_active_days
        ));
    }
    if percentile >= HOT_PERCENTILE {
        out.push(format!(
            "Hot file (rank {rank} of {of}): keep the diff small enough for one reviewer."
        ));
    }
    for partner in partners.iter().filter(|p| p.degree >= 0.5) {
        out.push(format!(
            "Changes together with {} (degree {:.2}): review the change together with that file.",
            partner.path, partner.degree
        ));
    }
    if commits >= MIN_OWNER_COMMITS
        && (ownership.top_author_share < 0.2
            || (ownership.minor_contributors >= 5 && ownership.top_author_share < 0.5))
    {
        out.push(format!(
            "No clear owner (top author {:.0}% of commits, {} occasional authors): keep the diff small enough for one reviewer, and explain any change of behavior.",
            ownership.top_author_share * 100.0,
            ownership.minor_contributors
        ));
    }
    out
}

fn round_to(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    /// No `live` filter: every file is reported.
    const NO_LIVE: Option<&HashSet<String>> = None;

    use proptest::prelude::*;

    use super::*;
    use crate::health::{DAY_SECS, FileChange};

    fn change(path: &str) -> FileChange {
        FileChange {
            path: path.to_owned(),
            old_path: None,
            added: Some(1),
            deleted: Some(1),
        }
    }

    /// Commits from `(day, author, paths)`, the sha being the position in the input.
    /// Returned newest first.
    fn log<A: AsRef<str>, S: AsRef<str>>(entries: &[(i64, A, Vec<S>)]) -> Vec<CommitRecord> {
        let mut commits: Vec<CommitRecord> = entries
            .iter()
            .enumerate()
            .map(|(n, (day, author, paths))| CommitRecord {
                sha: format!("{n:040x}"),
                time: day * DAY_SECS,
                author: author.as_ref().to_owned(),
                parents: 1,
                changes: paths.iter().map(|p| change(p.as_ref())).collect(),
            })
            .collect();
        commits.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| a.sha.cmp(&b.sha)));
        commits
    }

    fn sha(n: usize) -> String {
        format!("{n:040x}")
    }

    fn file<'a>(out: &'a [FileHealth], path: &str) -> &'a FileHealth {
        out.iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("no file {path} in {out:?}"))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// A file `filler.rs` changed on every day in `0..days`, plus `extra` on the given days.
    fn steady_with(days: i64, extra: &[(i64, &str)]) -> Vec<CommitRecord> {
        let entries: Vec<(i64, &str, Vec<&str>)> = (0..days)
            .map(|d| {
                let mut paths = vec!["filler.rs"];
                paths.extend(extra.iter().filter(|(e, _)| *e == d).map(|(_, p)| *p));
                (d, "dev", paths)
            })
            .collect();
        log(&entries)
    }

    fn compute_default(commits: &[CommitRecord]) -> Vec<FileHealth> {
        compute(commits, &HealthConfig::default(), NO_LIVE)
    }

    #[test]
    fn heat_halves_after_half_life_in_active_days() {
        let entries: Vec<(i64, &str, Vec<&str>)> = (0..=30)
            .map(|d| {
                let mut paths = vec!["filler.rs"];
                if d == 0 || d == 30 {
                    paths.push("a.rs");
                }
                (d, "dev", paths)
            })
            .collect();
        let out = compute_default(&log(&entries));
        // The change on day 30 weighs 1, the one on day 0 (30 active days earlier) 0.5.
        assert!(
            close(file(&out, "a.rs").heat, 1.5),
            "{}",
            file(&out, "a.rs").heat
        );
    }

    #[test]
    fn a_rename_carries_history_to_the_new_name() {
        let commits = vec![
            CommitRecord {
                sha: sha(2),
                time: 2 * DAY_SECS,
                author: "dev".into(),
                parents: 1,
                changes: vec![FileChange {
                    path: "new.rs".into(),
                    old_path: Some("old.rs".into()),
                    added: Some(0),
                    deleted: Some(0),
                }],
            },
            CommitRecord {
                sha: sha(1),
                time: DAY_SECS,
                author: "dev".into(),
                parents: 1,
                changes: vec![change("old.rs")],
            },
            CommitRecord {
                sha: sha(0),
                time: 0,
                author: "dev".into(),
                parents: 1,
                changes: vec![change("old.rs")],
            },
        ];
        let out = compute_default(&commits);
        assert!(out.iter().all(|f| f.path != "old.rs"));
        assert_eq!(file(&out, "new.rs").spark.iter().sum::<u32>(), 3);
    }

    #[test]
    fn ignored_commits_and_excluded_paths_drop_their_work() {
        let entries = vec![
            (0, "dev", vec!["a.rs", "vendor/v.rs", "Cargo.lock"]),
            (1, "dev", vec!["a.rs"]),
        ];
        let commits = log(&entries);
        let config = HealthConfig {
            ignore_revs: vec![sha(1)],
            exclude: vec!["vendor/".into()],
            ..HealthConfig::default()
        };
        let out = compute(&commits, &config, NO_LIVE);
        let paths: Vec<&str> = out.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.rs"]);
        assert_eq!(file(&out, "a.rs").spark.iter().sum::<u32>(), 1);
    }

    #[test]
    fn a_commit_that_loses_every_change_counts_nowhere() {
        let entries = vec![(0, "dev", vec!["a.rs"]), (5, "dev", vec!["Cargo.lock"])];
        let out = compute_default(&log(&entries));
        assert_eq!(file(&out, "a.rs").spark.iter().sum::<u32>(), 1);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn trend_rising_steady_and_falling() {
        let trend_of = |x_days: &[i64]| {
            let entries: Vec<(i64, &str, Vec<&str>)> = (0..60)
                .map(|d| {
                    let mut paths = vec!["filler.rs"];
                    if x_days.contains(&d) {
                        paths.push("x.rs");
                    }
                    (d, "dev", paths)
                })
                .collect();
            file(&compute_default(&log(&entries)), "x.rs").trend
        };
        // Recent: ages below 30. Previous: ages from 30 to 59.
        assert_eq!(trend_of(&[59, 58, 57, 56, 19]), Trend::Rising);
        assert_eq!(trend_of(&[59, 58, 19, 10]), Trend::Steady);
        assert_eq!(trend_of(&[59, 19, 10]), Trend::Falling);
        assert_eq!(trend_of(&[59, 58]), Trend::Rising);
        assert_eq!(trend_of(&[59]), Trend::Steady);
    }

    #[test]
    fn spark_bars_count_repository_ages_oldest_first() {
        let commits = steady_with(
            181,
            &[(180, "x.rs"), (165, "x.rs"), (1, "x.rs"), (0, "x.rs")],
        );
        let out = compute_default(&commits);
        let spark = file(&out, "x.rs").spark;
        assert_eq!(spark[11], 1, "age 0");
        assert_eq!(spark[10], 1, "age 15");
        assert_eq!(spark[0], 1, "age 179");
        assert_eq!(
            spark.iter().sum::<u32>(),
            3,
            "age 180 is older than the bars"
        );
    }

    #[test]
    fn dormancy_break_is_detected_and_a_new_file_is_not_flagged() {
        let mut extra = vec![(50, "x.rs"), (190, "x.rs"), (191, "x.rs"), (192, "x.rs")];
        extra.extend([(190, "y.rs"), (191, "y.rs"), (192, "y.rs")]);
        let out = compute_default(&steady_with(201, &extra));
        let x = file(&out, "x.rs").dormancy_break.clone();
        assert_eq!(
            x,
            Some(DormancyBreak {
                quiet_active_days: 139,
                recent_active_days: 3,
            })
        );
        assert!(file(&out, "x.rs").constraints.iter().any(|c| c == "Dormant for 139 active days, now changing on 3 of the last 30: explain the current behavior of the file back before changing it, and keep the change small."));
        assert_eq!(file(&out, "y.rs").dormancy_break, None);
    }

    #[test]
    fn dormancy_gap_of_a_scope_applies_to_its_files() {
        let mut extra = vec![
            (50, "legacy/x.rs"),
            (190, "legacy/x.rs"),
            (191, "legacy/x.rs"),
            (192, "legacy/x.rs"),
        ];
        extra.extend([(0, "legacy/y.rs")]);
        let config = HealthConfig {
            scopes: vec![crate::health::ScopeConfig {
                path: "legacy".into(),
                half_life: None,
                dormancy_gap: Some(200),
            }],
            ..HealthConfig::default()
        };
        let out = compute(&steady_with(201, &extra), &config, NO_LIVE);
        assert_eq!(file(&out, "legacy/x.rs").dormancy_break, None);
    }

    #[test]
    fn coupling_needs_five_shared_commits_and_degree() {
        let five: Vec<(i64, &str, Vec<&str>)> =
            (0..5).map(|d| (d, "dev", vec!["a.rs", "b.rs"])).collect();
        let out = compute_default(&log(&five));
        let partners = &file(&out, "a.rs").changes_together;
        assert_eq!(partners.len(), 1);
        assert_eq!(partners[0].path, "b.rs");
        assert!(close(partners[0].degree, 1.0));
        assert_eq!(partners[0].shared_commits, 5);
        assert!(file(&out, "a.rs").constraints.iter().any(|c| c == "Changes together with b.rs (degree 1.00): review the change together with that file."));

        let four: Vec<(i64, &str, Vec<&str>)> =
            (0..4).map(|d| (d, "dev", vec!["a.rs", "b.rs"])).collect();
        let out = compute_default(&log(&four));
        assert!(file(&out, "a.rs").changes_together.is_empty());
    }

    #[test]
    fn coupling_skips_commits_with_more_than_thirty_files() {
        let wide = |n: usize| -> Vec<String> {
            let mut paths = vec!["a.rs".to_owned(), "b.rs".to_owned()];
            paths.extend((0..n - 2).map(|k| format!("p{k:02}.rs")));
            paths
        };
        let at_limit: Vec<(i64, &str, Vec<String>)> =
            (0..5).map(|d| (d, "dev", wide(30))).collect();
        let out = compute_default(&log(&at_limit));
        assert!(
            file(&out, "a.rs")
                .changes_together
                .iter()
                .any(|p| p.path == "b.rs")
        );

        let over: Vec<(i64, &str, Vec<String>)> = (0..5).map(|d| (d, "dev", wide(31))).collect();
        let out = compute_default(&log(&over));
        assert!(file(&out, "a.rs").changes_together.is_empty());
    }

    #[test]
    fn coupling_lists_at_most_five_partners_strongest_first() {
        let mut paths = vec!["a.rs"];
        let names = ["b1.rs", "b2.rs", "b3.rs", "b4.rs", "b5.rs", "b6.rs"];
        paths.extend(names);
        let entries: Vec<(i64, &str, Vec<&str>)> =
            (0..5).map(|d| (d, "dev", paths.clone())).collect();
        let out = compute_default(&log(&entries));
        let partners: Vec<&str> = file(&out, "a.rs")
            .changes_together
            .iter()
            .map(|p| p.path.as_str())
            .collect();
        assert_eq!(partners, ["b1.rs", "b2.rs", "b3.rs", "b4.rs", "b5.rs"]);
    }

    #[test]
    fn ownership_counts_and_shares() {
        let mut entries: Vec<(i64, &str, Vec<&str>)> =
            (0..30).map(|d| (d, "a", vec!["o.rs"])).collect();
        let minor = ["u1", "u2", "u3", "u4", "u5", "u6", "u7", "u8", "u9"];
        entries.extend((30i64..).zip(minor).map(|(d, u)| (d, u, vec!["o.rs"])));
        let out = compute_default(&log(&entries));
        let o = &file(&out, "o.rs").ownership;
        assert_eq!(o.authors, 10);
        assert!(close(o.top_author_share, 0.77));
        assert_eq!(o.minor_contributors, 9);
    }

    #[test]
    fn constraints_texts_for_ownership_and_hot_files() {
        let mut entries: Vec<(i64, String, Vec<String>)> = Vec::new();
        for d in 0..10 {
            entries.push((d, format!("dev{d}"), vec!["own.rs".to_owned()]));
        }
        for d in 0..12 {
            let mut paths = vec!["hot.rs".to_owned()];
            if d == 11 {
                paths.extend((0..19).map(|k| format!("f{k:02}.rs")));
            }
            entries.push((100 + d, "hotdev".to_owned(), paths));
        }
        let out = compute_default(&log(&entries));
        let hot = file(&out, "hot.rs");
        assert_eq!(hot.rank, 1);
        assert!(hot.constraints.contains(
            &"Hot file (rank 1 of 21): keep the diff small enough for one reviewer.".to_owned()
        ));
        let own = file(&out, "own.rs");
        assert!(own.constraints.contains(&"No clear owner (top author 10% of commits, 0 occasional authors): keep the diff small enough for one reviewer, and explain any change of behavior.".to_owned()));
    }

    #[test]
    fn a_file_with_a_main_author_has_an_owner_despite_occasional_authors() {
        let mut entries: Vec<(i64, String, Vec<String>)> = Vec::new();
        for d in 0..90 {
            entries.push((d, "main".to_owned(), vec!["kept.rs".to_owned()]));
        }
        for d in 0..6 {
            entries.push((100 + d, format!("guest{d}"), vec!["kept.rs".to_owned()]));
        }
        let out = compute_default(&log(&entries));
        let kept = file(&out, "kept.rs");
        assert_eq!(kept.ownership.minor_contributors, 6);
        assert!(
            !kept
                .constraints
                .iter()
                .any(|c| c.starts_with("No clear owner"))
        );
    }

    #[test]
    fn hot_constraint_needs_the_top_five_percent() {
        let mut entries: Vec<(i64, &str, Vec<String>)> =
            vec![(0, "dev", vec!["hot.rs".to_owned()])];
        entries.push((1, "dev", vec!["hot.rs".to_owned()]));
        for k in 0..19 {
            entries.push((2, "dev", vec![format!("f{k:02}.rs")]));
        }
        let out = compute_default(&log(&entries));
        assert_eq!(file(&out, "hot.rs").rank, 1);
        assert!(
            file(&out, "hot.rs")
                .constraints
                .iter()
                .any(|c| c.starts_with("Hot file (rank 1 of 20)"))
        );
        assert!(
            file(&out, "f00.rs")
                .constraints
                .iter()
                .all(|c| !c.starts_with("Hot file"))
        );
    }

    #[test]
    fn live_filter_reports_only_live_files_but_counts_all_of_them() {
        let entries = vec![
            (0, "dev", vec!["a.rs", "b.rs"]),
            (1, "dev", vec!["b.rs"]),
            (2, "dev", vec!["a.rs"]),
        ];
        let commits = log(&entries);
        let all = compute_default(&commits);
        let live: HashSet<String> = ["a.rs".to_owned()].into_iter().collect();
        let out = compute(&commits, &HealthConfig::default(), Some(&live));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "a.rs");
        assert_eq!(out[0].rank, 1);
        assert!(close(out[0].percentile, 0.0));
        assert!(close(out[0].heat, file(&all, "a.rs").heat));
        assert_eq!(out[0].changes_together, file(&all, "a.rs").changes_together);
    }

    #[test]
    fn ranks_form_one_to_of_and_ties_break_by_path() {
        let entries = vec![(0, "dev", vec!["b.rs"]), (0, "dev", vec!["a.rs"])];
        let out = compute_default(&log(&entries));
        let order: Vec<(&str, u32)> = out.iter().map(|f| (f.path.as_str(), f.rank)).collect();
        assert_eq!(order, [("a.rs", 1), ("b.rs", 2)]);
        assert!(close(out[0].percentile, 50.0));
        assert!(close(out[1].percentile, 0.0));
    }

    fn arb_commits() -> impl Strategy<Value = Vec<CommitRecord>> {
        proptest::collection::vec(
            (
                0i64..200,
                0usize..4,
                proptest::collection::vec(0usize..8, 0..5),
            ),
            0..40,
        )
        .prop_map(|items| {
            let entries: Vec<(i64, String, Vec<String>)> = items
                .into_iter()
                .map(|(day, author, paths)| {
                    let mut unique: Vec<usize> = paths;
                    unique.sort_unstable();
                    unique.dedup();
                    (
                        day,
                        format!("dev{author}"),
                        unique.into_iter().map(|k| format!("p{k}.rs")).collect(),
                    )
                })
                .collect();
            log(&entries)
        })
    }

    proptest! {
        #[test]
        fn compute_is_deterministic(commits in arb_commits()) {
            let config = HealthConfig::default();
            prop_assert_eq!(compute(&commits, &config, NO_LIVE), compute(&commits, &config, NO_LIVE));
        }

        #[test]
        fn percentiles_are_within_zero_and_hundred(commits in arb_commits()) {
            for f in compute_default(&commits) {
                prop_assert!((0.0..=100.0).contains(&f.percentile), "{}", f.percentile);
            }
        }

        #[test]
        fn no_file_is_its_own_partner(commits in arb_commits()) {
            for f in compute_default(&commits) {
                prop_assert!(f.changes_together.iter().all(|p| p.path != f.path));
            }
        }

        #[test]
        fn spark_never_exceeds_the_file_commits(commits in arb_commits()) {
            let mut counts: HashMap<&str, u32> = HashMap::new();
            for commit in &commits {
                for ch in &commit.changes {
                    *counts.entry(ch.path.as_str()).or_insert(0) += 1;
                }
            }
            for f in compute_default(&commits) {
                let total: u32 = f.spark.iter().sum();
                prop_assert!(total <= counts[f.path.as_str()]);
            }
        }
    }
}

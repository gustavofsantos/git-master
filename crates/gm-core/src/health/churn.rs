//! Code health: following each line of each file along the first-parent history (D228,
//! docs/guide/code-health.md, "How lines are followed"). The replay is a pure function of the
//! hunk headers of the window and the configuration: no I/O, no clock.
//!
//! Each file keeps the known prefix of its lines. A line is `Born` on the active day of the
//! counted commit that added it, or `Old` when it was there before the window or was added by
//! an ignored commit. Every line past the end of the prefix is `Old`. A line's age at its death
//! or today is measured in active days of the repository: the index of its day among the
//! distinct days of the commits.

use std::collections::{HashMap, HashSet};

use gm_schema::{CHURN_DAYS, LineChurn, age_bucket};

use super::config::Excluder;
use super::{FirstParentCommit, HealthConfig, Hunk};

/// One line as the replay knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    /// Older than the window, or added by an ignored commit: never counted.
    Old,
    /// Added by a counted commit on the active day with this index.
    Born(u32),
}

/// One file as the replay follows it: its known lines and what they did. A rename moves all of it.
#[derive(Default)]
struct Tracked {
    /// The known prefix of the file, top to bottom.
    lines: Vec<Line>,
    churn: LineChurn,
    /// Whether a counted line was born or died in the file. A file that never was has no entry.
    touched: bool,
}

/// Interned paths: each one's id is its index in `names`, `excluded` and `files`.
#[derive(Default)]
struct Paths<'a> {
    ids: HashMap<&'a str, usize>,
    names: Vec<&'a str>,
    excluded: Vec<bool>,
    files: Vec<Tracked>,
}

impl<'a> Paths<'a> {
    fn id(&mut self, path: &'a str, excluder: &Excluder) -> usize {
        if let Some(&id) = self.ids.get(path) {
            return id;
        }
        let id = self.names.len();
        self.ids.insert(path, id);
        self.names.push(path);
        self.excluded.push(excluder.excluded(path));
        self.files.push(Tracked::default());
        id
    }
}

/// Follows the lines of every file over `commits` (oldest first) and returns the churn of each
/// file that had a line added in the window, keyed by its path at the last commit.
///
/// `days` is the UTC day of each commit, in the same order. The active days are the distinct
/// days of the commits; "now" is the last of them. A commit of `config.ignore_revs` moves lines
/// without counting them. A path the `exclude` setting leaves out gets no entry.
#[must_use]
pub fn follow(
    commits: &[FirstParentCommit],
    days: &[i64],
    config: &HealthConfig,
) -> HashMap<String, LineChurn> {
    let n = commits.len().min(days.len());
    let (commits, days) = (&commits[..n], &days[..n]);
    let mut distinct = days.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    let now = to_u32(distinct.len().saturating_sub(1));
    let day_index: Vec<u32> = days
        .iter()
        .map(|day| to_u32(distinct.binary_search(day).unwrap_or_else(|i| i)))
        .collect();

    let ignored: HashSet<&str> = config.ignore_revs.iter().map(String::as_str).collect();
    let excluder = Excluder::new(config);
    let mut paths = Paths::default();
    for (ci, commit) in commits.iter().enumerate() {
        let at = day_index[ci];
        let counted = !ignored.contains(commit.sha.as_str());

        // A rename moves the file's lines and history. Every move of the commit takes its
        // source first, so a chain or a swap of names cannot overwrite a file still to move.
        let mut renames: Vec<(usize, usize)> = commit
            .hunks
            .iter()
            .filter_map(|hunk| {
                let old = hunk.old_path.as_deref()?;
                Some((paths.id(old, &excluder), paths.id(&hunk.path, &excluder)))
            })
            .collect();
        renames.sort_unstable();
        renames.dedup();
        let taken: Vec<(usize, Tracked)> = renames
            .iter()
            .map(|&(from, to)| (to, std::mem::take(&mut paths.files[from])))
            .collect();
        for (to, file) in taken {
            paths.files[to] = file;
        }

        // A binary side forgets the file's known lines: they leave the count without a death.
        for hunk in &commit.hunks {
            if is_marker(hunk) {
                let id = paths.id(&hunk.path, &excluder);
                paths.files[id].lines.clear();
            }
        }

        // Per file the hunks go from the bottom up, so the positions above each stay valid.
        let mut work: Vec<(usize, &Hunk)> = commit
            .hunks
            .iter()
            .filter(|hunk| hunk.old_count != 0 || hunk.new_count != 0)
            .map(|hunk| (paths.id(&hunk.path, &excluder), hunk))
            .collect();
        work.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| b.1.old_start.cmp(&a.1.old_start))
        });
        for (id, hunk) in work {
            if paths.excluded[id] {
                // Not followed: its lines are no longer known.
                paths.files[id].lines.clear();
                continue;
            }
            apply(&mut paths.files[id], hunk, at, now, counted);
        }
    }

    let mut out = HashMap::new();
    for (id, mut file) in paths.files.into_iter().enumerate() {
        if paths.excluded[id] || !file.touched {
            continue;
        }
        for line in &file.lines {
            if let Line::Born(born) = *line {
                let age = now.saturating_sub(born);
                file.churn.alive[age_bucket(age)] += 1;
                if age >= CHURN_DAYS {
                    file.churn.added += 1;
                }
            }
        }
        out.insert(paths.names[id].to_owned(), file.churn);
    }
    out
}

/// A marker of a binary side (`Hunk` in mod.rs): all four numbers 0, and no old path.
fn is_marker(hunk: &Hunk) -> bool {
    hunk.old_path.is_none()
        && hunk.old_start == 0
        && hunk.old_count == 0
        && hunk.new_start == 0
        && hunk.new_count == 0
}

/// Replaces the lines a hunk removes with the lines it adds, and records what the removed
/// counted lines did. `at` is the active day of the commit, `now` the last active day.
fn apply(file: &mut Tracked, hunk: &Hunk, at: u32, now: u32, counted: bool) {
    let start = if hunk.old_count == 0 {
        hunk.old_start
    } else {
        hunk.old_start.saturating_sub(1)
    } as usize;
    let end = start + hunk.old_count as usize;
    if file.lines.len() < end {
        // Past the known prefix: the lines there are `Old`, and nothing is recorded for them.
        file.lines.resize(end, Line::Old);
    }
    let new_line = if counted { Line::Born(at) } else { Line::Old };
    if counted && hunk.new_count > 0 {
        file.touched = true;
    }
    let removed: Vec<Line> = file
        .lines
        .splice(
            start..end,
            std::iter::repeat_n(new_line, hunk.new_count as usize),
        )
        .collect();
    if !counted {
        return;
    }
    for line in removed {
        if let Line::Born(born) = line {
            file.touched = true;
            let age = at.saturating_sub(born);
            file.churn.deaths[age_bucket(age)] += 1;
            if now.saturating_sub(born) >= CHURN_DAYS {
                file.churn.added += 1;
                if age <= CHURN_DAYS {
                    file.churn.rewritten += 1;
                }
            }
        }
    }
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use gm_schema::AGE_BUCKETS;
    use proptest::prelude::*;

    use super::*;

    fn sha(i: usize) -> String {
        format!("{i:040x}")
    }

    fn span(path: &str, old_start: u32, old_count: u32, new_count: u32) -> Hunk {
        Hunk {
            path: path.to_owned(),
            old_path: None,
            old_start,
            old_count,
            new_start: old_start,
            new_count,
        }
    }

    fn zero_marker(path: &str) -> Hunk {
        span(path, 0, 0, 0)
    }

    fn renamed(from: &str, to: &str, old_start: u32, old_count: u32, new_count: u32) -> Hunk {
        Hunk {
            old_path: Some(from.to_owned()),
            ..span(to, old_start, old_count, new_count)
        }
    }

    #[test]
    fn a_binary_side_forgets_the_known_lines_without_deaths_and_the_new_lines_are_followed() {
        let commits = timeline(
            20,
            vec![
                (0, vec![span("f.txt", 0, 0, 2)]),
                // The file turned binary: a marker, no text hunk.
                (1, vec![zero_marker("f.txt")]),
                // Text again: one new line at the top, then it dies one day later.
                (2, vec![span("f.txt", 0, 0, 1)]),
                (3, vec![span("f.txt", 1, 1, 0)]),
            ],
        );
        // The two lines of day 0 leave the count without a death; the line of day 2 lived a day.
        let files = follow_each_day(&commits, &HealthConfig::default());
        assert_eq!(files["f.txt"], churn(1, 1, [1, 0, 0, 0, 0, 0, 0], NONE));
    }

    #[test]
    fn a_binary_rename_moves_then_forgets_the_lines() {
        let commits = timeline(
            20,
            vec![
                (0, vec![span("a.txt", 0, 0, 2)]),
                (
                    1,
                    vec![renamed("a.txt", "b.txt", 0, 0, 0), zero_marker("b.txt")],
                ),
            ],
        );
        let files = follow_each_day(&commits, &HealthConfig::default());
        assert_eq!(files.keys().collect::<Vec<_>>(), ["b.txt"]);
        assert_eq!(files["b.txt"], LineChurn::default());
    }

    /// One commit per active day `0..=last`; the `events` are the hunks of some of those days.
    fn timeline(last: usize, events: Vec<(usize, Vec<Hunk>)>) -> Vec<FirstParentCommit> {
        let mut commits: Vec<FirstParentCommit> = (0..=last)
            .map(|i| FirstParentCommit {
                sha: sha(i),
                hunks: Vec::new(),
            })
            .collect();
        for (day, hunks) in events {
            commits[day].hunks = hunks;
        }
        commits
    }

    fn follow_each_day(
        commits: &[FirstParentCommit],
        config: &HealthConfig,
    ) -> HashMap<String, LineChurn> {
        let days: Vec<i64> = (0..commits.len())
            .map(|d| i64::try_from(d).unwrap_or(i64::MAX))
            .collect();
        follow(commits, &days, config)
    }

    fn churn(
        added: u32,
        rewritten: u32,
        deaths: [u32; AGE_BUCKETS],
        alive: [u32; AGE_BUCKETS],
    ) -> LineChurn {
        LineChurn {
            added,
            rewritten,
            deaths,
            alive,
        }
    }

    const NONE: [u32; AGE_BUCKETS] = [0; AGE_BUCKETS];

    #[test]
    fn a_line_added_and_deleted_three_days_later_is_rewritten_once_it_is_old_enough() {
        let commits = timeline(
            16,
            vec![
                (0, vec![span("f.txt", 0, 0, 1)]),
                (3, vec![span("f.txt", 1, 1, 0)]),
            ],
        );
        let files = follow_each_day(&commits, &HealthConfig::default());
        assert_eq!(files["f.txt"], churn(1, 1, [1, 0, 0, 0, 0, 0, 0], NONE));
    }

    #[test]
    fn a_line_that_lives_is_alive_in_the_bucket_of_its_age_and_young_lines_are_not_added() {
        let commits = timeline(20, vec![(0, vec![span("f.txt", 0, 0, 2)])]);
        let files = follow_each_day(&commits, &HealthConfig::default());
        assert_eq!(files["f.txt"], churn(2, 0, NONE, [0, 0, 2, 0, 0, 0, 0]));

        let young = timeline(10, vec![(0, vec![span("f.txt", 0, 0, 1)])]);
        let files = follow_each_day(&young, &HealthConfig::default());
        assert_eq!(files["f.txt"], churn(0, 0, NONE, [0, 1, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn a_modified_line_is_a_death_and_a_birth() {
        let commits = timeline(
            20,
            vec![
                (0, vec![span("f.txt", 0, 0, 3)]),
                (1, vec![span("f.txt", 2, 1, 1)]),
            ],
        );
        let files = follow_each_day(&commits, &HealthConfig::default());
        // The three lines of day 0 are added (day 20 is 20 days after them); the new line of day
        // 1 is 19 days old. The one that died lived a day: rewritten.
        assert_eq!(
            files["f.txt"],
            churn(4, 1, [1, 0, 0, 0, 0, 0, 0], [0, 0, 3, 0, 0, 0, 0])
        );
    }

    #[test]
    fn insertions_go_after_the_line_they_name() {
        let commits = timeline(
            20,
            vec![
                (0, vec![span("f.txt", 0, 0, 2)]),
                (1, vec![span("f.txt", 1, 0, 1)]),
                (8, vec![span("f.txt", 2, 1, 0)]),
                (20, vec![span("f.txt", 2, 1, 0)]),
            ],
        );
        // Day 0 adds A and B, day 1 adds N between them, day 8 removes N (7 days old) and day 20
        // removes B (20 days old). A survives.
        let files = follow_each_day(&commits, &HealthConfig::default());
        assert_eq!(
            files["f.txt"],
            churn(3, 1, [1, 0, 1, 0, 0, 0, 0], [0, 0, 1, 0, 0, 0, 0])
        );
    }

    #[test]
    fn a_deletion_past_the_known_prefix_records_nothing() {
        let commits = timeline(
            20,
            vec![
                (0, vec![span("f.txt", 0, 0, 1)]),
                (1, vec![span("f.txt", 5, 2, 0)]),
            ],
        );
        let files = follow_each_day(&commits, &HealthConfig::default());
        assert_eq!(files["f.txt"], churn(1, 0, NONE, [0, 0, 1, 0, 0, 0, 0]));
    }

    #[test]
    fn a_rename_carries_the_lines_and_their_history_to_the_new_path() {
        let commits = timeline(
            20,
            vec![
                (0, vec![span("a.txt", 0, 0, 2)]),
                (1, vec![renamed("a.txt", "b.txt", 2, 1, 1)]),
            ],
        );
        let files = follow_each_day(&commits, &HealthConfig::default());
        assert_eq!(files.len(), 1);
        assert_eq!(
            files["b.txt"],
            churn(3, 1, [1, 0, 0, 0, 0, 0, 0], [0, 0, 2, 0, 0, 0, 0])
        );
    }

    #[test]
    fn a_rename_without_a_change_moves_the_lines() {
        let commits = timeline(
            20,
            vec![
                (0, vec![span("a.txt", 0, 0, 1)]),
                (1, vec![renamed("a.txt", "b.txt", 0, 0, 0)]),
            ],
        );
        let files = follow_each_day(&commits, &HealthConfig::default());
        assert_eq!(files.len(), 1);
        assert_eq!(files["b.txt"], churn(1, 0, NONE, [0, 0, 1, 0, 0, 0, 0]));
    }

    #[test]
    fn an_ignored_commit_moves_lines_and_counts_nothing() {
        let commits = timeline(
            20,
            vec![
                (0, vec![span("a.txt", 0, 0, 2)]),
                (
                    1,
                    vec![renamed("a.txt", "b.txt", 2, 1, 1), span("z.txt", 0, 0, 2)],
                ),
            ],
        );
        let config = HealthConfig {
            ignore_revs: vec![sha(1)],
            ..HealthConfig::default()
        };
        let files = follow_each_day(&commits, &config);
        // The ignored commit replaced line 2 of a.txt: the line it removed leaves the count, the
        // line it added is not counted, and z.txt (only added there) gets no entry.
        assert_eq!(files.len(), 1);
        assert_eq!(files["b.txt"], churn(1, 0, NONE, [0, 0, 1, 0, 0, 0, 0]));
    }

    #[test]
    fn an_excluded_path_gets_no_entry() {
        let commits = timeline(
            20,
            vec![(
                0,
                vec![span("vendor/x.rs", 0, 0, 2), span("src/a.rs", 0, 0, 2)],
            )],
        );
        let config = HealthConfig {
            exclude: vec!["vendor/".to_owned()],
            ..HealthConfig::default()
        };
        let files = follow_each_day(&commits, &config);
        assert_eq!(files.keys().collect::<Vec<_>>(), ["src/a.rs"]);
    }

    /// One generated commit: a file index, whether it renames the file, and its edits as
    /// (gap, removed, added) bytes.
    type Edit = (u8, u8, u8);
    type Op = (u8, bool, Vec<Edit>);

    /// A random but valid history: each commit edits one of three files, sometimes renaming it,
    /// with non-overlapping hunks against the file's real length.
    fn build(ops: &[Op]) -> (Vec<FirstParentCommit>, Vec<i64>, u64) {
        let mut lens: HashMap<String, u32> = HashMap::new();
        let mut commits = Vec::new();
        let mut days = Vec::new();
        let mut births = 0u64;
        for (ci, (file, rename, edits)) in ops.iter().enumerate() {
            let name = format!("f{file}");
            let target = if *rename {
                format!("r{ci}")
            } else {
                name.clone()
            };
            let old_path = (target != name).then(|| name.clone());
            let len = if *rename {
                lens.remove(&name).unwrap_or(0)
            } else {
                lens.get(&name).copied().unwrap_or(0)
            };
            let mut hunks = Vec::new();
            let mut pos = 0u32;
            let mut new_len = len;
            let mut last_insert_at = None;
            for &(gap, del, ins) in edits {
                pos += u32::from(gap) % (len - pos + 1);
                let d = u32::from(del) % (len - pos + 1);
                let i = u32::from(ins) % 4;
                if d == 0 && (i == 0 || last_insert_at == Some(pos)) {
                    continue;
                }
                if d == 0 {
                    last_insert_at = Some(pos);
                }
                hunks.push(Hunk {
                    path: target.clone(),
                    old_path: old_path.clone(),
                    old_start: if d == 0 { pos } else { pos + 1 },
                    old_count: d,
                    new_start: 0,
                    new_count: i,
                });
                pos += d;
                new_len = new_len - d + i;
                births += u64::from(i);
            }
            if hunks.is_empty() && old_path.is_some() {
                hunks.push(Hunk {
                    path: target.clone(),
                    old_path: old_path.clone(),
                    old_start: 0,
                    old_count: 0,
                    new_start: 0,
                    new_count: 0,
                });
            }
            lens.insert(target, new_len);
            commits.push(FirstParentCommit {
                sha: sha(ci),
                hunks,
            });
            days.push(i64::try_from(ci / 2).unwrap_or(i64::MAX));
        }
        (commits, days, births)
    }

    fn ops() -> impl Strategy<Value = Vec<Op>> {
        prop::collection::vec(
            (
                0u8..3,
                any::<bool>(),
                prop::collection::vec((any::<u8>(), any::<u8>(), any::<u8>()), 0..4),
            ),
            1..60,
        )
    }

    fn totals(files: &HashMap<String, LineChurn>) -> u64 {
        files
            .values()
            .flat_map(|c| c.deaths.iter().chain(c.alive.iter()))
            .map(|&n| u64::from(n))
            .sum()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn replaying_valid_hunks_never_panics_and_every_birth_dies_or_lives(ops in ops()) {
            let (commits, days, births) = build(&ops);
            let files = follow(&commits, &days, &HealthConfig::default());
            prop_assert_eq!(totals(&files), births);
            for churn in files.values() {
                prop_assert!(churn.rewritten <= churn.added);
            }
        }

        #[test]
        fn ignored_commits_never_panic_and_never_add_to_the_totals(ops in ops()) {
            let (commits, days, births) = build(&ops);
            let config = HealthConfig {
                ignore_revs: commits.iter().step_by(3).map(|c| c.sha.clone()).collect(),
                ..HealthConfig::default()
            };
            let files = follow(&commits, &days, &config);
            prop_assert!(totals(&files) <= births);
        }
    }
}

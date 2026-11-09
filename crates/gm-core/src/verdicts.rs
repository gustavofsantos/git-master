//! The pure rules of verdicts and reviewers (D189, D212): when a verdict is stale, what a value
//! must look like for its reviewer's signal, and which reviewers are defined. No Git and no
//! model here; the engine reads the branch and the files and calls these.

use std::path::Path;

use gm_schema::{Reviewer, ReviewerFileError, ReviewerSignal};

use crate::reviewers;

/// The highest score a reviewer can record (D189).
const SCORE_MAX: u32 = 100;
/// The longest rationale a verdict keeps, in characters (D212 (6)).
pub const MAX_RATIONALE_CHARS: usize = 4_000;
/// The longest body of a finding, in characters (D212 (6)).
pub const MAX_FINDING_BODY_CHARS: usize = 1_000;
/// The most findings one verdict records (D212 (6)).
pub const MAX_FINDINGS: usize = 30;
/// The verdicts a listing returns when it names no limit, and the most it returns.
pub const DEFAULT_LIST_LIMIT: u32 = 5;
pub const MAX_LIST_LIMIT: u32 = 20;

/// Whether a verdict recorded at `verdict_head` is stale: the branch's tip is not that commit,
/// or the branch no longer exists (`current_tip` is `None`). A stale verdict shows, and does not
/// count for a gate (D189 (2)).
#[must_use]
pub fn is_stale(verdict_head: &str, current_tip: Option<&str>) -> bool {
    current_tip != Some(verdict_head)
}

/// Checks `value` against `signal` and returns it as it is stored: a score as an integer from 0
/// to 100 without leading zeros, a level as `low`, `medium` or `high`, and a pass as `pass` or
/// `fail`, in lower case. Anything else is refused, and the reason says what was expected.
pub fn check_value(signal: ReviewerSignal, value: &str) -> Result<String, String> {
    let text = value.trim();
    match signal {
        ReviewerSignal::Score => whole_number(text)
            .filter(|score| *score <= SCORE_MAX)
            .map(|score| score.to_string())
            .ok_or_else(|| format!("a score is a whole number from 0 to 100, not {value:?}")),
        ReviewerSignal::Level => {
            let lower = text.to_ascii_lowercase();
            match lower.as_str() {
                "low" | "medium" | "high" => Ok(lower),
                _ => Err(format!("a level is low, medium or high, not {value:?}")),
            }
        }
        ReviewerSignal::Pass => {
            let lower = text.to_ascii_lowercase();
            match lower.as_str() {
                "pass" | "fail" => Ok(lower),
                _ => Err(format!("a pass is pass or fail, not {value:?}")),
            }
        }
    }
}

/// A decimal number of at most three digits, so a sign or a longer text is not one.
fn whole_number(text: &str) -> Option<u32> {
    let plain =
        !text.is_empty() && text.len() <= 3 && text.bytes().all(|byte| byte.is_ascii_digit());
    if plain { text.parse().ok() } else { None }
}

/// The reviewers a run can use (D212 (1)): the repository's and the user's, as
/// `reviewers::load` reads them (the repository's wins by name), then each preset whose name no
/// file defines, sorted by name. The errors are the reviewer files that could not be read.
#[must_use]
pub fn merged_reviewers(
    repo_dir: &Path,
    user_dir: Option<&Path>,
) -> (Vec<Reviewer>, Vec<ReviewerFileError>) {
    let loaded = reviewers::load(Some(repo_dir), user_dir);
    let mut reviewers = loaded.reviewers;
    for preset in reviewers::presets() {
        if !reviewers
            .iter()
            .any(|reviewer| reviewer.name == preset.name)
        {
            reviewers.push(preset);
        }
    }
    reviewers.sort_by(|a, b| a.name.cmp(&b.name));
    let errors = loaded
        .errors
        .into_iter()
        .map(|(file, reason)| ReviewerFileError { file, reason })
        .collect();
    (reviewers, errors)
}

#[cfg(test)]
mod tests {
    use gm_schema::{ReviewerDirection, ReviewerSource, ReviewerSubject};

    use super::*;

    fn reviewer(name: &str, description: &str) -> Reviewer {
        Reviewer {
            name: name.to_owned(),
            description: description.to_owned(),
            instructions: "Look at the change.".to_owned(),
            signal: ReviewerSignal::Level,
            direction: ReviewerDirection::LowerIsBetter,
            subject: ReviewerSubject::Branch,
            model: None,
            source: ReviewerSource::Repository,
        }
    }

    #[test]
    fn a_verdict_is_stale_when_the_tip_moved_or_the_branch_is_gone() {
        assert!(!is_stale("abc", Some("abc")));
        assert!(is_stale("abc", Some("def")));
        assert!(is_stale("abc", None));
    }

    #[test]
    fn a_score_is_a_whole_number_from_0_to_100_stored_without_leading_zeros() {
        assert_eq!(check_value(ReviewerSignal::Score, "0").unwrap(), "0");
        assert_eq!(check_value(ReviewerSignal::Score, "07").unwrap(), "7");
        assert_eq!(check_value(ReviewerSignal::Score, " 100 ").unwrap(), "100");
    }

    #[test]
    fn a_score_outside_0_to_100_or_not_a_number_is_refused_with_what_is_expected() {
        for bad in ["101", "1000", "-1", "+5", "", "abc", "3.5", "0000"] {
            let reason = check_value(ReviewerSignal::Score, bad).unwrap_err();
            assert!(
                reason.contains("whole number from 0 to 100"),
                "{bad}: {reason}"
            );
        }
    }

    #[test]
    fn a_level_is_low_medium_or_high_stored_in_lower_case() {
        assert_eq!(check_value(ReviewerSignal::Level, "Low").unwrap(), "low");
        assert_eq!(check_value(ReviewerSignal::Level, "HIGH").unwrap(), "high");
        assert_eq!(
            check_value(ReviewerSignal::Level, "medium").unwrap(),
            "medium"
        );
    }

    #[test]
    fn a_level_that_is_not_low_medium_or_high_is_refused() {
        let reason = check_value(ReviewerSignal::Level, "critical").unwrap_err();
        assert_eq!(reason, "a level is low, medium or high, not \"critical\"");
        assert!(check_value(ReviewerSignal::Level, "").is_err());
        assert!(check_value(ReviewerSignal::Level, "50").is_err());
    }

    #[test]
    fn a_pass_is_pass_or_fail_and_nothing_else() {
        assert_eq!(check_value(ReviewerSignal::Pass, "PASS").unwrap(), "pass");
        assert_eq!(check_value(ReviewerSignal::Pass, "fail").unwrap(), "fail");
        let reason = check_value(ReviewerSignal::Pass, "maybe").unwrap_err();
        assert_eq!(reason, "a pass is pass or fail, not \"maybe\"");
    }

    #[test]
    fn the_preset_is_used_when_no_file_defines_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let (found, errors) = merged_reviewers(dir.path(), None);
        assert!(errors.is_empty());
        let names: Vec<&str> = found
            .iter()
            .map(|reviewer| reviewer.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["code-review", "intent-drift", "risk", "security", "slop"]
        );
        assert!(
            found
                .iter()
                .all(|reviewer| reviewer.source == ReviewerSource::Preset)
        );
    }

    #[test]
    fn a_repository_file_wins_over_a_user_file_and_both_win_over_a_preset() {
        let repo = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        reviewers::save(repo.path(), &reviewer("security", "repository")).unwrap();
        reviewers::save(user.path(), &reviewer("security", "user")).unwrap();
        reviewers::save(user.path(), &reviewer("style", "user style")).unwrap();
        let (found, _) = merged_reviewers(repo.path(), Some(user.path()));
        let security = found.iter().find(|r| r.name == "security").unwrap();
        assert_eq!(security.description, "repository");
        assert_eq!(security.source, ReviewerSource::Repository);
        let style = found.iter().find(|r| r.name == "style").unwrap();
        assert_eq!(style.source, ReviewerSource::User);
        assert_eq!(found.iter().filter(|r| r.name == "security").count(), 1);
    }

    #[test]
    fn the_merged_list_is_sorted_by_name_and_a_bad_file_is_an_error_not_a_reviewer() {
        let repo = tempfile::tempdir().unwrap();
        reviewers::save(repo.path(), &reviewer("zeta", "last")).unwrap();
        std::fs::write(repo.path().join("broken.md"), "no front matter at all").unwrap();
        let (found, errors) = merged_reviewers(repo.path(), None);
        let names: Vec<&str> = found
            .iter()
            .map(|reviewer| reviewer.name.as_str())
            .collect();
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]), "{names:?}");
        assert!(names.contains(&"zeta"));
        assert!(!names.contains(&"broken"));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "broken.md");
    }
}

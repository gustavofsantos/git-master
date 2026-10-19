//! The checks the engine makes before Git runs a branch command. Pure: no I/O,
//! so the rules are tested on their own. Git stays the final judge of a name
//! (`git check-ref-format --branch`); these rules refuse what Git would refuse
//! with a clearer sentence, and Git's own message is passed through for the rest.

/// The first rule of `git check-ref-format --branch` that `name` breaks, as the
/// sentence the user reads. `None` when the name may be a branch.
#[must_use]
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("A branch name cannot be empty.");
    }
    if name.starts_with('-') {
        return Some("A branch name cannot start with '-'.");
    }
    if name.chars().any(|c| c.is_ascii_control()) {
        return Some("A branch name cannot contain control characters.");
    }
    if name.contains(' ') {
        return Some("A branch name cannot contain spaces.");
    }
    if name.contains(['~', '^', ':', '?', '*', '[', '\\']) {
        return Some("A branch name cannot contain ~ ^ : ? * [ or \\.");
    }
    if name.contains("..") {
        return Some("A branch name cannot contain '..'.");
    }
    if name.contains("@{") {
        return Some("A branch name cannot contain '@{'.");
    }
    if name == "@" {
        return Some("A branch name cannot be '@'.");
    }
    if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return Some("A branch name cannot start or end with '/' or contain '//'.");
    }
    if name.ends_with('.') {
        return Some("A branch name cannot end with '.'.");
    }
    for part in name.split('/') {
        if part.starts_with('.') {
            return Some("A part of a branch name cannot start with '.'.");
        }
        if part.rsplit_once('.').is_some_and(|(_, ext)| ext == "lock") {
            return Some("A part of a branch name cannot end with '.lock'.");
        }
    }
    None
}

/// The start point of a new branch, when it can be one: a name Git would not
/// read as an option. `None` when it is usable.
#[must_use]
pub fn start_point_problem(start: &str) -> Option<&'static str> {
    if start.is_empty() || start.starts_with('-') {
        Some("Choose a branch, tag or commit to start from.")
    } else {
        None
    }
}

/// Why local branch `name` cannot be deleted, given the checkout that has it:
/// the folder and whether that folder is the window's own. `None` when no
/// checkout has it.
#[must_use]
pub fn delete_refusal(name: &str, checked_out_at: Option<(&str, bool)>) -> Option<String> {
    let (path, open) = checked_out_at?;
    Some(if open {
        format!("Cannot delete the checked-out branch {name}.")
    } else {
        format!("Cannot delete {name}: it is checked out in the worktree at {path}.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each name with the sentence it gets; `None` marks a name Git accepts.
    const TABLE: &[(&str, Option<&str>)] = &[
        ("feature", None),
        ("feature/login-form", None),
        ("release-1.2", None),
        ("v1.2.3", None),
        ("pay@home", None),
        ("a@b", None),
        ("@x", None),
        ("x@", None),
        ("ünïcode", None),
        ("", Some("A branch name cannot be empty.")),
        ("-x", Some("A branch name cannot start with '-'.")),
        ("--force", Some("A branch name cannot start with '-'.")),
        ("a b", Some("A branch name cannot contain spaces.")),
        (" lead", Some("A branch name cannot contain spaces.")),
        (
            "a\tb",
            Some("A branch name cannot contain control characters."),
        ),
        (
            "a\nb",
            Some("A branch name cannot contain control characters."),
        ),
        (
            "bell\u{7}",
            Some("A branch name cannot contain control characters."),
        ),
        (
            "del\u{7f}",
            Some("A branch name cannot contain control characters."),
        ),
        ("a..b", Some("A branch name cannot contain '..'.")),
        (
            "a~b",
            Some("A branch name cannot contain ~ ^ : ? * [ or \\."),
        ),
        (
            "a^b",
            Some("A branch name cannot contain ~ ^ : ? * [ or \\."),
        ),
        (
            "a:b",
            Some("A branch name cannot contain ~ ^ : ? * [ or \\."),
        ),
        (
            "a?b",
            Some("A branch name cannot contain ~ ^ : ? * [ or \\."),
        ),
        (
            "a*b",
            Some("A branch name cannot contain ~ ^ : ? * [ or \\."),
        ),
        (
            "a[b",
            Some("A branch name cannot contain ~ ^ : ? * [ or \\."),
        ),
        (
            "a\\b",
            Some("A branch name cannot contain ~ ^ : ? * [ or \\."),
        ),
        ("a@{1}", Some("A branch name cannot contain '@{'.")),
        ("@", Some("A branch name cannot be '@'.")),
        (
            "/feature",
            Some("A branch name cannot start or end with '/' or contain '//'."),
        ),
        (
            "feature/",
            Some("A branch name cannot start or end with '/' or contain '//'."),
        ),
        (
            "a//b",
            Some("A branch name cannot start or end with '/' or contain '//'."),
        ),
        ("feature.", Some("A branch name cannot end with '.'.")),
        (
            ".hidden",
            Some("A part of a branch name cannot start with '.'."),
        ),
        (
            "a/.b",
            Some("A part of a branch name cannot start with '.'."),
        ),
        (
            "tip.lock",
            Some("A part of a branch name cannot end with '.lock'."),
        ),
        (
            "a.lock/b",
            Some("A part of a branch name cannot end with '.lock'."),
        ),
    ];

    #[test]
    fn every_name_in_the_table_gets_its_sentence() {
        for (name, expected) in TABLE {
            assert_eq!(name_problem(name), *expected, "{name:?}");
        }
    }

    #[test]
    fn each_rule_has_its_own_sentence() {
        assert_eq!(name_problem(""), Some("A branch name cannot be empty."));
        assert_eq!(
            name_problem("a b"),
            Some("A branch name cannot contain spaces.")
        );
        assert_eq!(
            name_problem("a\tb"),
            Some("A branch name cannot contain control characters.")
        );
        assert_eq!(
            name_problem("a..b"),
            Some("A branch name cannot contain '..'.")
        );
        assert_eq!(
            name_problem("a@{1}"),
            Some("A branch name cannot contain '@{'.")
        );
        assert_eq!(name_problem("@"), Some("A branch name cannot be '@'."));
        assert_eq!(
            name_problem("feature."),
            Some("A branch name cannot end with '.'.")
        );
        assert_eq!(
            name_problem("tip.lock"),
            Some("A part of a branch name cannot end with '.lock'.")
        );
    }

    #[test]
    fn a_start_point_must_not_be_empty_or_an_option() {
        assert_eq!(start_point_problem("main"), None);
        assert_eq!(start_point_problem("HEAD~2"), None);
        assert!(start_point_problem("").is_some());
        assert!(start_point_problem("--all").is_some());
    }

    #[test]
    fn a_branch_checked_out_here_or_elsewhere_is_named_in_the_refusal() {
        assert_eq!(
            delete_refusal("main", Some(("/repo", true))),
            Some("Cannot delete the checked-out branch main.".to_owned())
        );
        assert_eq!(
            delete_refusal("feature", Some(("/repo-feature", false))),
            Some(
                "Cannot delete feature: it is checked out in the worktree at /repo-feature."
                    .to_owned()
            )
        );
        assert_eq!(delete_refusal("feature", None), None);
    }
}

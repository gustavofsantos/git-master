//! The text of the dialog that shows the answer of Convert Hooks to Workflows (`WF3e`, D160): the
//! workflow files written, the sections left out with their reason, and the `git config` command
//! that removes each section the person no longer needs. The engine never edits Git config, so the
//! commands are for the person to run. No GPUI here.

/// The title of the dialog that shows a conversion's answer (`WF3e`).
pub const CONVERT_TITLE: &str = "Convert Hooks to Workflows";

/// The text of the conversion dialog: the files written, the sections left out with their reason,
/// and the command that removes each section the person no longer needs. The engine never edits
/// Git config, so the commands are for the person to run.
#[must_use]
pub fn convert_report(
    written: &[String],
    skipped: &[(String, String)],
    sections: &[String],
) -> String {
    let mut lines: Vec<String> = Vec::new();
    if written.is_empty() {
        lines.push("No workflow was written.".to_owned());
    } else {
        lines.push(
            "Written to your workflows folder (unreviewed: review it before it runs):".to_owned(),
        );
        lines.extend(written.iter().map(|path| format!("  {path}")));
    }
    if !skipped.is_empty() {
        lines.push(String::new());
        lines.push("Not converted:".to_owned());
        lines.extend(
            skipped
                .iter()
                .map(|(entry, reason)| format!("  {}: {reason}", entry_section(entry))),
        );
    }
    if !sections.is_empty() {
        lines.push(String::new());
        lines.push(
            "Remove the sections you no longer need (GitMaster does not read them any more):"
                .to_owned(),
        );
        lines.extend(
            sections
                .iter()
                .map(|entry| format!("  {}", remove_command(entry))),
        );
    }
    lines.join("\n")
}

/// The section of an entry (`<scope> gitmaster.prepare-commit.<name>`), without its scope.
fn entry_section(entry: &str) -> &str {
    entry.split_once(' ').map_or(entry, |(_, section)| section)
}

/// The `git config` command that removes an entry's section from its scope.
#[must_use]
pub fn remove_command(entry: &str) -> String {
    let (scope, section) = entry.split_once(' ').unwrap_or(("local", entry));
    format!(
        "git config --{scope} --remove-section {}",
        shell_word(section)
    )
}

/// `text` as one shell word: left as it is when it holds only safe characters, else quoted.
fn shell_word(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_convert_report_lists_files_skips_and_removal_commands() {
        let text = convert_report(
            &["/w/prepare-commit-llm.lua".to_owned()],
            &[(
                "local gitmaster.prepare-commit.team".to_owned(),
                "a repository section is not converted".to_owned(),
            )],
            &[
                "global gitmaster.prepare-commit.llm".to_owned(),
                "local gitmaster.prepare-commit.team".to_owned(),
            ],
        );
        assert!(text.contains("  /w/prepare-commit-llm.lua"));
        assert!(
            text.contains("  gitmaster.prepare-commit.team: a repository section is not converted")
        );
        assert!(
            text.contains("  git config --global --remove-section gitmaster.prepare-commit.llm")
        );
        assert!(
            text.contains("  git config --local --remove-section gitmaster.prepare-commit.team")
        );
    }

    #[test]
    fn a_convert_with_nothing_written_says_so() {
        assert_eq!(convert_report(&[], &[], &[]), "No workflow was written.");
    }

    #[test]
    fn a_section_name_with_a_space_is_quoted_in_the_command() {
        assert_eq!(
            remove_command("global gitmaster.prepare-commit.my llm"),
            "git config --global --remove-section 'gitmaster.prepare-commit.my llm'"
        );
        assert_eq!(
            remove_command("global gitmaster.prepare-commit.it's"),
            "git config --global --remove-section 'gitmaster.prepare-commit.it'\\''s'"
        );
    }
}

//! Parses `git worktree list --porcelain`. Pure: the adapter runs git and
//! passes the bytes here.

use gm_schema::WorktreeEntry;

use crate::GitError;

/// Parses the output of `git worktree list --porcelain -z` (`nul` true) or of
/// the newline form (`nul` false, for git before 2.36). Records are separated
/// by an empty token. In the newline form a path with a newline cannot be
/// told apart, so the NUL form is used whenever git accepts it.
pub fn parse(raw: &[u8], nul: bool) -> Result<Vec<WorktreeEntry>, GitError> {
    let terminator = if nul { b'\0' } else { b'\n' };
    let mut entries = Vec::new();
    let mut draft: Option<Draft> = None;
    for token in raw.split(|&byte| byte == terminator) {
        if token.is_empty() {
            entries.extend(draft.take().map(Draft::finish));
            continue;
        }
        let line = String::from_utf8_lossy(token);
        if let Some(path) = line.strip_prefix("worktree ") {
            entries.extend(draft.take().map(Draft::finish));
            draft = Some(Draft::new(path));
            continue;
        }
        let Some(current) = draft.as_mut() else {
            return Err(GitError(format!("unexpected worktree line: {line}")));
        };
        current.attribute(&line);
    }
    entries.extend(draft.take().map(Draft::finish));
    for (index, entry) in entries.iter_mut().enumerate() {
        entry.is_main = index == 0;
    }
    Ok(entries)
}

/// An entry while its attribute lines are read.
struct Draft {
    path: String,
    head: Option<String>,
    branch: Option<String>,
    detached: bool,
    bare: bool,
    locked: Option<String>,
    prunable: Option<String>,
}

impl Draft {
    fn new(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            head: None,
            branch: None,
            detached: false,
            bare: false,
            locked: None,
            prunable: None,
        }
    }

    /// Reads one attribute line. Unknown ones are skipped, so a newer git
    /// that adds an attribute still lists its worktrees.
    fn attribute(&mut self, line: &str) {
        if let Some(sha) = line.strip_prefix("HEAD ") {
            self.head = Some(sha.to_owned());
        } else if let Some(reference) = line.strip_prefix("branch ") {
            let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
            self.branch = Some(name.to_owned());
        } else if line == "detached" {
            self.detached = true;
        } else if line == "bare" {
            self.bare = true;
        } else if line == "locked" || line.starts_with("locked ") {
            self.locked = Some(reason(line, "locked"));
        } else if line == "prunable" || line.starts_with("prunable ") {
            self.prunable = Some(reason(line, "prunable"));
        }
    }

    fn finish(self) -> WorktreeEntry {
        WorktreeEntry {
            path: self.path,
            head: self.head,
            branch: self.branch,
            detached: self.detached,
            bare: self.bare,
            is_main: false,
            locked: self.locked,
            prunable: self.prunable,
        }
    }
}

/// The text after `keyword `, or empty when git gave no reason.
fn reason(line: &str, keyword: &str) -> String {
    line.strip_prefix(keyword)
        .and_then(|rest| rest.strip_prefix(' '))
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_nul(text: &str) -> Vec<WorktreeEntry> {
        parse(text.as_bytes(), true).unwrap()
    }

    #[test]
    fn main_checkout_is_the_first_entry_on_a_branch() {
        let entries = parse_nul(
            "worktree /repo\0HEAD aaa\0branch refs/heads/main\0\0\
             worktree /repo-feature\0HEAD bbb\0branch refs/heads/feature/x\0\0",
        );
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_main);
        assert!(!entries[1].is_main);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert_eq!(entries[1].branch.as_deref(), Some("feature/x"));
        assert_eq!(entries[1].head.as_deref(), Some("bbb"));
        assert!(!entries[1].detached && !entries[1].bare);
    }

    #[test]
    fn detached_head_has_no_branch() {
        let entries = parse_nul("worktree /repo\0HEAD aaa\0detached\0\0");
        assert_eq!(entries[0].branch, None);
        assert!(entries[0].detached);
    }

    #[test]
    fn bare_entry_has_no_head() {
        let entries = parse_nul("worktree /srv/repo.git\0bare\0\0");
        assert!(entries[0].bare);
        assert!(entries[0].is_main);
        assert_eq!(entries[0].head, None);
        assert_eq!(entries[0].branch, None);
    }

    #[test]
    fn lock_with_and_without_reason() {
        let entries = parse_nul(
            "worktree /repo\0HEAD aaa\0branch refs/heads/main\0\0\
             worktree /a\0HEAD bbb\0branch refs/heads/a\0locked\0\0\
             worktree /b\0HEAD ccc\0branch refs/heads/b\0locked on the USB stick\0\0",
        );
        assert_eq!(entries[1].locked.as_deref(), Some(""));
        assert_eq!(entries[2].locked.as_deref(), Some("on the USB stick"));
        assert_eq!(entries[0].locked, None);
    }

    #[test]
    fn prunable_with_and_without_reason() {
        let entries = parse_nul(
            "worktree /repo\0HEAD aaa\0branch refs/heads/main\0\0\
             worktree /gone\0HEAD bbb\0branch refs/heads/gone\0prunable gitdir file points to non-existent location\0\0\
             worktree /gone2\0HEAD ccc\0prunable\0\0",
        );
        assert_eq!(
            entries[1].prunable.as_deref(),
            Some("gitdir file points to non-existent location")
        );
        assert_eq!(entries[2].prunable.as_deref(), Some(""));
        assert_eq!(entries[0].prunable, None);
    }

    #[test]
    fn path_with_spaces_is_kept_whole() {
        let entries = parse_nul("worktree /home/me/my  repo wt \0HEAD aaa\0detached\0\0");
        assert_eq!(entries[0].path, "/home/me/my  repo wt ");
    }

    #[test]
    fn path_with_newline_survives_the_nul_form() {
        let entries = parse_nul("worktree /tmp/two\nlines\0HEAD aaa\0detached\0\0");
        assert_eq!(entries[0].path, "/tmp/two\nlines");
    }

    #[test]
    fn newline_form_reads_the_same_records() {
        let text = "worktree /repo\nHEAD aaa\nbranch refs/heads/main\n\nworktree /other wt\nHEAD bbb\ndetached\nlocked\n\n";
        let entries = parse(text.as_bytes(), false).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].path, "/other wt");
        assert!(entries[1].detached);
        assert_eq!(entries[1].locked.as_deref(), Some(""));
    }

    #[test]
    fn a_last_record_without_its_trailing_separator_is_kept() {
        let entries = parse_nul("worktree /repo\0HEAD aaa\0branch refs/heads/main");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn unknown_attributes_are_skipped() {
        let entries =
            parse_nul("worktree /repo\0HEAD aaa\0branch refs/heads/main\0future-thing 1\0\0");
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn empty_output_lists_nothing() {
        assert!(parse_nul("").is_empty());
    }

    #[test]
    fn an_attribute_before_any_worktree_is_an_error() {
        assert!(parse(b"HEAD aaa\0\0", true).is_err());
    }
}

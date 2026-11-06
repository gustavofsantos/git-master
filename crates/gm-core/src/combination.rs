//! Record Combination (D180(3)): the pure part. The checked-out members of a
//! workspace, the ones whose commit is new, and the message the workspace commit
//! gets. The Git reads and the commit are in `git.rs`; the engine joins them.

/// A checked-out member of a workspace, as Git reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// The folder, relative to the workspace root.
    pub path: String,
    /// The branch the member has checked out; `None` when its HEAD is detached.
    pub branch: Option<String>,
    /// The commit the member's HEAD is at; `None` when the branch has no commit yet.
    pub head: Option<String>,
    /// The commit the workspace records for the member; `None` when it records none.
    pub recorded: Option<String>,
}

/// What the combination commits: the gitlinks that change, and the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The folders whose gitlink the commit records, in the order of the members.
    pub paths: Vec<String>,
    pub message: String,
}

/// The number of characters of a commit id the message shows.
const SHORT_ID: usize = 7;

/// Plans the combination of `members`, or says why it is refused. The refusal is the
/// text the operation fails with. A detached member is named, so the person knows which.
pub fn plan(members: &[Member]) -> Result<Plan, String> {
    if members.is_empty() {
        return Err("This repository has no submodule to record.".to_owned());
    }
    if let Some(member) = members.iter().find(|member| member.branch.is_none()) {
        return Err(format!(
            "{} is on a detached HEAD: check out its branch first.",
            member.path
        ));
    }
    if let Some(member) = members.iter().find(|member| member.head.is_none()) {
        return Err(format!("{} has no commit yet.", member.path));
    }
    let paths: Vec<String> = members
        .iter()
        .filter(|member| member.head != member.recorded)
        .map(|member| member.path.clone())
        .collect();
    if paths.is_empty() {
        return Err("No member has a new commit.".to_owned());
    }
    Ok(Plan {
        paths,
        message: message(members),
    })
}

/// The message of the combination: `Record combination`, a blank line, then one line
/// per member with its folder, its branch and its short commit id. The columns are
/// aligned on the longest folder and the longest branch, two spaces apart.
pub fn message(members: &[Member]) -> String {
    let folders = members
        .iter()
        .map(|member| member.path.chars().count())
        .max()
        .unwrap_or(0);
    let branches = members
        .iter()
        .map(|member| member.branch.as_deref().unwrap_or("").chars().count())
        .max()
        .unwrap_or(0);
    let rows: Vec<String> = members
        .iter()
        .map(|member| {
            let branch = member.branch.as_deref().unwrap_or("");
            let short: String = member
                .head
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(SHORT_ID)
                .collect();
            format!("{:<folders$}  {:<branches$}  {short}", member.path, branch)
        })
        .collect();
    format!("Record combination\n\n{}\n", rows.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(path: &str, branch: Option<&str>, head: &str, recorded: &str) -> Member {
        Member {
            path: path.to_owned(),
            branch: branch.map(str::to_owned),
            head: Some(head.to_owned()),
            recorded: Some(recorded.to_owned()),
        }
    }

    const NEW: &str = "4f2a9c1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OLD: &str = "1111111bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const OTHER: &str = "9be03d7cccccccccccccccccccccccccccccccc";

    #[test]
    fn the_message_lists_every_member_with_aligned_columns() {
        let members = [
            member("billing", Some("feat/pay-v2"), NEW, NEW),
            member("checkout", Some("feat/pay-v2"), OTHER, OLD),
        ];

        assert_eq!(
            message(&members),
            "Record combination\n\nbilling   feat/pay-v2  4f2a9c1\ncheckout  feat/pay-v2  9be03d7\n"
        );
    }

    #[test]
    fn the_columns_follow_the_longest_branch() {
        let members = [
            member("a", Some("main"), NEW, NEW),
            member("bb", Some("feature/long"), OTHER, OLD),
        ];

        assert_eq!(
            message(&members),
            "Record combination\n\na   main          4f2a9c1\nbb  feature/long  9be03d7\n"
        );
    }

    #[test]
    fn a_plan_stages_only_the_members_whose_commit_is_new() {
        let members = [
            member("billing", Some("feat"), NEW, OLD),
            member("checkout", Some("main"), OTHER, OTHER),
        ];

        let plan = plan(&members).unwrap();

        assert_eq!(plan.paths, ["billing"]);
        assert_eq!(plan.message, message(&members));
    }

    #[test]
    fn no_member_with_a_new_commit_is_refused() {
        let members = [member("billing", Some("feat"), NEW, NEW)];

        assert_eq!(
            plan(&members),
            Err("No member has a new commit.".to_owned())
        );
    }

    #[test]
    fn a_detached_member_is_refused_and_named() {
        let mut detached = member("checkout", None, OTHER, OLD);
        detached.branch = None;
        let members = [member("billing", Some("feat"), NEW, OLD), detached];

        assert_eq!(
            plan(&members),
            Err("checkout is on a detached HEAD: check out its branch first.".to_owned())
        );
    }

    #[test]
    fn a_member_without_a_commit_is_refused() {
        let mut unborn = member("checkout", Some("feat"), OTHER, OLD);
        unborn.head = None;

        assert_eq!(
            plan(&[unborn]),
            Err("checkout has no commit yet.".to_owned())
        );
    }

    #[test]
    fn a_repository_without_members_is_refused() {
        assert_eq!(
            plan(&[]),
            Err("This repository has no submodule to record.".to_owned())
        );
    }
}

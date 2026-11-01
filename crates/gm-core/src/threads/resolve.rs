//! Where a stored reference points now (D145): a commit link or attachment after a rewrite, and
//! a file attachment after a rename. The stored ids are never changed. These functions decide
//! what a read shows, from the answers of `GitPort`, and say what to cache.

use std::path::Path;

use gm_schema::threads::CommitState;

use crate::GitPort;
use crate::port::{CommitPresence, Rewrite};

/// Commit references resolved by one read. Each one that is not reachable costs Git processes,
/// so the rest of a read's links show `unchecked` (D145).
pub const RESOLVE_LIMIT: usize = 50;
/// File attachments followed by one read of a label (D145).
pub const FOLLOW_LIMIT: usize = 50;
/// The cache value of a commit whose rewrite has more than one match.
pub const AMBIGUOUS: &str = "ambiguous";
/// The cache value of a commit that no rewrite explains.
pub const GONE: &str = "gone";

/// What a read shows for one commit reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The commit the reference resolves to, when it is not the stored id.
    pub resolved: Option<String>,
    pub state: CommitState,
    /// The value to cache in the store, or `None` to keep the cache as it is.
    pub cache: Option<String>,
}

fn answer(state: CommitState, resolved: Option<String>, cache: Option<&str>) -> Answer {
    Answer {
        resolved,
        state,
        cache: cache.map(str::to_owned),
    }
}

fn unchecked() -> Answer {
    answer(CommitState::Unchecked, None, None)
}

/// Resolves one commit reference (D145). `branch` is the branch the rewrite is looked for on (its
/// tip is the new tip), or `None` for HEAD. `cached` is what the store holds for the reference.
/// `budget` counts the references a read may resolve; once it is spent the answer is `unchecked`.
pub fn commit(
    git: &dyn GitPort,
    root: &Path,
    reference: &str,
    branch: Option<&str>,
    cached: Option<&str>,
    budget: &mut usize,
) -> Answer {
    if *budget == 0 {
        return unchecked();
    }
    *budget -= 1;
    match git.commit_presence(root, reference) {
        Err(_) => unchecked(),
        Ok(CommitPresence::Reachable) => answer(CommitState::Current, None, None),
        // A commit object the repository does not have can match nothing (D145).
        Ok(CommitPresence::Missing) => answer(CommitState::Gone, None, Some(GONE)),
        Ok(CommitPresence::Unreachable) => match cached {
            Some(value) => from_cache(value),
            None => search(git, root, reference, branch),
        },
    }
}

/// The answer a cached value gives: a commit id, or one of the two words.
fn from_cache(value: &str) -> Answer {
    match value {
        AMBIGUOUS => answer(CommitState::Ambiguous, None, None),
        GONE => answer(CommitState::Gone, None, None),
        id => answer(CommitState::Remapped, Some(id.to_owned()), None),
    }
}

/// Looks for the rewrite of an unreachable commit (D145) and caches what it finds.
fn search(git: &dyn GitPort, root: &Path, reference: &str, branch: Option<&str>) -> Answer {
    let Ok(candidates) = git.rewrite_candidates(root, branch) else {
        return unchecked();
    };
    let tip = branch
        .and_then(|name| git.branch_tip(root, name).ok().flatten())
        .or_else(|| git.resolve_commit(root, "HEAD").ok());
    let Some(tip) = tip else {
        return unchecked();
    };
    match git.rewritten_to(root, reference, &candidates, &tip) {
        Ok(Rewrite::Unique(id)) => {
            answer(CommitState::Remapped, Some(id.clone()), Some(id.as_str()))
        }
        Ok(Rewrite::Ambiguous) => answer(CommitState::Ambiguous, None, Some(AMBIGUOUS)),
        Ok(Rewrite::NoMatch) => answer(CommitState::Gone, None, Some(GONE)),
        // A range too long to compare, or a commit with no patch: nothing is cached, so the next
        // read asks again.
        Err(_) => unchecked(),
    }
}

/// The path a file has now, given the renames of the history to HEAD, oldest first (D145). Each
/// rename whose old name is the name followed moves it to the new one, so a chain of renames is
/// followed in order.
#[must_use]
pub fn follow(renames: &[(String, String)], path: &str) -> String {
    let mut current = path;
    for (old, new) in renames {
        if old == current {
            current = new.as_str();
        }
    }
    current.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(old, new)| ((*old).to_owned(), (*new).to_owned()))
            .collect()
    }

    #[test]
    fn a_file_follows_a_chain_of_renames_in_order() {
        let renames = pairs(&[("a.rs", "b.rs"), ("b.rs", "c.rs"), ("x.rs", "y.rs")]);
        assert_eq!(follow(&renames, "a.rs"), "c.rs");
        assert_eq!(follow(&renames, "x.rs"), "y.rs");
    }

    #[test]
    fn a_file_with_no_rename_keeps_its_path() {
        let renames = pairs(&[("a.rs", "b.rs")]);
        assert_eq!(follow(&renames, "other.rs"), "other.rs");
        assert_eq!(follow(&[], "a.rs"), "a.rs");
    }

    #[test]
    fn a_rename_back_to_an_earlier_name_is_followed_to_the_latest() {
        let renames = pairs(&[("a.rs", "b.rs"), ("b.rs", "a.rs"), ("a.rs", "z.rs")]);
        assert_eq!(follow(&renames, "a.rs"), "z.rs");
    }

    #[test]
    fn cached_words_and_ids_give_their_states() {
        assert_eq!(from_cache(AMBIGUOUS).state, CommitState::Ambiguous);
        assert_eq!(from_cache(GONE).state, CommitState::Gone);
        let id = "a".repeat(40);
        let found = from_cache(&id);
        assert_eq!(found.state, CommitState::Remapped);
        assert_eq!(found.resolved, Some(id));
        assert_eq!(found.cache, None);
    }
}

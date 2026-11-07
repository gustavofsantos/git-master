//! Contexts: named sets of repositories at the user level, and the memories attached to them
//! (design/threads.md, "Contexts"; D191, D193). `repo_id` names a clone, `visibility` decides which
//! contexts a repository sees (pure) and `store` keeps `contexts.db`.

pub mod repo_id;
pub mod store;
pub mod visibility;

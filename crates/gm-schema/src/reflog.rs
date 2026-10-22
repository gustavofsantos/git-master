use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How many entries one `LoadReflog` answers (B3, D95). The UI pages on from
/// the newest entries it holds.
pub const REFLOG_PAGE: usize = 200;

/// One entry of the reflog of HEAD (B3, D95): `HEAD@{index}`, newest first. The
/// entry records where HEAD was after a move; `oid` is that commit, whether or
/// not a branch still names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReflogEntry {
    pub index: u32,
    /// Full id of the commit HEAD moved to in this entry.
    pub oid: String,
    /// The text before the first `: ` of the reflog message (`commit`,
    /// `commit (amend)`, `checkout`, `reset`, `rebase (finish)`, `merge main`),
    /// or the whole message when it has no `: `.
    pub action: String,
    /// The text after the first `: ` of the reflog message, such as
    /// `moving from main to topic` or the commit subject; empty when none.
    pub detail: String,
    /// The subject of the commit `oid` points at; `None` when it is empty.
    pub subject: Option<String>,
    /// When the entry was written, in Unix seconds (not the commit time).
    pub time: i64,
}

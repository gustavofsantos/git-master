//! Types shared by the UI and the MCP server: commands going in,
//! events coming out, and the repository status they carry.

mod conflict;
mod diff;
pub mod features;
mod files;
mod gates;
mod grep;
mod history;
mod move_changes;
mod operation;
mod proposal;
mod reflog;
mod restack;
mod review;
mod stack;
mod stash;
mod status;
mod submodule;
mod syntax;
pub mod threads;
mod verdict;
mod workflow;
mod workspace;

pub use conflict::{
    ConflictFileRead, ConflictOperation, ConflictSide, ConflictSite, HunkOrigin, RecordedResolution,
};
pub use diff::{DiffLine, DiffSide, FileDiff, Hunk, HunkId, LineKind, StageTarget};
pub use features::{Feature, FeatureSet};
pub use files::{
    DirEntry, DirListing, FILE_HIT_LIMIT, FILE_VIEW_LIMIT, FileBody, FileHit, FileSearch, FileView,
};
pub use gates::{
    Bypass, GateDef, GateKind, GateLayer, GateOp, GateOutcome, GatesStatus, LayeredGate,
    MissingReason, MissingReviewer, When,
};
pub use grep::{
    GREP_FILE_LIMIT, GREP_LINE_LIMIT, GREP_TOTAL_LIMIT, GrepFile, GrepLine, GrepOptions,
    GrepResult, GrepScope,
};
pub use history::{
    CommitDetail, CommitFile, CommitSummary, DETAIL_FILE_LIMIT, HISTORY_PAGE, History,
    HistoryScope, RefKind, RefLabel,
};
pub use move_changes::{MoveBlocker, MoveOutcome, MovePreview, MoveTarget};
pub use operation::{
    Actor, Command, CommandKind, Event, IllegalTransition, MERGED_LIST_CHANGED, OperationId,
    OperationState, STASH_LIST_CHANGED, WorktreeMode,
};
pub use proposal::{Proposal, ProposalKind, ProposalStatus, ReviewerProposal};
pub use reflog::{REFLOG_PAGE, ReflogEntry};
pub use restack::{
    PlannedBranch, PlannedCommit, ReplayStep, RestackBlock, RestackMove, RestackPhase, RestackPlan,
    RestackRecord, RestackResolution,
};
pub use review::{
    ReviewAnchor, ReviewAuthorFilter, ReviewChange, ReviewComment, ReviewKind, ReviewLocation,
    ReviewReply, ReviewSeverity, ReviewStatus,
};
pub use stack::{OWN_COMMITS_LIMIT, Stack, StackBranch, StackSnapshot, StackState};
pub use stash::StashEntry;
pub use status::{
    Blocker, BranchEntry, BranchFailure, BranchInfo, Change, MergedBranch, PruneCandidate,
    RepoStatus, StatusEntry, SubmoduleChange, WorktreeEntry, WorktreeInspection,
};
pub use submodule::{SUBMODULE_COMMIT_LIMIT, SubmoduleCommit, SubmoduleDetail};
pub use syntax::{HighlightSpan, SyntaxRole};
pub use threads::{
    AttachKind, CanonicalEvent, CaptureGap, CutBound, EditChange, EditIntent, EditPhase, EditState,
    EntryKind, EntryRole, EventHeader, Evidence, FileChange, FileChangeState, HitKind, Label,
    LabelAttachment, LabelDetail, LineRun, LinkKind, LinkOrigin, Memory, MemoryFilter, MemoryKind,
    MemorySource, MemorySourceInput, MemorySourceKind, MemoryStatus, ProvenanceCommit,
    ProvenanceCut, ProvenanceEdit, ProvenanceResult, ProvenanceThread, SearchFacets, SearchGroup,
    SearchOrder, SessionOrigin, ThreadDetail, ThreadEdit, ThreadEntry, ThreadEntryBrief,
    ThreadFilter, ThreadHit, ThreadLink, ThreadRelKind, ThreadRelation, ThreadSummary, ToolKind,
    Unverifiable,
};
pub use verdict::{
    Approval, FindingInput, Level, Reviewer, ReviewerDirection, ReviewerFileError, ReviewerSignal,
    ReviewerSource, ReviewerSubject, Verdict, VerdictFinding,
};
pub use workflow::{
    WorkflowCallRecord, WorkflowDiagnostic, WorkflowDryCall, WorkflowDryRun, WorkflowEnd,
    WorkflowInfo, WorkflowInput, WorkflowRunDetail, WorkflowRunStatus, WorkflowRunSummary,
};
pub use workspace::{
    BranchHow, MemberPlan, MemberRequest, RemovalBlocker, RemovalBlockerKind, WorkspaceProblem,
    WorkspaceRow,
};

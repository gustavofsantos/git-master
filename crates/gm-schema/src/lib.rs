//! Types shared by the UI and the MCP server: commands going in,
//! events coming out, and the repository status they carry.

mod compare;
mod conflict;
mod diff;
pub mod features;
mod files;
mod grep;
mod health;
mod history;
mod move_changes;
mod operation;
mod reflog;
mod restack;
mod review;
mod scorecard;
mod stack;
mod stash;
mod status;
mod submodule;
mod syntax;
pub mod threads;
mod workflow;
mod workspace;

pub use compare::{RangeMode, RevisionRange};
pub use conflict::{
    ConflictFileRead, ConflictOperation, ConflictSide, ConflictSite, HunkOrigin, RecordedResolution,
};
pub use diff::{DiffLine, DiffSide, FileDiff, Hunk, HunkId, LineKind, StageTarget};
pub use features::{Feature, FeatureSet};
pub use files::{
    DirEntry, DirListing, FILE_HIT_LIMIT, FILE_VIEW_LIMIT, FileBody, FileHit, FileSearch, FileView,
};
pub use grep::{
    GREP_FILE_LIMIT, GREP_LINE_LIMIT, GREP_TOTAL_LIMIT, GrepFile, GrepLine, GrepOptions,
    GrepResult, GrepScope,
};
pub use health::{
    AGE_BUCKETS, CHURN_DAYS, ChurnSummary, Complexity, DormancyBreak, FileHealth, HealthProgress,
    HealthReport, LineChurn, Ownership, Partner, RemovedChurn, SPARK_BARS, SURVIVAL_AGES, Trend,
    age_bucket,
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
pub use reflog::{REFLOG_PAGE, ReflogEntry};
pub use restack::{
    PlannedBranch, PlannedCommit, ReplayStep, RestackBlock, RestackMove, RestackPhase, RestackPlan,
    RestackRecord, RestackResolution,
};
pub use review::{
    ReviewAnchor, ReviewAuthorFilter, ReviewChange, ReviewComment, ReviewKind, ReviewLocation,
    ReviewReply, ReviewSeverity, ReviewStatus,
};
pub use scorecard::{
    BODY_MAX_CHARS, COMMENT_BODY_MAX_CHARS, COMMENTS_MAX, CommentLabel, LIST_DEFAULT, LIST_MAX,
    NAME_MAX_CHARS, RETAINED_PER_NAME_AND_BRANCH, SUBJECT_MAX_CHARS, SUGGESTED_FIX_MAX_CHARS,
    Scorecard, ScorecardCommentInput, ScorecardStatus, ScorecardSummary, TREND_LEN,
    check_decorations, check_name, is_blocking, kind_of_label, resolve_kind, resolve_severity,
    severity_of,
};
pub use stack::{OWN_COMMITS_LIMIT, Stack, StackBranch, StackSnapshot, StackState};
pub use stash::StashEntry;
pub use status::{
    Blocker, BranchEntry, BranchFailure, BranchInfo, Change, ChangeStats, LineCounts, MergedBranch,
    PruneCandidate, RepoStatus, StatusEntry, SubmoduleChange, WorktreeEntry, WorktreeInspection,
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
pub use workflow::{
    WorkflowCallRecord, WorkflowDiagnostic, WorkflowDryCall, WorkflowDryRun, WorkflowEnd,
    WorkflowInfo, WorkflowInput, WorkflowLocation, WorkflowRole, WorkflowRunDetail,
    WorkflowRunStatus, WorkflowRunSummary, review_workflows,
};
pub use workspace::{
    BranchHow, MemberPlan, MemberRequest, RemovalBlocker, RemovalBlockerKind, WorkspaceProblem,
    WorkspaceRow,
};

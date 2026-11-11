//! The code health read (D225, docs/guide/code-health.md, "Building the index"): the index is
//! filled and the indicators computed on a thread of its own, as the provenance blame is (D144).
//! Its answer is dropped by `id` when the read was superseded.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use gm_schema::{FileHealth, HealthProgress, HealthReport, LineChurn, OperationId, RemovedChurn};

use crate::health::complexity::{self, MAX_BYTES, MEASURED_FILES};
use crate::health::ingest::{self, Tree};
use crate::health::store::HealthStore;
use crate::health::{CommitRecord, DAY_SECS, HealthConfig, churn, metrics};

/// The newest days of the window are indexed first, so the report of the recent commits comes
/// early; the rest of the window follows (see `HealthJob::run`).
const RECENT_DAYS: i64 = 183;
/// The error an ingest function returns when its stop flag is set.
const CANCELLED: &str = "cancelled";

/// The code health read of one commit and configuration, started by `LoadHealth`. It stops like
/// a search once `stop` is set.
pub struct HealthJob {
    pub id: OperationId,
    pub root: PathBuf,
    /// The index file (`health::store::db_path`).
    pub db: PathBuf,
    /// The full id of the commit the read is at.
    pub sha: String,
    /// The committer time of `sha`, in seconds.
    pub time: i64,
    pub config: HealthConfig,
    pub config_hash: String,
    pub stop: Arc<AtomicBool>,
    /// The instant after which the read stops reading history and answers with what the index
    /// holds, `complete` false (D229 (4)). `None` for no budget, as the app's own read has.
    pub deadline: Option<Instant>,
}

/// What a job reports while it runs.
pub enum HealthUpdate {
    /// The index holds this many commits of the window.
    Progress(HealthProgress),
    /// A report over the commits indexed so far (`complete` is false).
    Partial(HealthReport),
}

/// The end of a job: the report, or the reason it has none (an error or a cancellation).
pub struct HealthOutcome {
    pub id: OperationId,
    pub result: Result<HealthReport, String>,
}

impl HealthJob {
    /// Fills the index for the window in two phases, the recent commits first, and computes the
    /// report. `update` is called with each progress step and with a partial report when the
    /// older commits are still missing. A spent budget ends the read with a partial report; a
    /// stop from the engine ends it with an error.
    #[must_use]
    pub fn run(self, update: &mut dyn FnMut(HealthUpdate)) -> HealthOutcome {
        let result = self.compute(update);
        HealthOutcome {
            id: self.id,
            result,
        }
    }

    fn compute(&self, update: &mut dyn FnMut(HealthUpdate)) -> Result<HealthReport, String> {
        let mut store = HealthStore::open(&self.db)?;
        match self.index(&mut store, update) {
            Ok(()) => self.report(&store, true),
            Err(error) if error == CANCELLED && self.budget_spent() => self.report(&store, false),
            Err(error) => Err(error),
        }
    }

    /// Fills the index: the recent commits, the rest of the window, then the hunks of the
    /// first-parent window. Stops with `Err("cancelled")` when the stop flag is set.
    fn index(
        &self,
        store: &mut HealthStore,
        update: &mut dyn FnMut(HealthUpdate),
    ) -> Result<(), String> {
        let since = self.since();
        let recent = since.max(self.time - RECENT_DAYS * DAY_SECS);
        ingest::ingest(
            &self.root,
            store,
            &self.sha,
            recent,
            &self.stop,
            &mut |progress| {
                self.check_budget();
                update(HealthUpdate::Progress(progress));
            },
        )?;
        let full = ingest::window(&self.root, &self.sha, since)?;
        if !store.missing(&full)?.is_empty() {
            update(HealthUpdate::Partial(self.report(store, false)?));
            ingest::ingest(
                &self.root,
                store,
                &self.sha,
                since,
                &self.stop,
                &mut |progress| {
                    self.check_budget();
                    update(HealthUpdate::Progress(progress));
                },
            )?;
        }
        ingest::ingest_hunks(
            &self.root,
            store,
            &self.sha,
            since,
            &self.stop,
            &mut |progress| {
                self.check_budget();
                update(HealthUpdate::Progress(progress));
            },
        )?;
        Ok(())
    }

    /// The first instant of the window, in seconds.
    fn since(&self) -> i64 {
        self.time - i64::from(self.config.window_days) * DAY_SECS
    }

    /// Whether the budget of this read is spent.
    fn budget_spent(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// Sets the stop flag once the budget is spent. The ingest functions check it between
    /// batches, so the read ends there and is not treated as a cancellation.
    fn check_budget(&self) {
        if self.budget_spent() {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    /// The indicators of the commits of the window the index holds. Only files that exist at the
    /// commit are reported. `measure` reads the complexity of the hot files, unless the budget is
    /// spent: a partial answer does not read blobs past it.
    fn report(&self, store: &HealthStore, measure: bool) -> Result<HealthReport, String> {
        let window = ingest::window(&self.root, &self.sha, self.since())?;
        let commits = store.load(&window)?;
        let tree = ingest::tree(&self.root, &self.sha)?;
        let mut files = metrics::compute(&commits, &self.config, Some(&tree.paths));
        for file in &mut files {
            file.bytes = tree.blobs.get(&file.path).map_or(0, |blob| blob.bytes);
        }
        let mut removed_churn = Vec::new();
        if let Some(churn) = self.churn(store, &commits)? {
            for file in &mut files {
                file.churn = churn.get(&file.path).cloned();
            }
            removed_churn = removed(&churn, &tree.paths);
        }
        if measure && !self.budget_spent() {
            self.measure(&mut files, &tree)?;
        }
        for file in &mut files {
            let extra = metrics::measured_constraints(file);
            file.constraints.extend(extra);
        }
        Ok(HealthReport {
            rev: self.sha.clone(),
            config_hash: self.config_hash.clone(),
            complete: commits.len() == window.len(),
            files,
            removed_churn,
        })
    }

    /// The complexity of the hottest files that are text, at the commit (D229 (1)). Files are in
    /// rank order; the ones past `MEASURED_FILES` have none.
    fn measure(&self, files: &mut [FileHealth], tree: &Tree) -> Result<(), String> {
        let largest = u64::try_from(MAX_BYTES).unwrap_or(u64::MAX);
        let ids: Vec<Option<String>> = files
            .iter()
            .take(MEASURED_FILES)
            .map(|file| {
                tree.blobs
                    .get(&file.path)
                    .filter(|blob| blob.bytes <= largest)
                    .map(|blob| blob.id.clone())
            })
            .collect();
        let measured = complexity::read(&self.root, &ids, &self.stop)?;
        for (file, value) in files.iter_mut().zip(measured) {
            file.complexity = value;
        }
        Ok(())
    }

    /// The churn of each file, from the hunks of the first-parent window (D228). `None` until the
    /// index holds the hunks of every commit of that window, so a partial report has no churn.
    fn churn(
        &self,
        store: &HealthStore,
        commits: &[CommitRecord],
    ) -> Result<Option<HashMap<String, LineChurn>>, String> {
        let fp = ingest::first_parent_window(&self.root, &self.sha, self.since())?;
        if !store.missing_hunks(&fp)?.is_empty() {
            return Ok(None);
        }
        let mut hunks = store.load_hunks(&fp)?;
        hunks.reverse();
        let times: HashMap<&str, i64> = commits.iter().map(|c| (c.sha.as_str(), c.day())).collect();
        let mut kept = Vec::with_capacity(hunks.len());
        let mut days = Vec::with_capacity(hunks.len());
        for commit in hunks {
            if let Some(&day) = times.get(commit.sha.as_str()) {
                days.push(day);
                kept.push(commit);
            }
        }
        Ok(Some(churn::follow(&kept, &days, &self.config)))
    }
}

/// The churn of the files that the commit no longer holds, by path. A path with no churn is
/// left out (D229 (3)).
fn removed(churn: &HashMap<String, LineChurn>, live: &HashSet<String>) -> Vec<RemovedChurn> {
    let mut removed: Vec<RemovedChurn> = churn
        .iter()
        .filter(|(path, value)| !live.contains(path.as_str()) && **value != LineChurn::default())
        .map(|(path, value)| RemovedChurn {
            path: path.clone(),
            churn: value.clone(),
        })
        .collect();
    removed.sort_by(|a, b| a.path.cmp(&b.path));
    removed
}

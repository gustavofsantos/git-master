//! The code health read (D225, docs/guide/code-health.md, "Building the index"): the index is
//! filled and the indicators computed on a thread of its own, as the provenance blame is (D144).
//! Its answer is dropped by `id` when the read was superseded.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gm_schema::{HealthProgress, HealthReport, LineChurn, OperationId};

use crate::health::store::HealthStore;
use crate::health::{CommitRecord, DAY_SECS, HealthConfig, churn, ingest, metrics};

/// The newest days of the window are indexed first, so the report of the recent commits comes
/// early; the rest of the window follows (see `HealthJob::run`).
const RECENT_DAYS: i64 = 183;

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
    /// older commits are still missing.
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
        let since = self.since();
        let recent = since.max(self.time - RECENT_DAYS * DAY_SECS);
        ingest::ingest(
            &self.root,
            &mut store,
            &self.sha,
            recent,
            &self.stop,
            &mut |progress| {
                update(HealthUpdate::Progress(progress));
            },
        )?;
        let full = ingest::window(&self.root, &self.sha, since)?;
        if !store.missing(&full)?.is_empty() {
            update(HealthUpdate::Partial(self.report(&store)?));
            ingest::ingest(
                &self.root,
                &mut store,
                &self.sha,
                since,
                &self.stop,
                &mut |progress| {
                    update(HealthUpdate::Progress(progress));
                },
            )?;
        }
        ingest::ingest_hunks(
            &self.root,
            &mut store,
            &self.sha,
            since,
            &self.stop,
            &mut |progress| {
                update(HealthUpdate::Progress(progress));
            },
        )?;
        self.report(&store)
    }

    /// The first instant of the window, in seconds.
    fn since(&self) -> i64 {
        self.time - i64::from(self.config.window_days) * DAY_SECS
    }

    /// The indicators of the commits of the window the index holds. Only files that exist at the
    /// commit are reported.
    fn report(&self, store: &HealthStore) -> Result<HealthReport, String> {
        let window = ingest::window(&self.root, &self.sha, self.since())?;
        let commits = store.load(&window)?;
        let live = self.live_files()?;
        let mut files = metrics::compute(&commits, &self.config, Some(&live));
        if let Some(churn) = self.churn(store, &commits)? {
            for file in &mut files {
                file.churn = churn.get(&file.path).cloned();
            }
        }
        Ok(HealthReport {
            rev: self.sha.clone(),
            config_hash: self.config_hash.clone(),
            complete: commits.len() == window.len(),
            files,
            removed_churn: Vec::new(),
        })
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

    /// The paths of the files the commit holds (`git ls-tree`).
    fn live_files(&self) -> Result<HashSet<String>, String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["--no-optional-locks", "-c", "core.quotepath=off"])
            .args(["ls-tree", "-r", "-z", "--name-only", &self.sha])
            .env("LC_ALL", "C")
            .output()
            .map_err(|e| format!("cannot run git: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "cannot list the files of {}: {}",
                self.sha,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect())
    }
}

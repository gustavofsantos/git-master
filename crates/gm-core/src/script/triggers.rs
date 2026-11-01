//! Event triggers (design/workflow-scripts.md, `WF3a`, D155): which scripts an engine event starts,
//! and how the runs of one script wait while one is active (`concurrency`, D100). Pure: the engine
//! hands in what happened and the loaded scripts, and acts on the answer, so every rule has a test
//! without a repository or a window.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::meta::{Concurrency, Meta};

/// One engine event that can start a script. `on` is the trigger's name, such as `commit.created`.
#[derive(Clone, Debug, PartialEq)]
pub struct TriggerEvent {
    pub on: String,
    /// The branch the event concerns, which `match` looks at. `None` when there is none.
    pub branch: Option<String>,
    /// The fields the script reads from `input.trigger`, besides `on`.
    pub payload: Value,
}

impl TriggerEvent {
    /// `input.trigger`: the payload's fields, with `on` added.
    #[must_use]
    pub fn trigger_value(&self) -> Value {
        let mut fields = match &self.payload {
            Value::Object(fields) => fields.clone(),
            _ => Map::new(),
        };
        fields.insert("on".to_owned(), json!(self.on));
        Value::Object(fields)
    }
}

/// `repo.opened`: the repository opened, on `branch` when HEAD is on one.
#[must_use]
pub fn repo_opened(root: &str, branch: Option<String>) -> TriggerEvent {
    TriggerEvent {
        on: "repo.opened".to_owned(),
        branch,
        payload: json!({ "root": root }),
    }
}

/// `commit.created`: a commit finished on `branch`, `None` when HEAD is detached.
#[must_use]
pub fn commit_created(oid: &str, branch: Option<String>) -> TriggerEvent {
    TriggerEvent {
        on: "commit.created".to_owned(),
        payload: json!({ "oid": oid, "branch": branch }),
        branch,
    }
}

/// `checkout.done`: a checkout finished, with HEAD on `branch` (`None` when detached).
#[must_use]
pub fn checkout_done(branch: Option<String>, head: Option<&str>) -> TriggerEvent {
    TriggerEvent {
        on: "checkout.done".to_owned(),
        payload: json!({ "branch": branch, "head": head }),
        branch,
    }
}

/// `fetch.done`. A fetch runs on every remote, so `remote` is `all`.
#[must_use]
pub fn fetch_done(remote: &str) -> TriggerEvent {
    TriggerEvent {
        on: "fetch.done".to_owned(),
        branch: None,
        payload: json!({ "remote": remote }),
    }
}

/// `restack.done`: a restack moved `branches`, starting from `from`.
#[must_use]
pub fn restack_done(from: &str, branches: &[String]) -> TriggerEvent {
    TriggerEvent {
        on: "restack.done".to_owned(),
        branch: Some(from.to_owned()),
        payload: json!({ "from": from, "branches": branches }),
    }
}

/// `refs.changed`, once for a local branch whose tip moved.
#[must_use]
pub fn refs_changed(change: &RefChange) -> TriggerEvent {
    TriggerEvent {
        on: "refs.changed".to_owned(),
        branch: Some(change.branch.clone()),
        payload: json!({
            "ref": format!("refs/heads/{}", change.branch),
            "branch": change.branch,
            "old": change.old,
            "new": change.new,
        }),
    }
}

/// Whether the script with `meta` runs for `event`. Its trigger must name the same event, and its
/// `match` (when it has one) must hold for the event's branch. An event without a branch matches
/// only scripts without `match`. A manual workflow never matches, since no event is named `manual`.
#[must_use]
pub fn matches(meta: &Meta, event: &TriggerEvent) -> bool {
    if meta.trigger.on != event.on {
        return false;
    }
    match meta.trigger.filter.as_deref() {
        None => true,
        Some(rule) => match (rule.strip_prefix("branch:"), event.branch.as_deref()) {
            (Some(glob), Some(branch)) => glob_match(glob, branch),
            _ => false,
        },
    }
}

/// A local branch whose tip moved between two checks. `old` is `None` when the branch was created
/// and `new` is `None` when it was deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefChange {
    pub branch: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// The local branches whose tip differs between `before` and `after`, in name order. Both maps are
/// branch name to the full id of the tip.
#[must_use]
pub fn ref_changes(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<RefChange> {
    let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    names
        .into_iter()
        .filter_map(|name| {
            let (old, new) = (before.get(name), after.get(name));
            (old != new).then(|| RefChange {
                branch: name.clone(),
                old: old.cloned(),
                new: new.cloned(),
            })
        })
        .collect()
}

/// What the engine does with a trigger for a script that has runs in progress.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision<R, P> {
    /// Start a run with this payload now. The engine then reports the run with `started`, or the
    /// failure with `start_failed`.
    Start(P),
    /// Stop the active run `run`, which has not reached an applying call. The payload waits as the
    /// pending run.
    Cancel { run: R },
    /// The payload waits as the pending run.
    Queued,
    /// The trigger is dropped, because the script is active (`skip`).
    Dropped,
}

/// The runs of each script: at most one active run and at most one pending trigger per script, in
/// memory (D100). `R` is the run id and `P` what a pending trigger carries.
#[derive(Debug)]
pub struct Lanes<R, P> {
    lanes: BTreeMap<String, Lane<R, P>>,
}

#[derive(Debug)]
struct Lane<R, P> {
    active: Option<Active<R>>,
    pending: Option<P>,
}

#[derive(Debug)]
struct Active<R> {
    /// The run, once the engine has started it.
    run: Option<R>,
    /// Whether the run has reached an applying call. Such a run is never cancelled (D155(1)).
    applied: bool,
}

impl<R, P> Default for Lanes<R, P> {
    fn default() -> Self {
        Self {
            lanes: BTreeMap::new(),
        }
    }
}

impl<R: Copy + PartialEq, P> Lanes<R, P> {
    /// A trigger for the script `name`, whose `concurrency` is `mode`.
    pub fn on_trigger(&mut self, name: &str, mode: Concurrency, payload: P) -> Decision<R, P> {
        let lane = self.lanes.entry(name.to_owned()).or_insert(Lane {
            active: None,
            pending: None,
        });
        let busy = lane
            .active
            .as_ref()
            .map(|active| (active.run, active.applied));
        let Some((run, applied)) = busy else {
            lane.active = Some(Active {
                run: None,
                applied: false,
            });
            return Decision::Start(payload);
        };
        match mode {
            Concurrency::Skip => Decision::Dropped,
            Concurrency::Queue => {
                lane.pending = Some(payload);
                Decision::Queued
            }
            Concurrency::Replace => {
                lane.pending = Some(payload);
                match run {
                    Some(run) if !applied => Decision::Cancel { run },
                    _ => Decision::Queued,
                }
            }
        }
    }

    /// The engine started the run of the script `name`, which `on_trigger` or `finished` decided.
    pub fn started(&mut self, name: &str, run: R) {
        if let Some(active) = self
            .lanes
            .get_mut(name)
            .and_then(|lane| lane.active.as_mut())
        {
            active.run = Some(run);
        }
    }

    /// The run of the script `name` could not start. Nothing waits behind it.
    pub fn start_failed(&mut self, name: &str) {
        if let Some(lane) = self.lanes.get_mut(name) {
            lane.active = None;
            lane.pending = None;
        }
    }

    /// The run `run` reached an applying call.
    pub fn applied(&mut self, run: R) {
        if let Some(active) = self.active_of(run) {
            active.applied = true;
        }
    }

    /// The run `run` ended. Returns the script and its pending trigger, which the engine starts
    /// now; `None` when nothing waited. Returns `None` for a run that no lane holds.
    pub fn finished(&mut self, run: R) -> Option<(String, P)> {
        let name = self
            .lanes
            .iter()
            .find(|(_, lane)| lane.active.as_ref().is_some_and(|a| a.run == Some(run)))
            .map(|(name, _)| name.clone())?;
        let lane = self.lanes.get_mut(&name)?;
        if let Some(payload) = lane.pending.take() {
            lane.active = Some(Active {
                run: None,
                applied: false,
            });
            Some((name, payload))
        } else {
            lane.active = None;
            None
        }
    }

    /// Whether a run of the script `name` is active or about to start.
    #[must_use]
    pub fn busy(&self, name: &str) -> bool {
        self.lanes
            .get(name)
            .is_some_and(|lane| lane.active.is_some())
    }

    /// The pending trigger of the script `name`, if one waits.
    #[must_use]
    pub fn pending(&self, name: &str) -> Option<&P> {
        self.lanes.get(name)?.pending.as_ref()
    }

    fn active_of(&mut self, run: R) -> Option<&mut Active<R>> {
        self.lanes
            .values_mut()
            .filter_map(|lane| lane.active.as_mut())
            .find(|active| active.run == Some(run))
    }
}

/// Glob matching for `match`: `*` matches any run of characters, including
/// `/`, and `?` matches one character.
#[must_use]
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    // The last `*`, and the text position it currently absorbs up to.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::meta::{Trigger, parse};

    fn meta(on: &str, filter: Option<&str>, concurrency: Concurrency) -> Meta {
        Meta {
            name: "w".to_owned(),
            description: String::new(),
            trigger: Trigger {
                on: on.to_owned(),
                filter: filter.map(str::to_owned),
                key: None,
            },
            concurrency,
            inputs: Vec::new(),
        }
    }

    fn on_branch(on: &str, branch: Option<&str>) -> TriggerEvent {
        TriggerEvent {
            on: on.to_owned(),
            branch: branch.map(str::to_owned),
            payload: json!({}),
        }
    }

    #[test]
    fn a_trigger_without_match_runs_for_any_branch_and_none() {
        let m = meta("refs.changed", None, Concurrency::Queue);
        assert!(matches(&m, &on_branch("refs.changed", Some("main"))));
        assert!(matches(&m, &on_branch("refs.changed", Some("feat/x"))));
        assert!(matches(&m, &on_branch("refs.changed", None)));
    }

    #[test]
    fn match_holds_the_glob_for_the_event_branch() {
        let m = meta("refs.changed", Some("branch:feat/*"), Concurrency::Queue);
        assert!(matches(&m, &on_branch("refs.changed", Some("feat/x"))));
        assert!(matches(&m, &on_branch("refs.changed", Some("feat/x/y"))));
        assert!(!matches(&m, &on_branch("refs.changed", Some("main"))));
        assert!(!matches(&m, &on_branch("refs.changed", Some("feature"))));
    }

    #[test]
    fn an_event_without_a_branch_matches_only_scripts_without_match() {
        let filtered = meta("commit.created", Some("branch:feat/*"), Concurrency::Queue);
        assert!(!matches(&filtered, &on_branch("commit.created", None)));
        let open = meta("commit.created", None, Concurrency::Queue);
        assert!(matches(&open, &on_branch("commit.created", None)));
    }

    #[test]
    fn a_script_only_runs_for_its_own_event() {
        let m = meta("commit.created", None, Concurrency::Queue);
        assert!(!matches(&m, &on_branch("checkout.done", Some("main"))));
        let manual = meta("manual", None, Concurrency::Queue);
        assert!(!matches(&manual, &on_branch("repo.opened", None)));
        let phase = meta("before.commit", None, Concurrency::Queue);
        assert!(!matches(&phase, &on_branch("commit.created", None)));
    }

    #[test]
    fn a_match_parsed_from_meta_drives_the_matcher() {
        let value = json!({
            "name": "w",
            "trigger": { "on": "refs.changed", "match": "branch:feat/*" }
        });
        let parsed = parse(&value).unwrap();
        assert!(matches(&parsed, &on_branch("refs.changed", Some("feat/a"))));
        assert!(!matches(&parsed, &on_branch("refs.changed", Some("main"))));
    }

    #[test]
    fn trigger_value_adds_the_name_to_the_payload() {
        let event = commit_created("abc", Some("main".to_owned()));
        assert_eq!(
            event.trigger_value(),
            json!({ "on": "commit.created", "oid": "abc", "branch": "main" })
        );
        let bare = TriggerEvent {
            on: "fetch.done".to_owned(),
            branch: None,
            payload: Value::Null,
        };
        assert_eq!(bare.trigger_value(), json!({ "on": "fetch.done" }));
    }

    #[test]
    fn payloads_have_the_fields_of_the_spec() {
        assert_eq!(
            repo_opened("/r", Some("main".into())).trigger_value(),
            json!({ "on": "repo.opened", "root": "/r" })
        );
        assert_eq!(
            checkout_done(None, Some("abc")).trigger_value(),
            json!({ "on": "checkout.done", "branch": null, "head": "abc" })
        );
        assert_eq!(
            fetch_done("all").trigger_value(),
            json!({ "on": "fetch.done", "remote": "all" })
        );
        assert_eq!(
            restack_done("feat/a", &["feat/b".to_owned()]).trigger_value(),
            json!({ "on": "restack.done", "from": "feat/a", "branches": ["feat/b"] })
        );
        let change = RefChange {
            branch: "feat/x".into(),
            old: None,
            new: Some("n".into()),
        };
        assert_eq!(
            refs_changed(&change).trigger_value(),
            json!({
                "on": "refs.changed",
                "ref": "refs/heads/feat/x",
                "branch": "feat/x",
                "old": null,
                "new": "n"
            })
        );
    }

    fn tips(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, oid)| ((*name).to_owned(), (*oid).to_owned()))
            .collect()
    }

    #[test]
    fn ref_changes_report_created_moved_and_deleted_branches() {
        let before = tips(&[("main", "1"), ("gone", "2"), ("same", "3")]);
        let after = tips(&[("main", "4"), ("new", "5"), ("same", "3")]);
        assert_eq!(
            ref_changes(&before, &after),
            vec![
                RefChange {
                    branch: "gone".into(),
                    old: Some("2".into()),
                    new: None,
                },
                RefChange {
                    branch: "main".into(),
                    old: Some("1".into()),
                    new: Some("4".into()),
                },
                RefChange {
                    branch: "new".into(),
                    old: None,
                    new: Some("5".into()),
                },
            ]
        );
    }

    #[test]
    fn no_change_means_no_ref_change() {
        let same = tips(&[("main", "1")]);
        assert!(ref_changes(&same, &same).is_empty());
    }

    #[test]
    fn a_run_is_started_when_the_script_is_idle() {
        let mut lanes: Lanes<u64, &str> = Lanes::default();
        assert_eq!(
            lanes.on_trigger("w", Concurrency::Queue, "a"),
            Decision::Start("a")
        );
        assert!(lanes.busy("w"));
        lanes.started("w", 7);
        assert!(lanes.busy("w"));
    }

    #[test]
    fn queue_keeps_one_pending_payload_and_the_last_one_wins() {
        let mut lanes: Lanes<u64, &str> = Lanes::default();
        lanes.on_trigger("w", Concurrency::Queue, "a");
        lanes.started("w", 1);
        assert_eq!(
            lanes.on_trigger("w", Concurrency::Queue, "b"),
            Decision::Queued
        );
        assert_eq!(
            lanes.on_trigger("w", Concurrency::Queue, "c"),
            Decision::Queued
        );
        assert_eq!(lanes.pending("w"), Some(&"c"));
        assert_eq!(lanes.finished(1), Some(("w".to_owned(), "c")));
        assert_eq!(lanes.pending("w"), None);
        assert!(lanes.busy("w"), "the pending run is now the active one");
        lanes.started("w", 2);
        assert_eq!(lanes.finished(2), None);
        assert!(!lanes.busy("w"));
    }

    #[test]
    fn skip_drops_a_trigger_while_a_run_is_active() {
        let mut lanes: Lanes<u64, &str> = Lanes::default();
        lanes.on_trigger("w", Concurrency::Skip, "a");
        lanes.started("w", 1);
        assert_eq!(
            lanes.on_trigger("w", Concurrency::Skip, "b"),
            Decision::Dropped
        );
        assert_eq!(lanes.pending("w"), None);
        assert_eq!(lanes.finished(1), None);
        assert_eq!(
            lanes.on_trigger("w", Concurrency::Skip, "c"),
            Decision::Start("c"),
            "once the run ended, the next trigger starts"
        );
    }

    #[test]
    fn replace_cancels_a_run_that_has_not_applied() {
        let mut lanes: Lanes<u64, &str> = Lanes::default();
        lanes.on_trigger("w", Concurrency::Replace, "a");
        lanes.started("w", 1);
        assert_eq!(
            lanes.on_trigger("w", Concurrency::Replace, "b"),
            Decision::Cancel { run: 1 }
        );
        assert_eq!(lanes.pending("w"), Some(&"b"));
        assert_eq!(lanes.finished(1), Some(("w".to_owned(), "b")));
    }

    #[test]
    fn replace_queues_behind_a_run_that_has_applied() {
        let mut lanes: Lanes<u64, &str> = Lanes::default();
        lanes.on_trigger("w", Concurrency::Replace, "a");
        lanes.started("w", 1);
        lanes.applied(1);
        assert_eq!(
            lanes.on_trigger("w", Concurrency::Replace, "b"),
            Decision::Queued
        );
        assert_eq!(lanes.pending("w"), Some(&"b"));
        assert_eq!(lanes.finished(1), Some(("w".to_owned(), "b")));
    }

    #[test]
    fn replace_queues_when_the_run_id_is_not_known_yet() {
        let mut lanes: Lanes<u64, &str> = Lanes::default();
        lanes.on_trigger("w", Concurrency::Replace, "a");
        assert_eq!(
            lanes.on_trigger("w", Concurrency::Replace, "b"),
            Decision::Queued
        );
    }

    #[test]
    fn applied_marks_only_the_run_it_names() {
        let mut lanes: Lanes<u64, &str> = Lanes::default();
        lanes.on_trigger("w", Concurrency::Replace, "a");
        lanes.started("w", 1);
        lanes.applied(99);
        assert_eq!(
            lanes.on_trigger("w", Concurrency::Replace, "b"),
            Decision::Cancel { run: 1 }
        );
    }

    #[test]
    fn scripts_have_their_own_lanes() {
        let mut lanes: Lanes<u64, &str> = Lanes::default();
        lanes.on_trigger("a", Concurrency::Skip, "x");
        lanes.started("a", 1);
        assert_eq!(
            lanes.on_trigger("b", Concurrency::Skip, "y"),
            Decision::Start("y")
        );
    }

    #[test]
    fn a_failed_start_clears_the_lane() {
        let mut lanes: Lanes<u64, &str> = Lanes::default();
        lanes.on_trigger("w", Concurrency::Queue, "a");
        lanes.start_failed("w");
        assert!(!lanes.busy("w"));
        assert_eq!(
            lanes.on_trigger("w", Concurrency::Queue, "b"),
            Decision::Start("b")
        );
    }

    #[test]
    fn a_run_no_lane_holds_finishes_with_nothing() {
        let mut lanes: Lanes<u64, &str> = Lanes::default();
        assert_eq!(lanes.finished(5), None);
    }

    #[test]
    fn glob_matches_branch_names() {
        assert!(glob_match("feat/*", "feat/x"));
        assert!(glob_match("feat/*", "feat/x/y"));
        assert!(!glob_match("feat/*", "main"));
        assert!(!glob_match("feat/*", "feature"));
        assert!(glob_match("main", "main"));
        assert!(!glob_match("main", "main2"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
        assert!(glob_match("*fix*", "hotfix-12"));
        assert!(!glob_match("", "x"));
    }
}

//! `aida awaiting`'s idle-in-progress channel: In Progress specs whose derived
//! driver resolves to `idle` (no relay, drain, live agent or queued batch
//! covers them) and whose idle age exceeds a configurable threshold — "in
//! progress, but nothing is working it".
//!
//! Idle age is the time since the spec's last activity: the later of the
//! newest commit on a branch belonging to the spec (ahead of the default
//! branch) and its last status change (its last modification when no status
//! change is on record). The driver itself is NOT
//! re-derived here; the candidates are `aida ps`'s orphans, whose driver
//! [`crate::driver_view`] already resolved.
//!
//! The threshold is `[awaiting] idle_in_progress_hours` (default 24), read
//! from the project `.aida/config.toml` first and `~/.aida/config.toml`
//! second. It only tunes which rows are listed (policy, BUG-1723).
// trace:STORY-1680 | ai:claude

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aida_core::driver_registry::DerivedDriver;
use chrono::{DateTime, Utc};

use crate::PsOrphan;

/// Default `[awaiting] idle_in_progress_hours`.
// trace:STORY-1680 | ai:claude
pub(crate) const DEFAULT_IDLE_IN_PROGRESS_HOURS: u64 = 24;

/// Where an idle age was measured from.
// trace:STORY-1680 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum IdleAgeSource {
    /// The newest commit on the spec's branch, ahead of the default branch.
    BranchCommit,
    /// The spec's last status change.
    StatusChange,
    /// The spec's last modification — the fallback when its history records
    /// no status change at all.
    // trace:STORY-1680 | ai:claude
    #[serde(rename = "last-modified")]
    Modified,
}

impl IdleAgeSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            IdleAgeSource::BranchCommit => "branch-commit",
            IdleAgeSource::StatusChange => "status-change",
            IdleAgeSource::Modified => "last-modified",
        }
    }

    fn phrase(self) -> &'static str {
        match self {
            IdleAgeSource::BranchCommit => "its last branch commit",
            IdleAgeSource::StatusChange => "its last status change",
            IdleAgeSource::Modified => "its last modification",
        }
    }
}

/// One In Progress spec nothing is working, idle past the threshold.
// trace:STORY-1680 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct IdleInProgressItem {
    pub spec_id: String,
    pub title: String,
    /// Seconds since the activity named by `age_source`.
    pub idle_secs: i64,
    pub age_source: IdleAgeSource,
    /// The branch whose commit dated the age (`age_source = branch-commit`).
    pub branch: Option<String>,
    /// The threshold in force, so a reader can see why the row is listed.
    pub threshold_hours: u64,
}

impl IdleInProgressItem {
    /// `no commits or status change for 26h (dated from its last branch
    /// commit, story-1-work)`. Worded as activity, not "idle", so it does not
    /// read as contradicting `aida ps`'s shorter `idle` age (measured from
    /// the spec's last modification).
    // trace:STORY-1680 | ai:claude
    pub(crate) fn age_label(&self) -> String {
        let mut s = format!(
            "no commits or status change for {} (dated from {}",
            aida_core::driver_registry::short_age(self.idle_secs),
            self.age_source.phrase()
        );
        if let Some(b) = &self.branch {
            s.push_str(&format!(", {b}"));
        }
        s.push(')');
        s
    }
}

/// Everything the pure selection needs about one candidate spec.
// trace:STORY-1680 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct IdleCandidate {
    pub spec_id: String,
    pub title: String,
    pub driver: Option<DerivedDriver>,
    /// Newest commit (and its branch) on a branch belonging to the spec.
    pub branch_commit: Option<(String, DateTime<Utc>)>,
    pub status_changed_at: Option<DateTime<Utc>>,
    /// The spec's last modification; dates the age only when no status
    /// change is on record.
    pub modified_at: Option<DateTime<Utc>>,
}

/// PURE: keep the candidates whose driver is `idle` and whose idle age — time
/// since the later of the newest branch commit and the last status change —
/// exceeds `threshold_hours`, oldest first. A driven (`relay`/`drain`/
/// `agent`) or `queued` candidate is never listed, whatever its age; nor is
/// one whose age cannot be measured at all.
// trace:STORY-1680 | ai:claude
pub(crate) fn select_idle(
    candidates: Vec<IdleCandidate>,
    now: DateTime<Utc>,
    threshold_hours: u64,
) -> Vec<IdleInProgressItem> {
    let threshold_secs = i64::try_from(threshold_hours.saturating_mul(3600)).unwrap_or(i64::MAX);
    let mut out: Vec<IdleInProgressItem> = candidates
        .into_iter()
        .filter(|c| matches!(c.driver, Some(DerivedDriver::Idle { .. })))
        .filter_map(|c| {
            // Idle age runs from the LAST activity: whichever of the newest
            // branch commit and the last status change is later. An old
            // branch (e.g. an earlier, squash-merged slice) must not make a
            // spec just sent back to In Progress read as long idle.
            // trace:STORY-1680 | ai:claude
            let status = c
                .status_changed_at
                .map(|s| (s, IdleAgeSource::StatusChange))
                .or(c.modified_at.map(|m| (m, IdleAgeSource::Modified)));
            let (at, source, branch) = match (c.branch_commit, status) {
                (Some((_, b)), Some((s, src))) if s > b => (s, src, None),
                (Some((branch, b)), _) => (b, IdleAgeSource::BranchCommit, Some(branch)),
                (None, s) => {
                    let (s, src) = s?;
                    (s, src, None)
                }
            };
            let idle_secs = now.signed_duration_since(at).num_seconds().max(0);
            (idle_secs > threshold_secs).then_some(IdleInProgressItem {
                spec_id: c.spec_id,
                title: c.title,
                idle_secs,
                age_source: source,
                branch,
                threshold_hours,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.idle_secs
            .cmp(&a.idle_secs)
            .then_with(|| a.spec_id.cmp(&b.spec_id))
    });
    out
}

/// PURE: `[awaiting] idle_in_progress_hours` from one config body. A
/// non-positive or non-integer value is ignored (falls through).
// trace:STORY-1680 | ai:claude
pub(crate) fn threshold_from_toml(body: &str) -> Option<u64> {
    let v: toml::Value = body.parse().ok()?;
    let raw = v.get("awaiting")?.get("idle_in_progress_hours")?;
    raw.as_integer()
        .and_then(|n| u64::try_from(n).ok())
        .filter(|n| *n > 0)
}

fn config_file(dir: &Path) -> PathBuf {
    dir.join(".aida").join("config.toml")
}

/// The threshold in force: the project config wins, then `~/.aida`, then
/// [`DEFAULT_IDLE_IN_PROGRESS_HOURS`].
// trace:STORY-1680 | ai:claude
pub(crate) fn idle_threshold_hours(project_root: &Path, home: Option<&Path>) -> u64 {
    [Some(project_root), home]
        .into_iter()
        .flatten()
        .find_map(|dir| {
            std::fs::read_to_string(config_file(dir))
                .ok()
                .and_then(|body| threshold_from_toml(&body))
        })
        .unwrap_or(DEFAULT_IDLE_IN_PROGRESS_HOURS)
}

/// Latest status-change timestamp in a spec's history.
// trace:STORY-1680 | ai:claude
pub(crate) fn last_status_change(req: &aida_core::Requirement) -> Option<DateTime<Utc>> {
    req.history
        .iter()
        .filter(|h| h.changes.iter().any(|c| c.field_name == "status"))
        .map(|h| h.timestamp)
        .max()
}

fn git_stdout(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Upper-cased spec id → local branches named for it (`story-1680-work`).
fn branches_by_spec(root: &Path) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let listing = git_stdout(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .unwrap_or_default();
    for branch in listing.lines().map(str::trim).filter(|b| !b.is_empty()) {
        if let Some(spec) = crate::worktree_lease::spec_id_from_branch(branch) {
            out.entry(spec).or_default().push(branch.to_string());
        }
    }
    out
}

/// The default branch commits are measured against: `origin/main` when it
/// resolves, else `main`.
fn base_ref(root: &Path) -> &'static str {
    if git_stdout(root, &["rev-parse", "--verify", "-q", "origin/main"]).is_some() {
        "origin/main"
    } else {
        "main"
    }
}

/// Newest commit on `branch` that the base does not already contain.
fn newest_commit_ahead(root: &Path, branch: &str, base: &str) -> Option<DateTime<Utc>> {
    let out = git_stdout(
        root,
        &[
            "log",
            "-1",
            "--format=%cI",
            "--end-of-options",
            branch,
            &format!("^{base}"),
        ],
    )?;
    DateTime::parse_from_rfc3339(out.trim())
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// The idle-in-progress rows for the full `aida awaiting` report, from the
/// orphans `aida ps` already derived (driver included). A spec's branch is
/// its stale lease's branch plus every local branch named for it; the newest
/// commit ahead of the default branch across them dates the age. Specs a
/// live fan-out plausibly builds (`likely_fanout`) or a subagent may be
/// working (`possibly_subagent`) are skipped — see [`idle_orphans`].
// trace:STORY-1680 | ai:claude
pub(crate) fn collect_idle_in_progress(
    project_root: &Path,
    backend: &aida_core::CachedGitBackend,
    orphans: &[PsOrphan],
    now: DateTime<Utc>,
) -> Vec<IdleInProgressItem> {
    use aida_core::db::DatabaseBackend as _;
    let idle = idle_orphans(orphans);
    if idle.is_empty() {
        return Vec::new();
    }
    let threshold = idle_threshold_hours(project_root, crate::aida_home_dir().as_deref());
    let leases = crate::list_leases(project_root);
    let named = branches_by_spec(project_root);
    let base = base_ref(project_root);
    let candidates = idle
        .into_iter()
        .map(|o| {
            let lease_branch =
                crate::driver_view::lease_for_spec(&leases, &[o.spec.as_str()], |_| false)
                    .map(|(l, _)| l.branch.as_str());
            let branch_commit = spec_branches(&named, &o.spec, lease_branch)
                .into_iter()
                .filter_map(|b| newest_commit_ahead(project_root, &b, base).map(|t| (b, t)))
                .max_by_key(|(_, t)| *t);
            let req = backend.get_requirement_by_spec_id(&o.spec).ok().flatten();
            IdleCandidate {
                spec_id: o.spec.clone(),
                title: o.title.clone(),
                driver: o.driver.clone(),
                branch_commit,
                status_changed_at: req.as_ref().and_then(last_status_change),
                modified_at: req.as_ref().map(|r| r.modified_at),
            }
        })
        .collect();
    select_idle(candidates, now, threshold)
}

/// PURE: the orphans that may be listed as idle — driver `idle`, and neither
/// a likely live fan-out (`likely_fanout`) nor possibly worked by a subagent
/// (`possibly_subagent`, BUG-1656). A possibly-subagent spec stays on the
/// orphan channel, whose wording says it may be being worked; a likely
/// fan-out is not listed by `aida awaiting` at all (only `aida ps`).
// trace:STORY-1680 | ai:claude
pub(crate) fn idle_orphans(orphans: &[PsOrphan]) -> Vec<&PsOrphan> {
    orphans
        .iter()
        .filter(|o| !o.likely_fanout && !o.possibly_subagent)
        .filter(|o| matches!(o.driver, Some(DerivedDriver::Idle { .. })))
        .collect()
}

/// PURE: the branches that belong to `spec` — every local branch named for
/// it plus its best lease's branch (when set and not already named).
// trace:STORY-1680 | ai:claude
pub(crate) fn spec_branches(
    named: &HashMap<String, Vec<String>>,
    spec: &str,
    lease_branch: Option<&str>,
) -> Vec<String> {
    let mut branches = named
        .get(&spec.to_ascii_uppercase())
        .cloned()
        .unwrap_or_default();
    if let Some(b) = lease_branch.map(str::trim).filter(|b| !b.is_empty()) {
        if !branches.iter().any(|x| x == b) {
            branches.push(b.to_string());
        }
    }
    branches
}

/// PURE: drop the orphans already reported as idle, so a spec appears in
/// `aida awaiting` once — under idle-in-progress, not also as orphaned.
// trace:STORY-1680 | ai:claude
pub(crate) fn orphans_not_idle(
    orphans: Vec<PsOrphan>,
    idle: &[IdleInProgressItem],
) -> Vec<PsOrphan> {
    let idle_specs: std::collections::HashSet<String> = idle
        .iter()
        .map(|i| i.spec_id.to_ascii_uppercase())
        .collect();
    orphans
        .into_iter()
        .filter(|o| !idle_specs.contains(&o.spec.to_ascii_uppercase()))
        .collect()
}

/// TOON rows for agent-mode `idle_in_progress`.
// trace:STORY-1680 | ai:claude
pub(crate) fn toon_rows(items: &[IdleInProgressItem]) -> Vec<Vec<String>> {
    items
        .iter()
        .map(|i| {
            vec![
                i.spec_id.clone(),
                aida_core::driver_registry::short_age(i.idle_secs),
                i.age_source.as_str().to_string(),
                i.branch.clone().unwrap_or_else(|| "-".into()),
                i.threshold_hours.to_string(),
            ]
        })
        .collect()
}

/// JSON rows for `aida awaiting --json`.
// trace:STORY-1680 | ai:claude
pub(crate) fn json_rows(items: &[IdleInProgressItem]) -> Vec<serde_json::Value> {
    items
        .iter()
        .map(|i| {
            serde_json::json!({
                "spec_id": i.spec_id,
                "title": i.title,
                "driver_kind": "idle",
                "idle_secs": i.idle_secs,
                "idle_age": aida_core::driver_registry::short_age(i.idle_secs),
                "age_source": i.age_source.as_str(),
                "branch": i.branch,
                "threshold_hours": i.threshold_hours,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/story_1680_idle_in_progress_tests.rs"]
mod story_1680_idle_in_progress_tests;

//! Per-job run ledger for `[schedule]` jobs (STORY-1226, ADR-46 decision 2).
//!
//! One YAML store object per job at `schedule/<job>.yaml` under the
//! `aida-store` worktree — like specs, it survives event-log rotation and
//! syncs across hubs, so "who last triaged the mailbox and when" (SPIKE-82)
//! has one answer on every clone. Writes go through a CAS push-wins loop
//! copied from `aida_core::alias::link_cas`; a clone with no attached store
//! (or no git at all — tests) writes the file locally and stops there.
//!
//! High-frequency substrate jobs would otherwise commit on every tick, so
//! writes are **debounced**: a write whose only change is `last_run` moving
//! by less than [`DEBOUNCE_SECS`] with the same `result` is skipped entirely
//! (no dirty worktree, no commit).
//!
//! trace:STORY-1226 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Directory under the store worktree root that holds one file per job.
pub(crate) const LEDGER_DIR: &str = "schedule";

/// Minimum spacing between two ledger commits for a job whose only change is
/// `last_run` (result unchanged).
pub(crate) const DEBOUNCE_SECS: i64 = 60;

const MAX_RETRIES: u32 = 10;

/// Keep enough immutable trips to correlate a routed event after newer runs,
/// while placing a deterministic ceiling on the git-canonical ledger.
// trace:BUG-1573 | ai:codex
pub(crate) const MAX_FAILURE_TRIPS: usize = 20;

/// Allow-listed performance evidence emitted by `doctor check performance`.
/// Percentages use thousandths of one percent so the persisted/event contract
/// stays integer-typed and deterministic across serializers.
// trace:BUG-1573 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PerformanceAudit {
    pub command: String,
    pub budget_ms: u64,
    pub over_budget: usize,
    pub denominator: usize,
    pub proportion_millipercent: u32,
    pub tolerated_millipercent: u32,
    pub window_hours: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worst_ms: Option<u64>,
    #[serde(default)]
    pub excluded_samples: usize,
    #[serde(default)]
    pub lineage_scoped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FailureTrip {
    pub trip_id: String,
    pub at: DateTime<Utc>,
    pub status: i32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub performance: Vec<PerformanceAudit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_error: Option<String>,
}

/// Who reported a run: the seat that acted, the session id if known, and the
/// vendor (`claude` / `codex` / `antigravity` / `tick`).
// trace:STORY-1226 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LastBy {
    pub seat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
}

/// One once-until-cleared firing of a condition (`when`) job.
// trace:TASK-1281 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Episode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared_at: Option<DateTime<Utc>>,
}

impl Episode {
    /// Open = fired and not yet cleared: the job must not fire again.
    pub(crate) fn is_open(&self) -> bool {
        self.cleared_at.is_none()
    }
}

/// The store object for one job.
// trace:STORY-1226 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JobLedger {
    pub job: String,
    /// Last time the job ran (substrate) or was reported done (seat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_by: Option<LastBy>,
    /// `ok` / `failed:<code>` for substrate runs, `done` for seat reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Immutable, event-correlated failure evidence; oldest trips fall off at
    /// [`MAX_FAILURE_TRIPS`]. Old ledgers deserialize with an empty history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure_trips: Vec<FailureTrip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Seat jobs: set when a trigger (event / condition / interval) marked the
    /// job due; cleared by `aida schedule done`. Substrate jobs never set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_since: Option<DateTime<Utc>>,
    /// Why the job is due, e.g. `every 30m`, `on PrMerged`, `when …`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_reason: Option<String>,
    /// Condition jobs: the current / most recent episode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<Episode>,
    /// Reserved for STORY-1218's tick: when a headless seat was last cold-booted
    /// for this job (the ≤1/hour/seat rate limit reads it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cold_boot_at: Option<DateTime<Utc>>,
}

impl JobLedger {
    pub(crate) fn new(job: &str) -> Self {
        Self {
            job: job.to_string(),
            ..Default::default()
        }
    }
}

/// Outcome of feeding one predicate evaluation into a job's episode state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EpisodeTransition {
    /// Predicate turned true with no open episode — the job fires now.
    Fired,
    /// Predicate turned false while an episode was open — cleared; the job may
    /// fire again on the next true.
    Cleared,
    /// No change (still true inside an open episode, or still false).
    Unchanged,
}

/// Once-until-cleared: fire when the predicate turns true and there is no
/// open episode; clear when it turns false; otherwise leave the ledger alone.
// trace:STORY-1226 | ai:claude
pub(crate) fn apply_condition(
    ledger: &mut JobLedger,
    is_true: bool,
    now: DateTime<Utc>,
) -> EpisodeTransition {
    match (&mut ledger.episode, is_true) {
        (Some(ep), true) if ep.is_open() => EpisodeTransition::Unchanged,
        (_, true) => {
            ledger.episode = Some(Episode {
                fired_at: Some(now),
                cleared_at: None,
            });
            EpisodeTransition::Fired
        }
        (Some(ep), false) if ep.is_open() => {
            ep.cleared_at = Some(now);
            EpisodeTransition::Cleared
        }
        (_, false) => EpisodeTransition::Unchanged,
    }
}

/// File-system-safe form of a job name (`schedule/<name>.yaml`).
fn sanitize(job: &str) -> String {
    job.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `schedule/<job>.yaml` under the store worktree root.
pub(crate) fn ledger_path(store_root: &Path, job: &str) -> PathBuf {
    store_root
        .join(LEDGER_DIR)
        .join(format!("{}.yaml", sanitize(job)))
}

/// Path relative to the store root, for `git add`.
fn ledger_rel(job: &str) -> String {
    format!("{LEDGER_DIR}/{}.yaml", sanitize(job))
}

/// Read one job's ledger. Missing / unreadable / malformed → `None` (a ledger
/// that cannot be read is "never ran", never an error).
// trace:STORY-1226 | ai:claude
pub(crate) fn load(store_root: &Path, job: &str) -> Option<JobLedger> {
    let text = std::fs::read_to_string(ledger_path(store_root, job)).ok()?;
    serde_yaml::from_str(&text).ok()
}

/// Read every job ledger under the store. File-only, never touches git.
// trace:STORY-1226 | ai:claude
pub(crate) fn load_all(store_root: &Path) -> BTreeMap<String, JobLedger> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(store_root.join(LEDGER_DIR)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(ledger) = serde_yaml::from_str::<JobLedger>(&text) {
            out.insert(ledger.job.clone(), ledger);
        }
    }
    out
}

/// Write one ledger file (no git). The caller owns add/commit/push.
// trace:STORY-1226 | ai:claude
pub(crate) fn save(store_root: &Path, ledger: &JobLedger) -> Result<()> {
    let path = ledger_path(store_root, &ledger.job);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let yaml = serde_yaml::to_string(ledger).context("serializing schedule ledger")?;
    aida_core::write_atomic(&path, yaml.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Debounce rule: skip the write when the ONLY change is `last_run` moving by
/// less than [`DEBOUNCE_SECS`] (result and everything else identical). A
/// first write, a result change, a due/episode change, or a ≥60 s move always
/// writes.
// trace:STORY-1226 | ai:claude
pub(crate) fn should_write(prev: Option<&JobLedger>, next: &JobLedger) -> bool {
    let Some(prev) = prev else {
        // A first write that records nothing (e.g. a `when` job whose
        // predicate has never turned true) is not worth a store commit.
        return *next != JobLedger::new(&next.job);
    };
    if prev == next {
        return false;
    }
    if prev.last_by.as_ref().map(|by| &by.seat) != next.last_by.as_ref().map(|by| &by.seat) {
        // A different seat reporting the same result is durable provenance.
        // trace:TASK-1281 | ai:codex
        return true;
    }
    let mut same_clock = next.clone();
    same_clock.last_run = prev.last_run;
    same_clock.last_by = prev.last_by.clone();
    if &same_clock != prev {
        // Something other than the run clock / same-seat reporter changed.
        return true;
    }
    match (prev.last_run, next.last_run) {
        (Some(a), Some(b)) => (b - a).abs() >= Duration::seconds(DEBOUNCE_SECS),
        _ => true,
    }
}

/// Is `store_root` a git checkout (worktree `.git` file or repo `.git` dir)?
fn is_git_checkout(store_root: &Path) -> bool {
    store_root.join(".git").exists()
}

/// Mutate one job's ledger and persist it with a CAS push-wins loop (copied
/// from `alias::link_cas`): pull → load → mutate → debounce → save → add →
/// commit → push; on a rejected push, drop the stale commit and retry. Solo
/// (no `origin`) commits locally; a store root that is not a git checkout
/// just writes the file. Returns whether anything was written.
///
/// The pull leg is `pull_rebase_auto_merge`, so two clones that both wrote
/// the same job's ledger resolve last-writer-wins instead of wedging the
/// store. A push that FAILS (offline) is not an error: the commit stays local
/// and the next `aida push` / `db sync` carries it.
// trace:STORY-1226 | ai:claude
pub(crate) fn write_cas(
    store_root: &Path,
    job: &str,
    mutate: impl FnMut(&mut JobLedger),
) -> Result<bool> {
    write_cas_opts(store_root, job, true, mutate)
}

/// [`write_cas`] with an explicit `push` switch. The per-turn hook tick runs
/// under a few-second budget, so it commits locally (`push = false`) and lets
/// the next sync upload; every other writer pushes.
// trace:STORY-1226 | ai:claude
pub(crate) fn write_cas_opts(
    store_root: &Path,
    job: &str,
    push: bool,
    mut mutate: impl FnMut(&mut JobLedger),
) -> Result<bool> {
    use aida_core::git_ops;

    if !is_git_checkout(store_root) {
        let prev = load(store_root, job);
        let mut next = prev.clone().unwrap_or_else(|| JobLedger::new(job));
        mutate(&mut next);
        if !should_write(prev.as_ref(), &next) {
            return Ok(false);
        }
        save(store_root, &next)?;
        return Ok(true);
    }

    // Advisor review round 1 (#1946): never commit a ledger onto a store that
    // is mid-rebase or detached — the BUG-1229 data-loss class, now reachable
    // from a per-turn hook tick. Skip the write with a note; the next tick
    // retries once the store is repaired. trace:STORY-1226 | ai:claude
    if let Err(err) = git_ops::ensure_store_write_safe(store_root) {
        eprintln!("  schedule ledger for '{job}' not written this tick: {err}");
        return Ok(false);
    }
    let branch = git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    let local_only = !push || !git_ops::has_remote(store_root, "origin");

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 && !local_only {
            git_ops::pull_rebase_auto_merge(store_root, "origin", &branch)?;
        }
        let prev = load(store_root, job);
        let mut next = prev.clone().unwrap_or_else(|| JobLedger::new(job));
        mutate(&mut next);
        if !should_write(prev.as_ref(), &next) {
            return Ok(false);
        }
        save(store_root, &next)?;
        let rel = ledger_rel(job);
        git_ops::add(store_root, &[rel.as_str()])?;
        let msg = format!(
            "schedule: {} {}",
            job,
            next.result.as_deref().unwrap_or("update")
        );
        if !git_ops::commit(store_root, &msg)? {
            // Nothing staged (identical bytes) — treat as written.
            return Ok(true);
        }
        if local_only {
            return Ok(true);
        }
        match git_ops::push(store_root, "origin", &branch) {
            Ok(true) => return Ok(true),
            Ok(false) => {
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(store_root)
                    .output();
                continue;
            }
            Err(e) => {
                // Offline / remote unreachable: the commit is local and uploads
                // on the next sync. Never fail a scheduled run over it.
                eprintln!(
                    "note: schedule ledger for '{job}' committed locally; push failed ({e}) — \
                     it uploads on the next `aida push`"
                );
                return Ok(true);
            }
        }
    }
    anyhow::bail!(
        "could not write the schedule ledger for '{job}' after {MAX_RETRIES} attempts \
         (store push kept being rejected) — run `aida db sync --pull` and retry"
    )
}

/// Three-way field merge for a batched ledger write: a field the tick changed
/// (`next` differs from `base`) takes the tick's value; every other field keeps
/// whatever is on disk NOW (`fresh`), which may be a concurrent writer's update
/// made since the tick loaded `base`. Pure.
// trace:TASK-1280 | ai:claude
pub(crate) fn reapply_delta(
    base: Option<&JobLedger>,
    next: &JobLedger,
    fresh: Option<&JobLedger>,
) -> JobLedger {
    let Some(fresh) = fresh else {
        return next.clone();
    };
    let base = base.cloned().unwrap_or_else(|| JobLedger::new(&next.job));
    let mut out = fresh.clone();
    macro_rules! take_if_changed {
        ($($field:ident),* $(,)?) => {$(
            if next.$field != base.$field {
                out.$field = next.$field.clone();
            }
        )*};
    }
    take_if_changed!(
        last_run,
        last_by,
        result,
        note,
        due_since,
        due_reason,
        episode,
        cold_boot_at
    );
    out
}

/// Persist all ledger changes produced by one scheduler tick in one store
/// commit. The per-job debounce is still evaluated independently; only the
/// surviving files are staged.
// trace:TASK-1280 | ai:codex
pub(crate) fn write_batch_cas_opts(
    store_root: &Path,
    base: &BTreeMap<String, JobLedger>,
    next: &BTreeMap<String, JobLedger>,
    push: bool,
) -> Result<usize> {
    use aida_core::git_ops;

    // Review round 1 on #1962: never save the pre-tick snapshot wholesale. On
    // every attempt (including after a rejected push + rebase) re-apply only
    // the fields THIS tick changed (next vs base) onto the freshly loaded
    // ledger, so a concurrent writer's change to another field (a seat's
    // `schedule done`, a cold-boot stamp) is preserved. trace:TASK-1280 | ai:claude
    let changed = |current: &BTreeMap<String, JobLedger>| {
        next.iter()
            .map(|(job, ledger)| {
                let merged = reapply_delta(base.get(job), ledger, current.get(job));
                (job.clone(), merged)
            })
            .filter(|(job, merged)| should_write(current.get(job), merged))
            .collect::<Vec<_>>()
    };

    if !is_git_checkout(store_root) {
        let writes = changed(&load_all(store_root));
        for (_, ledger) in &writes {
            save(store_root, ledger)?;
        }
        return Ok(writes.len());
    }
    if let Err(err) = git_ops::ensure_store_write_safe(store_root) {
        eprintln!("  schedule ledgers not written this tick: {err}");
        return Ok(0);
    }
    let branch = git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    let local_only = !push || !git_ops::has_remote(store_root, "origin");

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 && !local_only {
            git_ops::pull_rebase_auto_merge(store_root, "origin", &branch)?;
        }
        let writes = changed(&load_all(store_root));
        if writes.is_empty() {
            return Ok(0);
        }
        for (_, ledger) in &writes {
            save(store_root, ledger)?;
        }
        let rels = writes
            .iter()
            .map(|(job, _)| ledger_rel(job))
            .collect::<Vec<_>>();
        let refs = rels.iter().map(String::as_str).collect::<Vec<_>>();
        git_ops::add(store_root, &refs)?;
        let msg = format!("schedule: tick ({} jobs)", writes.len());
        if !git_ops::commit(store_root, &msg)? || local_only {
            return Ok(writes.len());
        }
        match git_ops::push(store_root, "origin", &branch) {
            Ok(true) => return Ok(writes.len()),
            Ok(false) => {
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(store_root)
                    .output();
            }
            Err(e) => {
                eprintln!(
                    "note: schedule tick committed locally; push failed ({e}) — it uploads on the next `aida push`"
                );
                return Ok(writes.len());
            }
        }
    }
    anyhow::bail!(
        "could not write the schedule tick after {MAX_RETRIES} attempts (store push kept being rejected)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(min: u32, sec: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 18, 10, min, sec).unwrap()
    }

    #[test]
    fn roundtrip_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = JobLedger {
            job: "mailbox-triage".into(),
            last_run: Some(at(1, 0)),
            last_by: Some(LastBy {
                seat: "advisor".into(),
                session: Some("sess-1".into()),
                vendor: Some("claude".into()),
            }),
            result: Some("done".into()),
            failure_trips: Vec::new(),
            note: Some("manual".into()),
            due_since: None,
            due_reason: None,
            episode: Some(Episode {
                fired_at: Some(at(0, 0)),
                cleared_at: Some(at(0, 30)),
            }),
            cold_boot_at: None,
        };
        assert!(write_cas(dir.path(), "mailbox-triage", |l| *l = ledger.clone()).unwrap());
        assert!(dir.path().join("schedule/mailbox-triage.yaml").exists());
        assert_eq!(load(dir.path(), "mailbox-triage").unwrap(), ledger);
        let all = load_all(dir.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all["mailbox-triage"], ledger);
        // Absent ledger is None, never an error.
        assert!(load(dir.path(), "nope").is_none());
    }

    #[test]
    fn debounce_skips_noise() {
        let base = JobLedger {
            job: "session-reap".into(),
            last_run: Some(at(0, 0)),
            result: Some("ok".into()),
            ..Default::default()
        };
        // First write lands — unless it records nothing at all.
        assert!(should_write(None, &base));
        assert!(!should_write(None, &JobLedger::new("mailbox-latency")));
        // Identical → no write.
        assert!(!should_write(Some(&base), &base));
        // Only last_run moved, by 30 s, same result → skipped.
        let mut noise = base.clone();
        noise.last_run = Some(at(0, 30));
        assert!(!should_write(Some(&base), &noise));
        // 60 s or more → written.
        let mut later = base.clone();
        later.last_run = Some(at(1, 0));
        assert!(should_write(Some(&base), &later));
        // Result changed → written even inside the window.
        let mut failed = noise.clone();
        failed.result = Some("failed:2".into());
        assert!(should_write(Some(&base), &failed));
        // Due-state change → written even inside the window.
        let mut due = noise.clone();
        due.due_since = Some(at(0, 30));
        assert!(should_write(Some(&base), &due));

        // A second seat's report is not debounce noise.
        let mut advisor = noise.clone();
        advisor.last_by = Some(LastBy {
            seat: "advisor".into(),
            ..Default::default()
        });
        let mut implementer = advisor.clone();
        implementer.last_by.as_mut().unwrap().seat = "implementer".into();
        assert!(should_write(Some(&advisor), &implementer));

        // And through write_cas on a plain directory: the noisy write is a no-op.
        let dir = tempfile::tempdir().unwrap();
        assert!(write_cas(dir.path(), "session-reap", |l| *l = base.clone()).unwrap());
        assert!(!write_cas(dir.path(), "session-reap", |l| l.last_run = Some(at(0, 30))).unwrap());
        assert_eq!(
            load(dir.path(), "session-reap").unwrap().last_run,
            Some(at(0, 0))
        );
        assert!(write_cas(dir.path(), "session-reap", |l| l.last_run = Some(at(2, 0))).unwrap());
        assert_eq!(
            load(dir.path(), "session-reap").unwrap().last_run,
            Some(at(2, 0))
        );
    }

    // trace:BUG-1573 | ai:codex
    #[test]
    fn legacy_ledger_and_failure_trip_round_trip() {
        let legacy: JobLedger = serde_yaml::from_str(
            "job: performance-guard\nlast_run: 2026-09-21T21:11:59Z\nresult: failed:1\n",
        )
        .unwrap();
        assert!(legacy.failure_trips.is_empty());

        let mut current = legacy;
        current.failure_trips.push(FailureTrip {
            trip_id: "performance-guard@2026-09-21T21:11:59Z".into(),
            at: "2026-09-21T21:11:59Z".parse().unwrap(),
            status: 1,
            performance: vec![PerformanceAudit {
                command: "show".into(),
                budget_ms: 1000,
                over_budget: 1447,
                denominator: 7346,
                proportion_millipercent: 19_697,
                tolerated_millipercent: 10_000,
                window_hours: 24,
                worst_ms: Some(165_672),
                excluded_samples: 0,
                lineage_scoped: true,
            }],
            audit_error: None,
        });
        let yaml = serde_yaml::to_string(&current).unwrap();
        assert!(!yaml.contains("secret"));
        assert_eq!(serde_yaml::from_str::<JobLedger>(&yaml).unwrap(), current);
    }

    #[test]
    fn episode_lifecycle() {
        let mut l = JobLedger::new("mailbox-latency");
        // false with no episode → nothing.
        assert_eq!(
            apply_condition(&mut l, false, at(0, 0)),
            EpisodeTransition::Unchanged
        );
        assert!(l.episode.is_none());
        // turns true → fires once.
        assert_eq!(
            apply_condition(&mut l, true, at(1, 0)),
            EpisodeTransition::Fired
        );
        let ep = l.episode.clone().unwrap();
        assert_eq!(ep.fired_at, Some(at(1, 0)));
        assert!(ep.is_open());
        // still true → no re-fire (once-until-cleared).
        assert_eq!(
            apply_condition(&mut l, true, at(2, 0)),
            EpisodeTransition::Unchanged
        );
        assert_eq!(l.episode.as_ref().unwrap().fired_at, Some(at(1, 0)));
        // goes false → cleared.
        assert_eq!(
            apply_condition(&mut l, false, at(3, 0)),
            EpisodeTransition::Cleared
        );
        assert_eq!(l.episode.as_ref().unwrap().cleared_at, Some(at(3, 0)));
        // still false → nothing.
        assert_eq!(
            apply_condition(&mut l, false, at(4, 0)),
            EpisodeTransition::Unchanged
        );
        // true again → a NEW episode fires.
        assert_eq!(
            apply_condition(&mut l, true, at(5, 0)),
            EpisodeTransition::Fired
        );
        assert_eq!(l.episode.as_ref().unwrap().fired_at, Some(at(5, 0)));
        assert!(l.episode.as_ref().unwrap().is_open());
    }

    // trace:STORY-1226 | ai:claude
    #[test]
    fn write_cas_skips_a_store_that_is_mid_rebase() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q", "-b", "aida-store"]);
        git(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "seed",
        ]);
        std::fs::create_dir_all(root.join(".git").join("rebase-merge")).unwrap();
        let wrote = write_cas_opts(root, "mailbox-triage", false, |l| {
            l.result = Some("ok".into());
        })
        .unwrap();
        assert!(!wrote, "a mid-rebase store must not be written");
        assert!(load(root, "mailbox-triage").is_none());
    }

    // trace:TASK-1280 | ai:claude
    #[test]
    fn batch_retry_reapplies_only_the_ticks_delta_over_a_concurrent_update() {
        let base = JobLedger::new("mailbox-triage");
        // The tick marked the job due.
        let mut next = base.clone();
        next.due_since = Some(at(10, 0));
        next.due_reason = Some("every 30m".into());
        // Meanwhile another writer reported a run and left a note.
        let mut fresh = base.clone();
        fresh.last_run = Some(at(9, 30));
        fresh.note = Some("done by advisor".into());
        let merged = reapply_delta(Some(&base), &next, Some(&fresh));
        assert_eq!(merged.due_since, Some(at(10, 0)), "tick's change applied");
        assert_eq!(merged.due_reason.as_deref(), Some("every 30m"));
        assert_eq!(merged.last_run, Some(at(9, 30)), "concurrent change kept");
        assert_eq!(merged.note.as_deref(), Some("done by advisor"));
        // No ledger on disk yet → the tick's snapshot is the whole truth.
        assert_eq!(reapply_delta(Some(&base), &next, None), next);
        // A field the tick did NOT change never overwrites the fresh value,
        // even when base and next both carry a stale copy of it.
        let mut stale_next = next.clone();
        stale_next.note = base.note.clone();
        let merged = reapply_delta(Some(&base), &stale_next, Some(&fresh));
        assert_eq!(merged.note.as_deref(), Some("done by advisor"));
    }
}

//! Derived eligibility observations. Never grants merge or review authority.
// trace:STORY-2336 | ai:codex
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub(crate) struct ForgeEvidence {
    /// The PR's base branch; the ancestry check compares its tip to the head.
    pub base_ref: Option<String>,
    pub ci_completed_at: Option<DateTime<Utc>>,
}

/// Whether the base branch tip is an ancestor of the PR head. Decided from
/// commit ancestry, never from `mergeStateStatus` (BLOCKED can be current;
/// CLEAN can be behind without strict protection). `Unknown` never starts an
/// observation.
// trace:STORY-2336 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ancestry {
    Current,
    Behind,
    Unknown,
}

/// Network ancestry evidence: the forge compare API's `behind_by` for
/// `base...head`, read through forge.rs (STORY-2401). Anything not settled
/// (`Pending` or `Transient`) is `Unknown`.
// trace:STORY-2336 trace:STORY-2401 | ai:claude
pub(crate) fn forge_ancestry(root: &Path, base: &str, head: &str) -> Ancestry {
    ancestry_from_observed(crate::forge::github_compare_behind_by(root, base, head))
}
fn ancestry_from_observed(behind_by: crate::forge_observed::Observed<u64>) -> Ancestry {
    match behind_by.known() {
        Some(0) => Ancestry::Current,
        Some(_) => Ancestry::Behind,
        None => Ancestry::Unknown,
    }
}
#[cfg(test)]
fn ancestry_from_behind_by(s: &str) -> Ancestry {
    ancestry_from_observed(crate::forge::parse_behind_by(s))
}
impl ForgeEvidence {
    pub fn from_json(pr: &serde_json::Value) -> Self {
        let checks = pr["statusCheckRollup"].as_array();
        // Seed only when every check supplies a completion time. A partial
        // rollup cannot establish the time CI became green.
        let ci_completed_at = checks.filter(|c| !c.is_empty()).and_then(|checks| {
            checks
                .iter()
                .map(|c| {
                    // trace:BUG-2492 | ai:codex — successful commit statuses
                    // have startedAt, not CheckRun completedAt. Pending evidence
                    // must never seed a completed-CI observation.
                    let time = if c["state"] == "SUCCESS" {
                        c["startedAt"].as_str()
                    } else {
                        c["completedAt"].as_str()
                    };
                    parse_time(time)
                })
                .collect::<Option<Vec<_>>>()
                .and_then(|times| times.into_iter().max())
        });
        Self {
            base_ref: pr["baseRefName"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            ci_completed_at,
        }
    }
}
fn parse_time(s: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Observation {
    pr: u64,
    head_sha: String,
    title: String,
    branch: String,
    kind: String,
    first_observed_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
}
/// The last time a known disqualification was observed for (PR, head, kind).
/// A later re-seed never predates it.
// trace:STORY-2336 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Invalidation {
    pr: u64,
    head_sha: String,
    kind: String,
    invalidated_at: DateTime<Utc>,
}
/// The disposable, non-canonical cache file. An unreadable or older-format
/// file degrades to empty.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Cache {
    observations: Vec<Observation>,
    #[serde(default)]
    invalidations: Vec<Invalidation>,
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct IdleEligibleItem {
    pub pr: u64,
    pub head_sha: String,
    pub state: String,
    pub kind: String,
    pub idle_minutes: i64,
    pub first_observed_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub next_command: String,
}
impl IdleEligibleItem {
    pub fn age_label(&self) -> String {
        format!("idle >= {} min (since first observed)", self.idle_minutes)
    }
}

fn local_verdicts(
    root: &Path,
    pr: u64,
    title: &str,
) -> Vec<crate::gate::review_verdict::RecordedVerdict> {
    let mut ids = crate::pr_ship::extract_spec_ids_from_text(title);
    ids.push(format!("PR-{pr}"));
    ids.iter()
        .filter_map(|id| crate::gate::review_verdict::read_recorded_verdict(root, id))
        .filter(|v| !v.is_closed())
        .collect()
}
fn reviewing(root: &Path, pr: u64, head: &str, branch: &str, title: &str) -> bool {
    if crate::gate::review_marker::live_description(root, pr, Some(head)).is_some() {
        return true;
    }
    let specs = crate::pr_ship::extract_spec_ids_from_text(title);
    crate::list_leases(root).iter().any(|l| {
        l.role.as_deref() == Some("reviewer")
            && (l.scope == format!("PR-{pr}") || l.branch == branch || specs.contains(&l.scope))
            && [
                (l.active_pid, l.active_pid_start_time.as_deref()),
                (l.creator_pid, l.creator_pid_start_time.as_deref()),
            ]
            .into_iter()
            .any(|(pid, start)| {
                pid.is_some_and(|pid| crate::process_probe::process_identity_is_alive(pid, start))
            })
    })
}
fn locally_disqualified(root: &Path, pr: u64, head: &str, title: &str, branch: &str) -> bool {
    crate::gate::merge_hold_core::read_hold_record(root, pr).is_some()
        || reviewing(root, pr, head, branch, title)
}
// trace:STORY-2476 | ai:codex — shared current-head eligibility, without idle-age filtering.
pub(crate) fn local_kind(
    root: &Path,
    pr: u64,
    head: &str,
    title: &str,
    branch: &str,
) -> Option<(String, Option<DateTime<Utc>>)> {
    if locally_disqualified(root, pr, head, title, branch) {
        return None;
    }
    verdict_kind(root, pr, head, title)
}
/// The eligibility kind the verdict evidence alone implies, ignoring holds and
/// running reviews (so a disqualification can be recorded against its kind).
fn verdict_kind(
    root: &Path,
    pr: u64,
    head: &str,
    title: &str,
) -> Option<(String, Option<DateTime<Utc>>)> {
    let verdicts = local_verdicts(root, pr, title);
    let decision = crate::awaiting_you::classify_pr_review(&verdicts, Some(head));
    let at_head: Vec<_> = verdicts
        .iter()
        .filter(|v| {
            crate::awaiting_you::review_relation(v.reviewed_sha.as_deref(), Some(head))
                == crate::awaiting_you::ReviewRelation::Same
        })
        .collect();
    if !decision.suppressed && at_head.iter().any(|v| v.kind.approves()) {
        Some((
            "ship-ready".into(),
            at_head
                .iter()
                .filter(|v| v.kind.approves())
                .filter_map(|v| parse_time(v.recorded_at.as_deref()))
                .max(),
        ))
    } else if at_head.is_empty()
        && verdicts.iter().all(|v| {
            crate::awaiting_you::review_relation(v.reviewed_sha.as_deref(), Some(head))
                == crate::awaiting_you::ReviewRelation::Moved
        })
    {
        Some(("unreviewed-push".into(), None))
    } else {
        None
    }
}

fn refresh(
    root: &Path,
    snapshot: crate::status_prs::OpenPrSnapshot,
    previous: &Cache,
    now: DateTime<Utc>,
    ancestry: &dyn Fn(&str, &str) -> Ancestry,
) -> Cache {
    let mut out = Cache::default();
    for pr in snapshot.by_branch.values() {
        let Some(head) = pr.head_sha.as_deref().filter(|h| !h.is_empty()) else {
            continue;
        };
        let Some((kind, verdict_at)) = verdict_kind(root, pr.number, head, &pr.title) else {
            continue;
        };
        let same = |p: u64, h: &str, k: &str| p == pr.number && h == head && k == kind;
        let prior = previous
            .observations
            .iter()
            .find(|p| same(p.pr, &p.head_sha, &p.kind));
        let mut invalidated_at = previous
            .invalidations
            .iter()
            .filter(|i| same(i.pr, &i.head_sha, &i.kind))
            .map(|i| i.invalidated_at)
            .max();
        let evidence = snapshot
            .eligibility
            .get(&pr.number)
            .cloned()
            .unwrap_or_default();
        // trace:STORY-2336 | ai:claude — `None` = disqualified (drop and record
        // the boundary); `Some(false)` = evidence unknown (start nothing, keep
        // an existing observation untouched).
        let eligible = if pr.is_draft
            || crate::pr_has_merge_hold(root, pr)
            || locally_disqualified(root, pr.number, head, &pr.title, &pr.head_branch)
        {
            None
        } else if kind == "ship-ready" {
            // trace:TASK-2794 | ai:claude — only CONFLICTING is a known
            // disqualification; GitHub reports UNKNOWN (or omits the field)
            // while it recomputes after the base moves, which is evidence
            // unknown and must not reset ship age.
            if pr.ci_rollup.as_deref() != Some("pass")
                || pr.review_decision.as_deref() == Some("CHANGES_REQUESTED")
                || pr.mergeable.as_deref() == Some("CONFLICTING")
            {
                None
            } else if pr.mergeable.as_deref() == Some("MERGEABLE") {
                Some(true)
            } else {
                Some(false)
            }
        } else {
            match evidence
                .base_ref
                .as_deref()
                .map_or(Ancestry::Unknown, |base| ancestry(base, head))
            {
                Ancestry::Current => Some(true),
                Ancestry::Behind => None,
                Ancestry::Unknown => Some(false),
            }
        };
        match eligible {
            None => invalidated_at = Some(now),
            Some(false) => out.observations.extend(prior.cloned()),
            Some(true) => {
                let seed = verdict_at
                    .zip(evidence.ci_completed_at)
                    .map(|(v, c)| v.max(c).max(invalidated_at.unwrap_or(v)))
                    .filter(|t| *t <= now);
                out.observations.push(Observation {
                    pr: pr.number,
                    head_sha: head.into(),
                    title: pr.title.clone(),
                    branch: pr.head_branch.clone(),
                    kind: kind.clone(),
                    first_observed_at: prior.map(|p| p.first_observed_at).or(seed).unwrap_or(now),
                    observed_at: now,
                });
            }
        }
        // Retained only while this PR is open at this head with this kind.
        if let Some(at) = invalidated_at {
            out.invalidations.push(Invalidation {
                pr: pr.number,
                head_sha: head.into(),
                kind,
                invalidated_at: at,
            });
        }
    }
    out.observations.sort_by_key(|p| p.pr);
    out.invalidations.sort_by_key(|i| i.pr);
    out
}

/// `None` is the strictly read-only notice/statusline path. A failed forge
/// snapshot is empty and conservatively clears the derived observations.
pub(crate) fn collect(
    project: &Path,
    snapshot: Option<crate::status_prs::OpenPrSnapshot>,
    now: DateTime<Utc>,
) -> Vec<IdleEligibleItem> {
    collect_with(project, snapshot, now, &forge_ancestry)
}
/// `collect` with injected ancestry evidence (consulted only on refresh).
fn collect_with(
    project: &Path,
    snapshot: Option<crate::status_prs::OpenPrSnapshot>,
    now: DateTime<Utc>,
    ancestry: &dyn Fn(&Path, &str, &str) -> Ancestry,
) -> Vec<IdleEligibleItem> {
    use fs2::FileExt;
    let root = crate::main_worktree_root_from(project);
    let path = root.join(".aida/idle-eligible.json");
    let read = || {
        std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
            .unwrap_or_default()
    };
    let mut cache = read();
    if let Some(snapshot) = snapshot {
        // Nonblocking permanent sidecar serializes refresh writes. Re-read
        // under the lock so another refresher's first observation is retained.
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join(".aida/idle-eligible.lock"))
            .ok();
        let lock = lock.filter(|l| l.try_lock_exclusive().is_ok());
        let previous = if lock.is_some() { read() } else { cache };
        // Even if persistence is busy/unavailable, use the fresh snapshot in
        // this report: known disqualifications must never reuse stale rows.
        cache = refresh(&root, snapshot, &previous, now, &|base, head| {
            ancestry(&root, base, head)
        });
        if let Some(lock) = lock {
            if let Ok(bytes) = serde_json::to_vec(&cache) {
                let tmp = root.join(format!(".aida/idle-eligible-{}.tmp", uuid::Uuid::new_v4()));
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
                let _ = std::fs::remove_file(tmp);
            }
            let _ = FileExt::unlock(&lock);
        }
    }
    rows(
        &root,
        cache.observations,
        now,
        crate::advisor::AdvisorConfig::load(&root).eligible_idle_minutes,
    )
}
fn rows(
    root: &Path,
    observations: Vec<Observation>,
    now: DateTime<Utc>,
    threshold: u64,
) -> Vec<IdleEligibleItem> {
    observations
        .into_iter()
        .filter_map(|p| {
            let freshness = now.signed_duration_since(p.observed_at).num_seconds();
            let minutes = now.signed_duration_since(p.first_observed_at).num_minutes();
            if !(0..=900).contains(&freshness) || minutes < 0 || (minutes as u64) < threshold {
                return None;
            }
            let (kind, _) = local_kind(root, p.pr, &p.head_sha, &p.title, &p.branch)?;
            if kind != p.kind {
                return None;
            }
            let ship = kind == "ship-ready";
            Some(IdleEligibleItem {
                pr: p.pr,
                head_sha: p.head_sha,
                state: if ship { "ship" } else { "review" }.into(),
                kind,
                idle_minutes: minutes,
                first_observed_at: p.first_observed_at,
                observed_at: p.observed_at,
                next_command: if ship {
                    format!("aida pr ship {}", p.pr)
                } else {
                    format!("aida queue work PR-{} --role reviewer", p.pr)
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    fn now() -> DateTime<Utc> {
        parse_time(Some("2026-11-03T05:00:00Z")).unwrap()
    }
    fn snapshot(head: &str, ci: &str) -> crate::status_prs::OpenPrSnapshot {
        snapshot_with(head, ci, "CLEAN")
    }
    fn snapshot_with(head: &str, ci: &str, merge_state: &str) -> crate::status_prs::OpenPrSnapshot {
        let mut s = crate::status_prs::parse_open_pr_snapshot(&serde_json::json!([{
            "number": 42, "title": "work", "headRefName": "topic", "headRefOid": head,
            "baseRefName": "main",
            "mergeable": "MERGEABLE", "mergeStateStatus": merge_state, "isDraft": false,
            "statusCheckRollup": [{"name": "build", "status": "COMPLETED", "conclusion": "SUCCESS", "completedAt": "2026-11-03T04:30:00Z"}]
        }]).to_string(), Some(&[]));
        s.by_branch.get_mut("topic").unwrap().ci_rollup = Some(ci.into());
        s
    }
    // trace:STORY-2336 | ai:claude — hermetic ancestry evidence.
    fn refresh_with(
        root: &Path,
        snap: crate::status_prs::OpenPrSnapshot,
        prev: &Cache,
        now: DateTime<Utc>,
        a: Ancestry,
    ) -> Cache {
        refresh(root, snap, prev, now, &|base, _| {
            assert_eq!(base, "main");
            a
        })
    }
    fn current(
        root: &Path,
        snap: crate::status_prs::OpenPrSnapshot,
        prev: &Cache,
        now: DateTime<Utc>,
    ) -> Vec<Observation> {
        refresh_with(root, snap, prev, now, Ancestry::Current).observations
    }
    fn cache(observations: &[Observation]) -> Cache {
        Cache {
            observations: observations.to_vec(),
            invalidations: Vec::new(),
        }
    }
    fn collect_current(
        root: &Path,
        snap: Option<crate::status_prs::OpenPrSnapshot>,
        now: DateTime<Utc>,
    ) -> Vec<IdleEligibleItem> {
        collect_with(root, snap, now, &|_, _, _| Ancestry::Current)
    }
    fn setup() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join(".aida/review-verdicts")).unwrap();
        t
    }
    fn verdict(root: &Path, kind: &str, head: &str) {
        std::fs::write(
            root.join(".aida/review-verdicts/PR-42.json"),
            serde_json::json!({
                "verdict": kind, "reviewed_sha": head, "recorded_at": "2026-11-03T04:40:00Z"
            })
            .to_string(),
        )
        .unwrap();
    }
    #[test]
    fn idle_eligible_review_becomes_due_and_resets_on_head_or_action() {
        let t = setup();
        let root = t.path();
        let n = now();
        let first = current(root, snapshot("abcdef1234", "pass"), &Cache::default(), n);
        assert_eq!(first.len(), 1);
        assert!(rows(root, first.clone(), n, 15).is_empty());
        let later = current(
            root,
            snapshot("abcdef1234", "pass"),
            &cache(&first),
            n + Duration::minutes(16),
        );
        let due = rows(root, later.clone(), n + Duration::minutes(16), 15);
        assert_eq!(due[0].state, "review");
        assert_eq!(due[0].idle_minutes, 16);
        assert_eq!(due[0].next_command, "aida queue work PR-42 --role reviewer");
        let changed = current(
            root,
            snapshot("fedcba4321", "pass"),
            &cache(&later),
            n + Duration::minutes(17),
        );
        assert!(rows(root, changed, n + Duration::minutes(17), 15).is_empty());
        verdict(root, "request-changes", "abcdef1234");
        assert!(current(
            root,
            snapshot("abcdef1234", "pass"),
            &cache(&later),
            n + Duration::minutes(18)
        )
        .is_empty());
        assert!(rows(root, later, n + Duration::minutes(18), 15).is_empty());
    }
    #[test]
    fn idle_eligible_ship_seed_green_only_and_disqualification_reset() {
        let t = setup();
        let root = t.path();
        let n = now();
        verdict(root, "approved", "abcdef1234");
        let first = current(root, snapshot("abcdef1234", "pass"), &Cache::default(), n);
        let due = rows(root, first.clone(), n, 15);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].state, "ship");
        assert_eq!(due[0].idle_minutes, 20); // max(verdict, CI)
        assert_eq!(due[0].next_command, "aida pr ship 42");
        for ci in ["pending", "fail", "missing", "unknown", "?"] {
            assert!(current(root, snapshot("abcdef1234", ci), &cache(&first), n).is_empty());
        }
        let mut no_time = snapshot("abcdef1234", "pass");
        no_time.eligibility.get_mut(&42).unwrap().ci_completed_at = None;
        let reset = current(root, no_time, &Cache::default(), n);
        assert_eq!(reset[0].first_observed_at, n);
        let mut held = snapshot("abcdef1234", "pass");
        held.by_branch
            .get_mut("topic")
            .unwrap()
            .labels
            .push("aida:merge-hold".into());
        assert!(current(root, held, &cache(&first), n).is_empty());
        verdict(root, "approved", "fedcba4321");
        let pushed = current(root, snapshot("abcdef1234", "pass"), &cache(&first), n);
        assert_eq!(pushed[0].kind, "unreviewed-push");
        assert_eq!(pushed[0].first_observed_at, n);
    }
    #[test]
    fn idle_eligible_review_started_hold_draft_and_behind_are_excluded() {
        let t = setup();
        let root = t.path();
        let n = now();
        let first = current(root, snapshot("abcdef1234", "pass"), &Cache::default(), n);
        let dir = root.join(".aida/review-in-progress");
        std::fs::create_dir_all(&dir).unwrap();
        let marker = crate::gate::review_marker::Marker::for_this_process(
            42,
            Some("abcdef1234"),
            None,
            "test",
        );
        crate::gate::review_marker::write(root, &marker).unwrap();
        assert!(current(root, snapshot("abcdef1234", "pass"), &cache(&first), n).is_empty());
        assert!(rows(root, first, n + Duration::minutes(15), 15).is_empty());
        crate::gate::review_marker::clear(root, 42);
        let mut draft = snapshot("abcdef1234", "pass");
        draft.by_branch.get_mut("topic").unwrap().is_draft = true;
        assert!(current(root, draft, &Cache::default(), n).is_empty());
        std::fs::create_dir_all(root.join(".aida/merge-holds")).unwrap();
        std::fs::write(root.join(".aida/merge-holds/PR-42"), "unreadable hold").unwrap();
        assert!(current(root, snapshot("abcdef1234", "pass"), &Cache::default(), n).is_empty());
    }
    #[test]
    fn idle_eligible_cache_offline_freshness_config_and_all_renderers() {
        let t = setup();
        let root = t.path();
        let n = now();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[advisor]\neligible_idle_minutes = 2\n",
        )
        .unwrap();
        assert!(collect_current(root, Some(snapshot("abcdef1234", "pass")), n).is_empty());
        let path = root.join(".aida/idle-eligible.json");
        let before = std::fs::read(&path).unwrap();
        let due = collect(root, None, n + Duration::minutes(3));
        assert_eq!(due.len(), 1);
        assert_eq!(
            before,
            std::fs::read(&path).unwrap(),
            "offline read must not write"
        );
        assert_eq!(collect(root, None, n + Duration::minutes(15)).len(), 1);
        assert!(collect(root, None, n + Duration::seconds(901)).is_empty());
        assert!(collect(root, None, n - Duration::seconds(1)).is_empty());
        let report = crate::awaiting_you::AwaitingReport {
            idle_eligible: due,
            ..Default::default()
        };
        assert_eq!(report.to_json()["idle_eligible"][0]["state"], "review");
        assert!(report
            .render_agent_view()
            .contains("idle >= 3 min (since first observed)"));
        let mut human = Vec::new();
        report.render(true, &mut human).unwrap();
        assert!(String::from_utf8(human)
            .unwrap()
            .contains("idle_eligible PR-42 review"));
        assert!(report.compact_line().unwrap().contains("1 idle eligible"));
        assert!(crate::statusbar_cmd::you_channels(&report).contains(&(1, "idle eligible".into())));
        assert!(
            collect_current(root, Some(crate::status_prs::OpenPrSnapshot::default()), n).is_empty()
        );
        assert!(collect(root, None, n + Duration::minutes(3)).is_empty());
    }
    #[test]
    fn idle_eligible_busy_cache_still_uses_known_disqualification() {
        use fs2::FileExt;
        let t = setup();
        let root = t.path();
        let n = now();
        verdict(root, "approved", "abcdef1234");
        assert_eq!(
            collect_current(root, Some(snapshot("abcdef1234", "pass")), n).len(),
            1
        );
        let before = std::fs::read(root.join(".aida/idle-eligible.json")).unwrap();
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .open(root.join(".aida/idle-eligible.lock"))
            .unwrap();
        lock.lock_exclusive().unwrap();
        assert!(collect_current(root, Some(snapshot("abcdef1234", "fail")), n).is_empty());
        assert_eq!(
            before,
            std::fs::read(root.join(".aida/idle-eligible.json")).unwrap()
        );
        FileExt::unlock(&lock).unwrap();
    }
    #[test]
    fn idle_eligible_config_defaults_and_partial_completion_evidence() {
        assert_eq!(
            crate::advisor::AdvisorConfig::default().eligible_idle_minutes,
            15
        );
        assert_eq!(
            crate::advisor::AdvisorConfig::from_toml_str("[advisor]\neligible_idle_minutes = 7\n")
                .eligible_idle_minutes,
            7
        );
        assert_eq!(
            crate::advisor::AdvisorConfig::from_toml_str("[advisor]\neligible_idle_minutes = -1\n")
                .eligible_idle_minutes,
            15
        );
        let f = ForgeEvidence::from_json(
            &serde_json::json!({"baseRefName": "main", "statusCheckRollup": [{"completedAt": "2026-11-03T04:30:00Z"}, {}]}),
        );
        assert_eq!(f.base_ref.as_deref(), Some("main"));
        assert!(f.ci_completed_at.is_none());
        assert_eq!(ancestry_from_behind_by("0"), Ancestry::Current);
        assert_eq!(ancestry_from_behind_by("3"), Ancestry::Behind);
        assert_eq!(ancestry_from_behind_by(""), Ancestry::Unknown);
        assert_eq!(ancestry_from_behind_by("null"), Ancestry::Unknown);
    }
    // trace:STORY-2336 | ai:claude — F1: "current with base" is ancestry
    // evidence; mergeStateStatus never decides it.
    #[test]
    fn idle_eligible_review_eligibility_comes_from_ancestry_not_merge_state() {
        let t = setup();
        let root = t.path();
        let n = now();
        // BLOCKED but current: starts, then keeps, the observation.
        let blocked = || snapshot_with("abcdef1234", "pass", "BLOCKED");
        let first = refresh_with(root, blocked(), &Cache::default(), n, Ancestry::Current);
        assert_eq!(first.observations.len(), 1);
        assert_eq!(first.observations[0].kind, "unreviewed-push");
        let later = n + Duration::minutes(16);
        let kept = refresh_with(root, blocked(), &first, later, Ancestry::Current);
        assert_eq!(kept.observations[0].first_observed_at, n);
        assert_eq!(
            rows(root, kept.observations.clone(), later, 15)[0].state,
            "review"
        );
        // Behind with non-strict protection (CLEAN): no observation.
        let clean = || snapshot_with("abcdef1234", "pass", "CLEAN");
        assert!(
            refresh_with(root, clean(), &Cache::default(), n, Ancestry::Behind)
                .observations
                .is_empty()
        );
        assert!(refresh_with(root, clean(), &kept, later, Ancestry::Behind)
            .observations
            .is_empty());
        // Unknown never starts one; it neither refreshes nor restarts one.
        assert!(
            refresh_with(root, clean(), &Cache::default(), n, Ancestry::Unknown)
                .observations
                .is_empty()
        );
        let unknown = refresh_with(root, clean(), &first, later, Ancestry::Unknown);
        assert_eq!(unknown.observations[0].observed_at, n);
        assert_eq!(unknown.observations[0].first_observed_at, n);
        // A missing base ref is Unknown too, without consulting ancestry.
        let mut no_base = clean();
        no_base.eligibility.get_mut(&42).unwrap().base_ref = None;
        assert!(refresh(root, no_base, &Cache::default(), n, &|_, _| panic!(
            "no base"
        ))
        .observations
        .is_empty());
    }
    // trace:STORY-2336 | ai:claude — F2: a re-seed never predates the last
    // observed disqualification for (PR, head, kind).
    #[test]
    fn idle_eligible_ship_reseed_restarts_at_cleared_disqualification() {
        let t = setup();
        let root = t.path();
        let n = now(); // verdict 04:40, CI 04:30: both unchanged throughout.
        verdict(root, "approved", "abcdef1234");
        let run = |prev: &Cache, at, snap| refresh_with(root, snap, prev, at, Ancestry::Current);
        let ready = || snapshot("abcdef1234", "pass");
        let eligible = run(&Cache::default(), n, ready());
        assert_eq!(
            rows(root, eligible.observations.clone(), n, 15)[0].idle_minutes,
            20
        );
        // Hold observed at +30: the row drops and the boundary is recorded.
        let mut held = ready();
        held.by_branch
            .get_mut("topic")
            .unwrap()
            .labels
            .push("aida:merge-hold".into());
        let hold_at = n + Duration::minutes(30);
        let on_hold = run(&eligible, hold_at, held);
        assert!(on_hold.observations.is_empty());
        assert_eq!(on_hold.invalidations[0].invalidated_at, hold_at);
        // Cleared at +31: age restarts at the clear, not at the old approval.
        let cleared_at = n + Duration::minutes(31);
        let after_hold = run(&on_hold, cleared_at, ready());
        assert_eq!(after_hold.observations[0].first_observed_at, hold_at);
        assert!(rows(root, after_hold.observations.clone(), cleared_at, 15).is_empty());
        let due_at = hold_at + Duration::minutes(15);
        let due = run(&after_hold, due_at, ready());
        assert_eq!(rows(root, due.observations, due_at, 15)[0].idle_minutes, 15);
        // Same lifecycle through an observed running review.
        let marker = crate::gate::review_marker::Marker::for_this_process(
            42,
            Some("abcdef1234"),
            None,
            "test",
        );
        crate::gate::review_marker::write(root, &marker).unwrap();
        let review_at = n + Duration::minutes(60);
        let reviewing = run(&after_hold, review_at, ready());
        assert!(reviewing.observations.is_empty());
        crate::gate::review_marker::clear(root, 42);
        let after_review = run(&reviewing, review_at + Duration::minutes(1), ready());
        assert_eq!(after_review.observations[0].first_observed_at, review_at);
        // The boundary is dropped once the head moves on.
        let moved = run(&after_review, review_at, snapshot("fedcba4321", "pass"));
        assert!(moved.invalidations.is_empty());
    }
    // trace:TASK-2794 | ai:claude — GitHub's transient UNKNOWN (recomputing
    // after main moves) keeps ship age; only CONFLICTING disqualifies.
    #[test]
    fn idle_eligible_ship_unknown_mergeable_keeps_age_conflicting_resets() {
        let t = setup();
        let root = t.path();
        let n = now();
        verdict(root, "approved", "abcdef1234");
        let run = |prev: &Cache, at, snap| refresh_with(root, snap, prev, at, Ancestry::Current);
        let with_mergeable = |m: Option<&str>| {
            let mut s = snapshot("abcdef1234", "pass");
            s.by_branch.get_mut("topic").unwrap().mergeable = m.map(Into::into);
            s
        };
        let eligible = run(&Cache::default(), n, with_mergeable(Some("MERGEABLE")));
        let first_seen = eligible.observations[0].first_observed_at;
        for m in [Some("UNKNOWN"), None] {
            let recomputing = run(&eligible, n + Duration::minutes(5), with_mergeable(m));
            assert!(recomputing.invalidations.is_empty(), "{m:?}");
            assert_eq!(recomputing.observations[0].first_observed_at, first_seen);
            let back = run(
                &recomputing,
                n + Duration::minutes(6),
                with_mergeable(Some("MERGEABLE")),
            );
            assert_eq!(back.observations[0].first_observed_at, first_seen);
        }
        // Unknown with no prior observation starts nothing.
        let fresh = run(&Cache::default(), n, with_mergeable(Some("UNKNOWN")));
        assert!(fresh.observations.is_empty() && fresh.invalidations.is_empty());
        let conflict_at = n + Duration::minutes(7);
        let conflicting = run(&eligible, conflict_at, with_mergeable(Some("CONFLICTING")));
        assert!(conflicting.observations.is_empty());
        assert_eq!(conflicting.invalidations[0].invalidated_at, conflict_at);
    }
}

// trace:BUG-2492 | ai:codex
#[cfg(test)]
mod bug2492_status_time_tests {
    #[test]
    fn bug2492_status_completion_seed() {
        let mut pr = serde_json::json!({"statusCheckRollup":[
            {"completedAt":"2026-11-08T01:00:00Z"},
            {"context":"merge-hold-gate","state":"SUCCESS","startedAt":"2026-11-08T02:00:00Z"}
        ]});
        assert_eq!(
            super::ForgeEvidence::from_json(&pr).ci_completed_at,
            super::parse_time(Some("2026-11-08T02:00:00Z"))
        );
        pr["statusCheckRollup"][1]["state"] = serde_json::json!("PENDING");
        assert!(super::ForgeEvidence::from_json(&pr)
            .ci_completed_at
            .is_none());
    }
}

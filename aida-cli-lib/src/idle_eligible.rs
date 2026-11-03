//! Derived eligibility observations. Never grants merge or review authority.
// trace:STORY-2336 | ai:codex
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub(crate) struct ForgeEvidence {
    pub up_to_date: bool,
    pub ci_completed_at: Option<DateTime<Utc>>,
}
impl ForgeEvidence {
    pub fn from_json(pr: &serde_json::Value) -> Self {
        let checks = pr["statusCheckRollup"].as_array();
        // Seed only when every check supplies a completion time. A partial
        // rollup cannot establish the time CI became green.
        let ci_completed_at = checks.filter(|c| !c.is_empty()).and_then(|checks| {
            checks
                .iter()
                .map(|c| parse_time(c["completedAt"].as_str()))
                .collect::<Option<Vec<_>>>()
                .and_then(|times| times.into_iter().max())
        });
        Self {
            up_to_date: matches!(
                pr["mergeStateStatus"].as_str(),
                Some("CLEAN" | "UNSTABLE" | "HAS_HOOKS")
            ),
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
fn local_kind(
    root: &Path,
    pr: u64,
    head: &str,
    title: &str,
    branch: &str,
) -> Option<(String, Option<DateTime<Utc>>)> {
    if crate::gate::merge_hold_core::read_hold_record(root, pr).is_some()
        || reviewing(root, pr, head, branch, title)
    {
        return None;
    }
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
    previous: &[Observation],
    now: DateTime<Utc>,
) -> Vec<Observation> {
    let mut out = Vec::new();
    for pr in snapshot.by_branch.values() {
        let Some(head) = pr.head_sha.as_deref().filter(|h| !h.is_empty()) else {
            continue;
        };
        if pr.is_draft || crate::pr_has_merge_hold(root, pr) {
            continue;
        }
        let Some((kind, verdict_at)) =
            local_kind(root, pr.number, head, &pr.title, &pr.head_branch)
        else {
            continue;
        };
        let evidence = snapshot
            .eligibility
            .get(&pr.number)
            .cloned()
            .unwrap_or_default();
        if kind == "ship-ready" {
            if pr.ci_rollup.as_deref() != Some("pass")
                || pr.mergeable.as_deref() != Some("MERGEABLE")
                || pr.review_decision.as_deref() == Some("CHANGES_REQUESTED")
            {
                continue;
            }
        } else if !evidence.up_to_date {
            continue;
        }
        let prior = previous
            .iter()
            .find(|p| p.pr == pr.number && p.head_sha == head && p.kind == kind);
        let seed = verdict_at
            .zip(evidence.ci_completed_at)
            .map(|(v, c)| v.max(c))
            .filter(|t| *t <= now);
        out.push(Observation {
            pr: pr.number,
            head_sha: head.into(),
            title: pr.title.clone(),
            branch: pr.head_branch.clone(),
            kind,
            first_observed_at: prior.map(|p| p.first_observed_at).or(seed).unwrap_or(now),
            observed_at: now,
        });
    }
    out.sort_by_key(|p| p.pr);
    out
}

/// `None` is the strictly read-only notice/statusline path. A failed forge
/// snapshot is empty and conservatively clears the derived observations.
pub(crate) fn collect(
    project: &Path,
    snapshot: Option<crate::status_prs::OpenPrSnapshot>,
    now: DateTime<Utc>,
) -> Vec<IdleEligibleItem> {
    use fs2::FileExt;
    let root = crate::main_worktree_root_from(project);
    let path = root.join(".aida/idle-eligible.json");
    let read = || {
        std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<Observation>>(&b).ok())
            .unwrap_or_default()
    };
    let mut observations = read();
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
        let previous = if lock.is_some() { read() } else { observations };
        // Even if persistence is busy/unavailable, use the fresh snapshot in
        // this report: known disqualifications must never reuse stale rows.
        observations = refresh(&root, snapshot, &previous, now);
        if let Some(lock) = lock {
            if let Ok(bytes) = serde_json::to_vec(&observations) {
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
        observations,
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
        let mut s = crate::status_prs::parse_open_pr_snapshot(&serde_json::json!([{
            "number": 42, "title": "work", "headRefName": "topic", "headRefOid": head,
            "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "isDraft": false,
            "statusCheckRollup": [{"name": "build", "status": "COMPLETED", "conclusion": "SUCCESS", "completedAt": "2026-11-03T04:30:00Z"}]
        }]).to_string(), Some(&[]));
        s.by_branch.get_mut("topic").unwrap().ci_rollup = Some(ci.into());
        s
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
        let first = refresh(root, snapshot("abcdef1234", "pass"), &[], n);
        assert_eq!(first.len(), 1);
        assert!(rows(root, first.clone(), n, 15).is_empty());
        let later = refresh(
            root,
            snapshot("abcdef1234", "pass"),
            &first,
            n + Duration::minutes(16),
        );
        let due = rows(root, later.clone(), n + Duration::minutes(16), 15);
        assert_eq!(due[0].state, "review");
        assert_eq!(due[0].idle_minutes, 16);
        assert_eq!(due[0].next_command, "aida queue work PR-42 --role reviewer");
        let changed = refresh(
            root,
            snapshot("fedcba4321", "pass"),
            &later,
            n + Duration::minutes(17),
        );
        assert!(rows(root, changed, n + Duration::minutes(17), 15).is_empty());
        verdict(root, "request-changes", "abcdef1234");
        assert!(refresh(
            root,
            snapshot("abcdef1234", "pass"),
            &later,
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
        let first = refresh(root, snapshot("abcdef1234", "pass"), &[], n);
        let due = rows(root, first.clone(), n, 15);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].state, "ship");
        assert_eq!(due[0].idle_minutes, 20); // max(verdict, CI)
        assert_eq!(due[0].next_command, "aida pr ship 42");
        for ci in ["pending", "fail", "missing", "unknown", "?"] {
            assert!(refresh(root, snapshot("abcdef1234", ci), &first, n).is_empty());
        }
        let mut no_time = snapshot("abcdef1234", "pass");
        no_time.eligibility.get_mut(&42).unwrap().ci_completed_at = None;
        let reset = refresh(root, no_time, &[], n);
        assert_eq!(reset[0].first_observed_at, n);
        let mut held = snapshot("abcdef1234", "pass");
        held.by_branch
            .get_mut("topic")
            .unwrap()
            .labels
            .push("aida:merge-hold".into());
        assert!(refresh(root, held, &first, n).is_empty());
        verdict(root, "approved", "fedcba4321");
        let pushed = refresh(root, snapshot("abcdef1234", "pass"), &first, n);
        assert_eq!(pushed[0].kind, "unreviewed-push");
        assert_eq!(pushed[0].first_observed_at, n);
    }
    #[test]
    fn idle_eligible_review_started_hold_draft_and_behind_are_excluded() {
        let t = setup();
        let root = t.path();
        let n = now();
        let first = refresh(root, snapshot("abcdef1234", "pass"), &[], n);
        let dir = root.join(".aida/review-in-progress");
        std::fs::create_dir_all(&dir).unwrap();
        let marker = crate::gate::review_marker::Marker::for_this_process(
            42,
            Some("abcdef1234"),
            None,
            "test",
        );
        crate::gate::review_marker::write(root, &marker).unwrap();
        assert!(refresh(root, snapshot("abcdef1234", "pass"), &first, n).is_empty());
        assert!(rows(root, first, n + Duration::minutes(15), 15).is_empty());
        crate::gate::review_marker::clear(root, 42);
        let mut behind = snapshot("abcdef1234", "pass");
        behind.eligibility.get_mut(&42).unwrap().up_to_date = false;
        assert!(refresh(root, behind, &[], n).is_empty());
        let mut draft = snapshot("abcdef1234", "pass");
        draft.by_branch.get_mut("topic").unwrap().is_draft = true;
        assert!(refresh(root, draft, &[], n).is_empty());
        std::fs::create_dir_all(root.join(".aida/merge-holds")).unwrap();
        std::fs::write(root.join(".aida/merge-holds/PR-42"), "unreadable hold").unwrap();
        assert!(refresh(root, snapshot("abcdef1234", "pass"), &[], n).is_empty());
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
        assert!(collect(root, Some(snapshot("abcdef1234", "pass")), n).is_empty());
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
        assert!(collect(root, Some(crate::status_prs::OpenPrSnapshot::default()), n).is_empty());
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
            collect(root, Some(snapshot("abcdef1234", "pass")), n).len(),
            1
        );
        let before = std::fs::read(root.join(".aida/idle-eligible.json")).unwrap();
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .open(root.join(".aida/idle-eligible.lock"))
            .unwrap();
        lock.lock_exclusive().unwrap();
        assert!(collect(root, Some(snapshot("abcdef1234", "fail")), n).is_empty());
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
            &serde_json::json!({"mergeStateStatus": "BEHIND", "statusCheckRollup": [{"completedAt": "2026-11-03T04:30:00Z"}, {}]}),
        );
        assert!(!f.up_to_date);
        assert!(f.ci_completed_at.is_none());
    }
}

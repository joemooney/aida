//! Additional independent evidence for the pipeline detector.
// trace:TASK-3075 | ai:codex
use super::*;
use std::collections::BTreeSet;

pub(super) struct Alarm {
    pub kind: &'static str,
    pub active: Option<bool>,
    pub identity: String,
    pub immediate: bool,
}
impl Alarm {
    fn new(kind: &'static str, active: Option<bool>, identity: String) -> Self {
        Self {
            kind,
            active,
            identity,
            immediate: kind != "under-cap",
        }
    }
}

/// Count each occupied implementer worktree once across lease/registry replicas.
/// MCP servers and the harness have distinct PIDs but consume one lane.
/// Missing start identity or invisible processes cannot prove spare capacity.
pub(super) fn implementer_count(
    root: &Path,
    leases: Option<&[crate::leases::SessionLease]>,
    entries: Option<&[crate::agent_registry::AgentRegistryEntry]>,
) -> Option<usize> {
    let observer = aida_core::pid_observer::PidObserver::current();
    let mut live = BTreeSet::new();
    let mut processes = BTreeSet::new();
    let lane = |path: &Path, pid: u32, start: &str| {
        if path.as_os_str().is_empty() {
            format!("pid:{pid}:{start}")
        } else {
            format!(
                "tree:{}",
                path.canonicalize()
                    .unwrap_or_else(|_| path.to_path_buf())
                    .display()
            )
        }
    };
    for l in leases?
        .iter()
        .filter(|l| l.role.as_deref() == Some("implementer"))
    {
        if l.hostname != crate::coordination::hostname() || !observer.can_observe(l.pid_namespace) {
            return None;
        }
        let pid = l.active_pid.or(l.creator_pid)?;
        let start = if l.active_pid.is_some() {
            l.active_pid_start_time.as_ref()
        } else {
            l.creator_pid_start_time.as_ref()
        }?;
        if crate::process_probe::process_identity_is_alive(pid, Some(start))
            && processes.insert((pid, start.clone()))
        {
            live.insert(lane(&l.worktree_path, pid, start));
        }
    }
    for e in entries?
        .iter()
        .filter(|e| e.role.as_deref() == Some("implementer"))
    {
        match crate::agent_registry::observed_liveness_from(root, e, observer) {
            Some(false) => {}
            Some(true) => {
                let start = e.pid_start.as_deref()?;
                if processes.insert((e.pid, start.to_owned())) {
                    live.insert(lane(&e.worktree_path, e.pid, start));
                }
            }
            None => return None,
        }
    }
    Some(live.len())
}

pub(super) fn observe(
    root: &Path,
    prs: Option<&crate::status_prs::OpenPrSnapshot>,
    count: Option<usize>,
    queued: Option<bool>,
    threshold: u32,
    now: i64,
    details: &mut Vec<String>,
) -> (Vec<Alarm>, Option<BTreeSet<String>>) {
    // trace:STORY-2660 | ai:codex
    let cap = crate::standalone_implementer::configured_cap(root);
    if let Err(error) = &cap {
        details.push(format!("Implementer capacity unknown: {error:#}"));
    }
    let cap = cap.ok();
    details.push(format!("Implementers: {count:?}, cap {cap:?}; queued work: {queued:?}; inspect aida queue list --for implementer; dispatch: aida queue work <SPEC>"));
    let under = Alarm::new(
        "under-cap",
        and(&[count.zip(cap).map(|(n, cap)| n < cap), queued]),
        String::new(),
    );
    let merge = last_merge(root).map(|time| (time.to_string(), time));
    let no_merge = aged("no-merge-90m", merge, now, 90 * 60_000, details);
    if no_merge.active.is_none() {
        details.push("Last actual merge unknown; inspect gh pr list --state merged --json number,mergedAt; default-branch commit age is not merge evidence".into());
    }
    let mail = oldest_mail(root);
    let stale = match mail {
        Ok(Some(mail)) => aged(
            "stale-advisor-mail",
            Some(mail),
            now,
            i64::from(threshold) * 60_000,
            details,
        ),
        Ok(None) => Alarm::new("stale-advisor-mail", Some(false), String::new()),
        Err(e) => {
            details.push(format!("Advisor mail inspection unknown: {e:#}"));
            Alarm::new("stale-advisor-mail", None, String::new())
        }
    };
    details
        .push("Advisor mail: inspect aida mailbox inbox; read ACKs do not establish action".into());
    let (empty, members) = match empty_rework_evidence(root, prs, details) {
        Ok((ids, members)) => (Some(ids), Some(members)),
        Err(e) => {
            details.push(format!("Empty-rework evidence unknown: {e:#}; inspect .aida/empty-rework and aida integrate"));
            (None, None)
        }
    };
    let empty = Alarm::new(
        "empty-rework-exit",
        empty.as_ref().map(|ids| !ids.is_empty()),
        empty.unwrap_or_default().join(","),
    );
    (vec![under, no_merge, stale, empty], members)
}

fn aged(
    kind: &'static str,
    evidence: Option<(String, i64)>,
    now: i64,
    age: i64,
    details: &mut Vec<String>,
) -> Alarm {
    match evidence {
        Some((id, time)) if time > 0 && time <= now => {
            details.push(format!(
                "{kind}: evidence {id}, age {}m (threshold {}m)",
                (now - time) / 60_000,
                age / 60_000
            ));
            Alarm::new(kind, Some(now - time >= age), id)
        }
        _ => Alarm::new(kind, None, String::new()),
    }
}

/// Bounded actual-merge observation; saturated evidence must prove that no
/// unseen update could contain a later merge. Never use default-branch age.
fn last_merge(root: &Path) -> Option<i64> {
    let kind = crate::forge::read_forge_config(root)
        .or_else(|| crate::forge::origin_url(root).map(|u| crate::forge::detect_forge_kind(&u)));
    if kind != Some(crate::forge::ForgeKind::GitHub) {
        return None;
    }
    let rows = crate::forge::github_recent_merge_rows(root).known()?;
    parse_merge(rows.as_bytes())
}
fn parse_merge(bytes: &[u8]) -> Option<i64> {
    let rows: Vec<serde_json::Value> = serde_json::from_slice(bytes).ok()?;
    if rows.is_empty() || rows.len() > 100 {
        return None;
    }
    let times = rows
        .iter()
        .map(|r| {
            let parse = |key: &str| {
                chrono::DateTime::parse_from_rfc3339(r[key].as_str()?)
                    .ok()
                    .map(|t| t.timestamp_millis())
            };
            Some((parse("mergedAt")?, parse("updatedAt")?))
        })
        .collect::<Option<Vec<_>>>()?;
    if times.iter().any(|(merged, updated)| merged > updated) {
        return None;
    }
    let latest = times.iter().map(|(m, _)| *m).max()?;
    // Any unseen row has updatedAt <= the oldest fetched update. A merge
    // cannot postdate its update, so this bound proves the maximum is actual.
    (rows.len() < 100 || times.iter().map(|(_, u)| *u).min()? <= latest).then_some(latest)
}

/// Inspect verified advisor subjects without minting an audience or ACKing mail.
fn oldest_mail(root: &Path) -> Result<Option<(String, i64)>> {
    use aida_core::mailbox_logical::{inspect_mail, MailboxContext};
    let store = crate::detect_distributed_store_from(root)
        .ok_or_else(|| anyhow::anyhow!("store unknown"))?;
    let directory = crate::mailbox_resolver::RecipientDirectory::load(root, &store)?;
    let ctx = MailboxContext::resolve_or_local(root, Some(&store))
        .map_err(|_| anyhow::anyhow!("mail context unknown"))?;
    let limits = crate::mailbox_view::passive_limits();
    let mut oldest = None;
    let mut found = false;
    for (_, subject) in directory
        .inspection_readers()
        .into_iter()
        .filter(|(e, _)| e.role.as_deref() == Some("advisor"))
    {
        found = true;
        let subject = subject?;
        let mail = inspect_mail(&ctx, &subject, &limits);
        if let Some(candidate) = oldest_unread(mail)? {
            if oldest.as_ref().is_none_or(|(_, time)| candidate.1 < *time) {
                oldest = Some(candidate);
            }
        }
    }
    anyhow::ensure!(found, "no verified advisor subject");
    // A complete empty inbox is false, distinct from absent evidence.
    Ok(oldest)
}

fn oldest_unread(
    mail: aida_core::mailbox_logical::MailInspection,
) -> Result<Option<(String, i64)>> {
    use aida_core::mailbox_logical::InspectionState;
    anyhow::ensure!(
        mail.complete && mail.unknown_ids.is_empty(),
        "mail inspection partial"
    );
    let mut oldest = None;
    for m in mail.records {
        // Alarm requests must not create their own stale-mail episode.
        if (m.subject == "Pipeline stall alarm" && m.from == ALARM_SENDER)
            || !matches!(m.state, InspectionState::Unread | InspectionState::Pending)
        {
            continue;
        }
        anyhow::ensure!(m.timestamp > 0, "mail timestamp invalid");
        if oldest.as_ref().is_none_or(|(_, time)| m.timestamp < *time) {
            oldest = Some((m.id, m.timestamp));
        }
    }
    Ok(oldest)
}

/// Existing unchanged-head retry receipts are evidence of an actual empty exit.
/// Re-check the current PR/head and blockers; a changed/closed/green PR clears it.
#[cfg(test)]
fn empty_rework(
    root: &Path,
    prs: Option<&crate::status_prs::OpenPrSnapshot>,
    details: &mut Vec<String>,
) -> Result<Vec<String>> {
    empty_rework_evidence(root, prs, details).map(|(ids, _)| ids)
}

// Retention evidence shares the exact receipt/PR read used for active alarms.
// A CI rerun can clear the predicate without retiring the receipt/head.
// trace:TASK-3075 | ai:codex
fn empty_rework_evidence(
    root: &Path,
    prs: Option<&crate::status_prs::OpenPrSnapshot>,
    details: &mut Vec<String>,
) -> Result<(Vec<String>, BTreeSet<String>)> {
    let receipts = files(&root.join(".aida/empty-rework"))?;
    if receipts
        .iter()
        .all(|p| p.extension().and_then(|s| s.to_str()) != Some("json"))
    {
        return Ok((Vec::new(), BTreeSet::new()));
    }
    let prs = prs.ok_or_else(|| anyhow::anyhow!("PR snapshot unknown"))?;
    let mut ids = Vec::new();
    let mut members = BTreeSet::new();
    for path in receipts {
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let head: String = serde_json::from_slice(&std::fs::read(&path)?)?;
        let key = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("receipt name invalid"))?;
        let (spec, pr) = key
            .rsplit_once('-')
            .ok_or_else(|| anyhow::anyhow!("receipt name invalid"))?;
        uuid::Uuid::parse_str(spec)?;
        let pr: u64 = pr.parse()?;
        if let Some(p) = prs.by_branch.values().find(|p| p.number == pr) {
            let current_head = p
                .head_sha
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("empty-rework head unknown"))?;
            if current_head != head {
                continue;
            }
            members.insert(format!("{key}-{head}"));
            if p.ci_rollup.as_deref() == Some("fail")
                || p.mergeable.as_deref() == Some("CONFLICTING")
                || current_rejection(root, p)
            {
                ids.push(format!("{key}-{head}"));
                details.push(format!("Empty rework PR-{pr} at {head}: unchanged blocked head; inspect gh pr checks {pr}; rework: aida queue rework {spec}"));
            } else if pr_ready(root, p).is_none() {
                anyhow::bail!("empty-rework blockers unknown");
            }
        }
    }
    ids.sort();
    Ok((ids, members))
}

#[cfg(test)]
mod tests {
    use super::*;
    const MIN: i64 = 60_000;

    #[test]
    fn capacity_counts_verified_process_once_and_refuses_foreign_or_missing_identity() {
        let t = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let pid = std::process::id();
        let e: crate::agent_registry::AgentRegistryEntry =
            serde_json::from_value(serde_json::json!({
                "id":"test-worker", "agent_type":"codex", "pid":pid,
                "pid_start":crate::process_probe::process_start_identity(pid),
                "pid_namespace":crate::agent_registry::current_pid_namespace(),
                "started_at":now,"last_active_at":now,"role":"implementer",
                "worktree_path":t.path(),"source":"test"
            }))
            .unwrap();
        assert_eq!(
            implementer_count(t.path(), Some(&[]), Some(&[e.clone(), e.clone()])),
            Some(1)
        );
        let lease: crate::leases::SessionLease = serde_json::from_value(serde_json::json!({
            "id":"test-lease","scope":"TASK-3075","slug":"task-3075","owner":"test",
            "worktree_path":t.path(),"branch":"task-3075","started_at":now,
            "hostname":crate::coordination::hostname(),"role":"implementer",
            "pid_namespace":e.pid_namespace,"active_pid":pid,"active_pid_start_time":e.pid_start,
        }))
        .unwrap();
        assert_eq!(
            implementer_count(t.path(), Some(&[lease.clone()]), Some(&[e.clone()])),
            Some(1)
        );
        let mut foreign = lease;
        foreign.hostname = "unobservable-host".into();
        assert_eq!(
            implementer_count(t.path(), Some(&[foreign]), Some(&[])),
            None
        );
        let mut invisible = e.clone();
        invisible.pid_namespace = Some(u64::MAX);
        assert_eq!(
            implementer_count(t.path(), Some(&[]), Some(&[invisible])),
            None
        );
        let mut legacy = e.clone();
        legacy.pid_start = None;
        assert_eq!(
            implementer_count(t.path(), Some(&[]), Some(&[legacy])),
            None
        );
        let mut ended = e;
        ended.ended_at = Some(now);
        assert_eq!(
            implementer_count(t.path(), Some(&[]), Some(&[ended])),
            Some(0)
        );
        assert_eq!(implementer_count(t.path(), None, Some(&[])), None);
    }

    #[test]
    fn stale_mail_uses_verified_unread_and_pending_evidence_without_self_alarms() {
        use aida_core::mailbox_logical::{InspectionRecord, InspectionState, MailInspection};
        let record = |id: &str, time, subject: &str, state| InspectionRecord {
            id: id.into(),
            timestamp: time,
            subject: subject.into(),
            state,
            from: "worker".into(),
            to: aida_core::mailbox::Recipient::Agent("advisor".into()),
            intent: aida_core::mailbox::Intent::Request,
        };
        let mut own = record("self", MIN, "Pipeline stall alarm", InspectionState::Unread);
        own.from = ALARM_SENDER.into();
        let records = vec![
            own,
            record("old-read", MIN, "read", InspectionState::Acknowledged),
            record("new-unread", 3 * MIN, "review", InspectionState::Unread),
            record("old-pending", 2 * MIN, "rework", InspectionState::Pending),
        ];
        let mail = |complete, records| MailInspection {
            records,
            complete,
            unknown_ids: Vec::new(),
        };
        let evidence = oldest_unread(mail(true, records)).unwrap();
        assert_eq!(evidence, Some(("old-pending".into(), 2 * MIN)));
        let unrelated = record(
            "other-sender",
            MIN,
            "Pipeline stall alarm",
            InspectionState::Unread,
        );
        assert_eq!(
            oldest_unread(mail(true, vec![unrelated])).unwrap(),
            Some(("other-sender".into(), MIN))
        );
        assert!(oldest_unread(mail(false, Vec::new())).is_err());
        assert_eq!(oldest_unread(mail(true, Vec::new())).unwrap(), None);
        let mut details = Vec::new();
        assert_eq!(
            aged(
                "stale-advisor-mail",
                evidence.clone(),
                32 * MIN - 1,
                30 * MIN,
                &mut details
            )
            .active,
            Some(false)
        );
        assert_eq!(
            aged(
                "stale-advisor-mail",
                evidence,
                32 * MIN,
                30 * MIN,
                &mut details
            )
            .active,
            Some(true)
        );
    }

    #[test]
    fn delivered_alarm_has_stable_detector_sender_and_subject() {
        let t = tempfile::tempdir().unwrap();
        crate::supervise_cmd::send_advisor_body(
            t.path(),
            "advisor",
            "Pipeline needs attention: stale-advisor-mail".into(),
        )
        .unwrap();
        let messages = crate::mailbox_store::read_local_messages(t.path()).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from, ALARM_SENDER);
        assert_eq!(messages[0].subject.as_deref(), Some("Pipeline stall alarm"));
    }

    #[test]
    fn merge_age_uses_actual_merge_maximum_and_refuses_incomplete_evidence() {
        let row =
            |merge: &str, update: &str| serde_json::json!({"mergedAt":merge,"updatedAt":update});
        let old = row("2026-11-04T00:00:00Z", "2026-11-05T10:00:00Z");
        let latest = row("2026-11-05T09:00:00Z", "2026-11-05T09:00:00Z");
        let expected = chrono::DateTime::parse_from_rfc3339("2026-11-05T09:00:00Z")
            .unwrap()
            .timestamp_millis();
        let parse = |v: serde_json::Value| parse_merge(&serde_json::to_vec(&v).unwrap());
        assert_eq!(parse(serde_json::json!([old, latest])), Some(expected));
        assert_eq!(parse(serde_json::json!([])), None);
        assert_eq!(parse(serde_json::json!([{"mergedAt":null}])), None);
        let mut saturated = vec![old; 100];
        assert_eq!(parse(serde_json::json!(saturated)), None);
        saturated[99] = latest;
        assert_eq!(parse(serde_json::json!(saturated)), Some(expected));
        let mut details = Vec::new();
        assert_eq!(
            aged(
                "no-merge-90m",
                Some(("merge".into(), MIN)),
                91 * MIN - 1,
                90 * MIN,
                &mut details
            )
            .active,
            Some(false)
        );
        assert_eq!(
            aged(
                "no-merge-90m",
                Some(("merge".into(), MIN)),
                91 * MIN,
                90 * MIN,
                &mut details
            )
            .active,
            Some(true)
        );
        assert_eq!(
            aged(
                "no-merge-90m",
                Some(("future".into(), 100 * MIN)),
                91 * MIN,
                90 * MIN,
                &mut details
            )
            .active,
            None
        );
    }

    #[test]
    fn under_cap_is_independent_and_requires_queued_work_and_known_capacity() {
        let t = tempfile::tempdir().unwrap();
        let mut details = Vec::new();
        let cap = crate::standalone_implementer::configured_cap(t.path()).unwrap();
        for (count, queued, expected) in [
            (Some(0), Some(true), Some(true)),
            (Some(cap), Some(true), Some(false)),
            (Some(0), Some(false), Some(false)),
            (None, Some(true), None),
            (Some(0), None, None),
        ] {
            assert_eq!(
                observe(t.path(), None, count, queued, 30, MIN, &mut details).0[0].active,
                expected
            );
        }
    }

    #[test]
    fn empty_exit_requires_receipt_matching_a_still_blocked_head() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join(".aida/empty-rework");
        std::fs::create_dir_all(&dir).unwrap();
        let spec = uuid::Uuid::new_v4();
        let path = dir.join(format!("{spec}-42.json"));
        std::fs::write(&path, "\"abcdef1234\"").unwrap();
        let raw = serde_json::json!([{
            "number":42,"title":"work","headRefName":"topic","headRefOid":"abcdef1234",
            "mergeable":"CONFLICTING","isDraft":false,
            "statusCheckRollup":[{"name":"build","status":"COMPLETED","conclusion":"FAILURE"}]
        }]);
        let mut prs = parse_stall_prs(raw.as_array().unwrap(), Some(&["build".into()]));
        let mut details = Vec::new();
        assert_eq!(
            empty_rework(t.path(), Some(&prs), &mut details)
                .unwrap()
                .len(),
            1
        );
        assert!(details[0].contains(&format!("aida queue rework {spec}")));
        // Red -> pending rerun -> red on the same head must not mail twice.
        let mut s = Snapshot::default();
        let mut sends = 0;
        let mut send = |_: &str| {
            sends += 1;
            Ok(super::SendResult::Sent)
        };
        for (minute, ci, expected) in [(1, "fail", true), (2, "pending", false), (3, "fail", true)]
        {
            let p = prs.by_branch.get_mut("topic").unwrap();
            p.mergeable = Some("MERGEABLE".into());
            p.ci_rollup = Some(ci.into());
            let (ids, members) = empty_rework_evidence(t.path(), Some(&prs), &mut details).unwrap();
            s.empty_rework_members = Some(members);
            assert_eq!(!ids.is_empty(), expected);
            s.extra = vec![Alarm::new(
                "empty-rework-exit",
                Some(!ids.is_empty()),
                ids.join(","),
            )];
            let report = tick(t.path(), &s, minute * MIN, true, 30, &mut send).unwrap();
            assert_eq!(report.accepted.len(), usize::from(minute == 1));
        }
        assert_eq!(sends, 1);
        prs.by_branch.get_mut("topic").unwrap().head_sha = Some("newhead".into());
        assert!(empty_rework(t.path(), Some(&prs), &mut details)
            .unwrap()
            .is_empty());
        assert!(empty_rework(t.path(), None, &mut details).is_err());
        prs.by_branch.clear();
        assert!(empty_rework(t.path(), Some(&prs), &mut details)
            .unwrap()
            .is_empty());
        std::fs::write(&path, "broken").unwrap();
        assert!(empty_rework(t.path(), Some(&prs), &mut details).is_err());
    }

    // trace:TASK-3075 | ai:codex
    #[test]
    fn empty_exit_deduplicates_each_receipt_head_across_membership_changes() {
        for fail_after_send in [false, true] {
            let t = tempfile::tempdir().unwrap();
            let dir = t.path().join(".aida/empty-rework");
            std::fs::create_dir_all(&dir).unwrap();
            for pr in [41, 42] {
                let key = uuid::Uuid::new_v4();
                std::fs::write(dir.join(format!("{key}-{pr}.json")), "\"samehead\"").unwrap();
            }
            let raw = serde_json::json!([
                {"number":41,"title":"A","headRefName":"a","headRefOid":"samehead",
                 "mergeable":"MERGEABLE","isDraft":false},
                {"number":42,"title":"B","headRefName":"b","headRefOid":"samehead",
                 "mergeable":"MERGEABLE","isDraft":false}
            ]);
            let mut prs = parse_stall_prs(raw.as_array().unwrap(), Some(&["build".into()]));
            let mut sends = 0;
            // A -> A+B -> B -> A+B must deliver only for A, then B, even
            // when the accepted-result write fails and leaves a reservation.
            for (i, (a, b)) in [
                ("fail", "pending"),
                ("fail", "fail"),
                ("pending", "fail"),
                ("fail", "fail"),
            ]
            .into_iter()
            .enumerate()
            {
                prs.by_branch.get_mut("a").unwrap().ci_rollup = Some(a.into());
                prs.by_branch.get_mut("b").unwrap().ci_rollup = Some(b.into());
                let ids = empty_rework(t.path(), Some(&prs), &mut Vec::new()).unwrap();
                let s = Snapshot {
                    extra: vec![Alarm::new("empty-rework-exit", Some(true), ids.join(","))],
                    ..Default::default()
                };
                let mut writes = 0;
                let result = tick_with_writer(
                    t.path(),
                    &s,
                    (i as i64 + 1) * MIN,
                    true,
                    30,
                    &mut |_| {
                        sends += 1;
                        Ok(super::SendResult::Sent)
                    },
                    &mut |path, bytes| {
                        writes += 1;
                        if fail_after_send && i < 2 && writes == 2 {
                            anyhow::bail!("injected post-send failure");
                        }
                        Ok(aida_core::write_atomic(path, bytes)?)
                    },
                );
                assert_eq!(result.is_err(), fail_after_send && i < 2);
                assert_eq!(sends, (i + 1).min(2));
            }
            // A new head with a matching new receipt is fresh evidence.
            prs.by_branch.get_mut("a").unwrap().head_sha = Some("newhead".into());
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .ends_with("-41.json")
                {
                    std::fs::write(path, "\"newhead\"").unwrap();
                }
            }
            let ids = empty_rework(t.path(), Some(&prs), &mut Vec::new()).unwrap();
            let s = Snapshot {
                extra: vec![Alarm::new("empty-rework-exit", Some(true), ids.join(","))],
                ..Default::default()
            };
            let report = tick(t.path(), &s, 5 * MIN, true, 30, &mut |_| {
                sends += 1;
                Ok(super::SendResult::Sent)
            })
            .unwrap();
            assert_eq!(report.accepted, ["empty-rework-exit"]);
            assert_eq!(sends, 3);
        }
    }

    // trace:TASK-3075 | ai:codex
    #[test]
    fn empty_exit_prunes_retired_heads_but_keeps_pending_deliveries() {
        for cleared_by in [
            "receipt-removed",
            "receipt-replaced",
            "head-changed",
            "pr-closed",
        ] {
            for pending in [false, true] {
                let t = tempfile::tempdir().unwrap();
                let dir = t.path().join(".aida/empty-rework");
                std::fs::create_dir_all(&dir).unwrap();
                let path = dir.join(format!("{}-42.json", uuid::Uuid::new_v4()));
                let raw = serde_json::json!([{
                    "number":42,"title":"work","headRefName":"topic","headRefOid":"oldhead",
                    "mergeable":"CONFLICTING","isDraft":false
                }]);
                let mut prs = parse_stall_prs(raw.as_array().unwrap(), Some(&["build".into()]));
                // Repeated head replacement must stay bounded, including while
                // the previous accepted delivery is no longer an active alarm.
                for round in 0..4 {
                    let head = format!("head{round}");
                    std::fs::write(&path, serde_json::to_vec(&head).unwrap()).unwrap();
                    prs.by_branch.get_mut("topic").unwrap().head_sha = Some(head);
                    let (ids, members) =
                        empty_rework_evidence(t.path(), Some(&prs), &mut Vec::new()).unwrap();
                    let s = Snapshot {
                        extra: vec![Alarm::new("empty-rework-exit", Some(true), ids.join(","))],
                        empty_rework_members: Some(members),
                        ..Default::default()
                    };
                    tick(t.path(), &s, (round + 1) * MIN, true, 30, &mut |_| {
                        Ok(super::SendResult::Sent)
                    })
                    .unwrap();
                    let state: State = serde_json::from_slice(
                        &std::fs::read(t.path().join(".aida/pipeline-stalls.json")).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(state.episodes.len(), 1);
                }
                let state_path = t.path().join(".aida/pipeline-stalls.json");
                let mut state: State =
                    serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
                let episode = state.episodes.values_mut().next().unwrap();
                episode.accepted = !pending;
                episode.delivery_pending = pending;
                std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
                match cleared_by {
                    "receipt-removed" => std::fs::remove_file(&path).unwrap(),
                    "receipt-replaced" => std::fs::write(&path, "\"replacement\"").unwrap(),
                    "head-changed" => {
                        prs.by_branch.get_mut("topic").unwrap().head_sha =
                            Some("replacement".into())
                    }
                    "pr-closed" => prs.by_branch.clear(),
                    _ => unreachable!(),
                }
                // Unknown evidence cannot retire prior delivery evidence.
                // Preview also must not persist pruning.
                tick(
                    t.path(),
                    &Snapshot::default(),
                    5 * MIN,
                    true,
                    30,
                    &mut |_| panic!("unknown evidence"),
                )
                .unwrap();
                let unchanged: State =
                    serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
                assert_eq!(unchanged.episodes.len(), 1);
                for preview in [true, false] {
                    let (_, members) =
                        empty_rework_evidence(t.path(), Some(&prs), &mut Vec::new()).unwrap();
                    let s = Snapshot {
                        empty_rework_members: Some(members),
                        ..Default::default()
                    };
                    tick(t.path(), &s, 6 * MIN, !preview, 30, &mut |_| {
                        panic!("no active alarms")
                    })
                    .unwrap();
                    let state: State =
                        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
                    assert_eq!(state.episodes.len(), usize::from(preview || pending));
                }
            }
        }
    }

    // trace:TASK-3075 | ai:codex
    #[test]
    fn empty_exit_closes_legacy_aggregate_without_inventing_action() {
        for pending in [false, true] {
            let t = tempfile::tempdir().unwrap();
            std::fs::create_dir(t.path().join(".aida")).unwrap();
            let state = serde_json::json!({"version":1,"episodes":{
                "empty-rework-exit":{"first_ms":MIN,"observed_ms":MIN,
                    "accepted":!pending,"delivery_pending":pending,"identity":"A,B"}
            }});
            let path = t.path().join(".aida/pipeline-stalls.json");
            std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
            let s = Snapshot {
                extra: vec![Alarm::new("empty-rework-exit", Some(true), "A,B".into())],
                ..Default::default()
            };
            let report = tick(t.path(), &s, 2 * MIN, true, 30, &mut |_| {
                panic!("legacy closure sends no mail")
            })
            .unwrap();
            assert!(report.due.is_empty());
            assert_eq!(
                report
                    .unknown
                    .iter()
                    .filter(|s| s.contains("action unknown; legacy episode closed"))
                    .count(),
                2
            );
            let state: State = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            assert!(state.episodes.is_empty());
            assert_eq!(state.version, 2);
        }
    }

    // trace:TASK-3075 | ai:codex
    #[test]
    fn identity_delivery_survives_unknown_false_gaps_and_backwards_clock() {
        for kind in ["no-merge-90m", "stale-advisor-mail", "empty-rework-exit"] {
            let t = tempfile::tempdir().unwrap();
            std::fs::create_dir(t.path().join(".aida")).unwrap();
            let mut s = Snapshot {
                extra: vec![Alarm::new(kind, Some(true), "evidence1".into())],
                ..Default::default()
            };
            let mut send = |_: &str| Ok(super::SendResult::Sent);
            assert_eq!(
                tick(t.path(), &s, MIN, true, 30, &mut send)
                    .unwrap()
                    .accepted,
                [kind]
            );
            for (active, minute) in [
                (None, 2),
                (Some(false), 3),
                (Some(true), 100),
                (Some(true), 50),
            ] {
                s.extra[0].active = active;
                // Missing evidence must not replace the retained identity.
                s.extra[0].identity = if active == Some(true) {
                    "evidence1"
                } else {
                    ""
                }
                .into();
                assert!(tick(t.path(), &s, minute * MIN, true, 30, &mut send)
                    .unwrap()
                    .due
                    .is_empty());
            }
            s.extra[0].identity = "evidence2".into();
            assert_eq!(
                tick(t.path(), &s, 101 * MIN, true, 30, &mut send)
                    .unwrap()
                    .accepted,
                [kind]
            );
        }
    }

    #[test]
    fn no_advisor_mail_is_unknown_and_new_kinds_use_operator_fallback() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::create_dir_all(root.join(".aida-store/objects")).unwrap();
        let git = |args: &[&str]| {
            let output = crate::test_git::git_command()
                .current_dir(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(
            root.join(".aida/config.toml"),
            "[deployment]\nstore_path = \".aida-store\"\n[notify]\ncommand = \"cat >> operator-mail\"\nmin_interval = \"0s\"\n",
        )
        .unwrap();
        git(&["add", ".aida/config.toml"]);
        git(&["commit", "-q", "-m", "trusted notifications"]);
        let head = git(&["rev-parse", "HEAD"]);
        git(&[
            "update-ref",
            "refs/remotes/origin/main",
            String::from_utf8_lossy(&head.stdout).trim(),
        ]);
        assert!(oldest_mail(root)
            .unwrap_err()
            .to_string()
            .contains("no verified advisor subject"));
        assert!(crate::supervise_cmd::live_advisor_recipient(root).is_none());
        let mut details = Vec::new();
        let (alarms, _) = observe(root, None, Some(0), Some(false), 30, MIN, &mut details);
        assert!(alarms
            .iter()
            .find(|a| a.kind == "stale-advisor-mail")
            .unwrap()
            .active
            .is_none());
        for kind in [
            "under-cap",
            "no-merge-90m",
            "stale-advisor-mail",
            "empty-rework-exit",
        ] {
            let s = Snapshot {
                extra: vec![Alarm::new(kind, Some(true), "evidence".into())],
                ..Default::default()
            };
            // Supply stale-mail evidence explicitly: a real no-advisor snapshot
            // leaves that kind unknown, while other due kinds still fall back.
            let mut send = |body: &str| {
                let outcome = crate::supervise_cmd::deliver_pipeline_alarm(root, body)?;
                assert_eq!(outcome, super::SendResult::Sent);
                Ok(outcome)
            };
            let first = tick(root, &s, MIN, true, 30, &mut send).unwrap();
            if kind == "under-cap" {
                assert!(first.accepted.is_empty());
                for minute in [11, 21] {
                    tick(root, &s, minute * MIN, true, 30, &mut send).unwrap();
                }
                assert_eq!(
                    tick(root, &s, 31 * MIN, true, 30, &mut send)
                        .unwrap()
                        .accepted,
                    [kind]
                );
            } else {
                assert_eq!(first.accepted, [kind]);
            }
            assert!(std::fs::read_to_string(root.join("operator-mail"))
                .unwrap()
                .contains(kind));
        }
    }

    #[test]
    fn new_kinds_deliver_independently_and_preview_preserves_state() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        let mut s = Snapshot {
            lanes_running: Some(true),
            train_running: Some(true),
            ready: Some(true),
            work_waiting: Some(true),
            ..Default::default()
        };
        s.extra = vec![
            Alarm::new("under-cap", Some(true), String::new()),
            Alarm::new("no-merge-90m", Some(true), "merge1".into()),
            Alarm::new("stale-advisor-mail", Some(true), "mail1".into()),
            Alarm::new("empty-rework-exit", Some(true), "head1".into()),
        ];
        let mut sent = 0;
        let mut send = |_: &str| {
            sent += 1;
            Ok(super::SendResult::Sent)
        };
        let first = tick(t.path(), &s, MIN, true, 30, &mut send).unwrap();
        assert_eq!(
            first.accepted,
            ["no-merge-90m", "stale-advisor-mail", "empty-rework-exit"]
        );
        for minute in [11, 21] {
            assert!(tick(t.path(), &s, minute * MIN, true, 30, &mut send)
                .unwrap()
                .due
                .is_empty());
        }
        let bytes = std::fs::read(t.path().join(".aida/pipeline-stalls.json")).unwrap();
        let dry = tick(t.path(), &s, 31 * MIN, false, 30, &mut |_| panic!()).unwrap();
        assert_eq!(dry.due, ["under-cap"]);
        assert_eq!(
            bytes,
            std::fs::read(t.path().join(".aida/pipeline-stalls.json")).unwrap()
        );
        assert_eq!(
            tick(t.path(), &s, 31 * MIN, true, 30, &mut send)
                .unwrap()
                .accepted,
            ["under-cap"]
        );
        s.extra[2].identity = "mail2".into();
        assert_eq!(
            tick(t.path(), &s, 32 * MIN, true, 30, &mut send)
                .unwrap()
                .accepted,
            ["stale-advisor-mail"]
        );
        s.extra[1].active = None;
        let unknown = tick(t.path(), &s, 33 * MIN, true, 30, &mut send).unwrap();
        assert_eq!(unknown.unknown, ["no-merge-90m"]);
        s.extra[1].active = Some(true);
        assert!(tick(t.path(), &s, 34 * MIN, true, 30, &mut send)
            .unwrap()
            .due
            .is_empty());
        // An observation gap does not duplicate accepted age/event alarms.
        s.extra[0].active = Some(false);
        assert!(tick(t.path(), &s, 100 * MIN, true, 30, &mut send)
            .unwrap()
            .due
            .is_empty());
        assert_eq!(sent, 3);
    }
}

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
) -> Vec<Alarm> {
    let cap = crate::standalone_implementer::configured_cap(root);
    details.push(format!("Implementers: {count:?}, cap {cap}; queued work: {queued:?}; inspect aida queue list --for implementer; dispatch: aida queue work <SPEC>"));
    let under = Alarm::new(
        "under-cap",
        and(&[count.map(|n| n < cap), queued]),
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
    let empty = match empty_rework(root, prs, details) {
        Ok(ids) => Some(ids),
        Err(e) => {
            details.push(format!("Empty-rework evidence unknown: {e:#}; inspect .aida/empty-rework and aida integrate"));
            None
        }
    };
    let empty = Alarm::new(
        "empty-rework-exit",
        empty.as_ref().map(|ids| !ids.is_empty()),
        empty.unwrap_or_default().join(","),
    );
    vec![under, no_merge, stale, empty]
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
    let gh = crate::resolve_gh_binary()?;
    let mut cmd = std::process::Command::new(gh);
    // Search orders by update; parse_merge validates the unseen-row bound.
    cmd.current_dir(root).args([
        "pr",
        "list",
        "--state",
        "merged",
        "--search",
        "sort:updated-desc",
        "--limit",
        "100",
        "--json",
        "mergedAt,updatedAt",
    ]);
    let out = crate::command_output_with_timeout(cmd, std::time::Duration::from_secs(10))?;
    if !out.status.success() {
        return None;
    }
    parse_merge(&out.stdout)
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
        if m.subject == "Pipeline stall alarm"
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
fn empty_rework(
    root: &Path,
    prs: Option<&crate::status_prs::OpenPrSnapshot>,
    details: &mut Vec<String>,
) -> Result<Vec<String>> {
    let receipts = files(&root.join(".aida/empty-rework"))?;
    if receipts
        .iter()
        .all(|p| p.extension().and_then(|s| s.to_str()) != Some("json"))
    {
        return Ok(Vec::new());
    }
    let prs = prs.ok_or_else(|| anyhow::anyhow!("PR snapshot unknown"))?;
    let mut ids = Vec::new();
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
            if p.head_sha.as_deref() != Some(head.as_str()) {
                continue;
            }
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
    Ok(ids)
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
        let records = vec![
            record("self", MIN, "Pipeline stall alarm", InspectionState::Unread),
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
        let cap = crate::standalone_implementer::configured_cap(t.path());
        for (count, queued, expected) in [
            (Some(1), Some(true), Some(true)),
            (Some(cap), Some(true), Some(false)),
            (Some(0), Some(false), Some(false)),
            (None, Some(true), None),
            (Some(0), None, None),
        ] {
            assert_eq!(
                observe(t.path(), None, count, queued, 30, MIN, &mut details)[0].active,
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
            Ok(true)
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
        assert_eq!(
            tick(t.path(), &s, 34 * MIN, true, 30, &mut send)
                .unwrap()
                .accepted,
            ["no-merge-90m"]
        );
        // An observation gap does not duplicate accepted age/event alarms.
        s.extra[0].active = Some(false);
        assert!(tick(t.path(), &s, 100 * MIN, true, 30, &mut send)
            .unwrap()
            .due
            .is_empty());
        assert_eq!(sent, 4);
    }
}

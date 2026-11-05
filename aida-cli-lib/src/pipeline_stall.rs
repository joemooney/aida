//! Independent, observation-based pipeline alarms. No completion/merge authority.
// trace:STORY-2476 | ai:codex
use anyhow::Result;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

const THRESHOLD_MS: i64 = 30 * 60 * 1000;
// An absent observer cannot prove that a predicate held throughout a long gap.
const MAX_GAP_MS: i64 = 15 * 60 * 1000;

#[derive(Default, Deserialize, Serialize)]
struct State {
    version: u32,
    episodes: BTreeMap<String, Episode>,
}
#[derive(Deserialize, Serialize)]
struct Episode {
    first_ms: i64,
    observed_ms: i64,
    accepted: bool,
}
#[derive(Default, Debug, Serialize)]
pub(super) struct Report {
    execute: bool,
    active: Vec<String>,
    due: Vec<String>,
    accepted: Vec<String>,
    unknown: Vec<String>,
    busy: bool,
    evidence: Vec<String>,
}
impl Report {
    pub(super) fn summary(&self) -> String {
        format!(
            "pipeline alarms{}: active {:?}, due {:?}, accepted {:?}, unknown {:?}{}",
            if self.execute { "" } else { " (dry-run)" },
            self.active,
            self.due,
            self.accepted,
            self.unknown,
            if self.busy {
                "; another tick holds lock"
            } else {
                ""
            }
        )
    }
}
#[derive(Default)]
struct Snapshot {
    lanes_running: Option<bool>,
    train_running: Option<bool>,
    ready: Option<bool>,
    work_waiting: Option<bool>,
    details: Vec<String>,
}
impl Snapshot {
    fn predicates(&self) -> [(&'static str, Option<bool>); 2] {
        [
            (
                "pipeline-idle",
                and(&[
                    self.lanes_running.map(|v| !v),
                    self.ready.map(|v| !v),
                    self.work_waiting,
                ]),
            ),
            (
                "ready-without-train",
                and(&[self.ready, self.train_running.map(|v| !v)]),
            ),
        ]
    }
}
fn and(values: &[Option<bool>]) -> Option<bool> {
    if values.contains(&Some(false)) {
        Some(false)
    } else if values.contains(&None) {
        None
    } else {
        Some(true)
    }
}
fn any(values: &[Option<bool>]) -> Option<bool> {
    if values.contains(&Some(true)) {
        Some(true)
    } else if values.contains(&None) {
        None
    } else {
        Some(false)
    }
}

pub(super) fn run(
    root: &Path,
    execute: bool,
    send: &mut dyn FnMut(&str) -> Result<bool>,
) -> Result<Report> {
    // Read-only preview never creates a directory, lock, timer, eligibility cache or mail.
    let lock = if execute {
        let dir = root.join(".aida");
        std::fs::create_dir_all(&dir)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("pipeline-stalls.lock"))?;
        if file.try_lock_exclusive().is_err() {
            return Ok(Report {
                execute,
                busy: true,
                ..Default::default()
            });
        }
        Some(file)
    } else {
        None
    };
    let snapshot = observe(root);
    let report = tick(
        root,
        &snapshot,
        chrono::Utc::now().timestamp_millis(),
        execute,
        send,
    );
    drop(lock); // permanent sidecar is never unlinked
    report
}
fn tick(
    root: &Path,
    snapshot: &Snapshot,
    now: i64,
    execute: bool,
    send: &mut dyn FnMut(&str) -> Result<bool>,
) -> Result<Report> {
    let path = root.join(".aida/pipeline-stalls.json");
    let mut state = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<State>(&b).ok())
        .filter(|s| s.version == 1)
        .unwrap_or_default();
    state.version = 1;
    let mut report = Report {
        execute,
        evidence: snapshot
            .details
            .iter()
            .take(100)
            .map(|s| s.chars().take(1024).collect())
            .collect(),
        ..Default::default()
    };
    for (kind, active) in snapshot.predicates() {
        if active != Some(true) {
            state.episodes.remove(kind);
            if active.is_none() {
                report.unknown.push(kind.into());
            }
            continue;
        }
        report.active.push(kind.into());
        let episode = state.episodes.entry(kind.into()).or_insert(Episode {
            first_ms: now,
            observed_ms: now,
            accepted: false,
        });
        if now < episode.observed_ms || now - episode.observed_ms > MAX_GAP_MS {
            *episode = Episode {
                first_ms: now,
                observed_ms: now,
                accepted: false,
            };
        }
        episode.observed_ms = now;
        if now - episode.first_ms >= THRESHOLD_MS && !episode.accepted {
            report.due.push(kind.into());
        }
    }
    if execute && !report.due.is_empty() {
        let body = format!("Pipeline needs attention: {} (continuously observed >= 30 minutes).\n{}\nInspect: aida ps; aida queue list --for implementer; aida integrate\nNo lanes, leases or PRs were changed by this alarm.",
            report.due.join(", "), bounded_digest(&snapshot.details));
        // Retain the episode on refused/failed delivery so the next tick retries.
        match send(&body) {
            Ok(true) => {
                for kind in &report.due {
                    state.episodes.get_mut(kind).unwrap().accepted = true;
                }
                report.accepted = report.due.clone();
            }
            Ok(false) => report
                .unknown
                .push("delivery unavailable; retry next tick".into()),
            Err(e) => report
                .unknown
                .push(format!("delivery failed; retry next tick: {e:#}")),
        }
    }
    if execute {
        aida_core::write_atomic(&path, &serde_json::to_vec_pretty(&state)?)?;
    }
    Ok(report)
}

fn bounded_digest(lines: &[String]) -> String {
    let mut body = String::new();
    for line in lines {
        let line: String = line.chars().take(1024).collect();
        if body.len() + line.len() > 24_000 {
            body.push_str("\nAdditional evidence omitted; inspect aida ps / aida integrate.\n");
            break;
        }
        body.push_str(&line);
        body.push('\n');
    }
    body
}

// Parse every entry: best-effort registry lists cannot establish a negative.
fn registry(root: &Path) -> Result<Vec<crate::agent_registry::AgentRegistryEntry>> {
    let mut out = Vec::new();
    for path in files(&crate::agent_registry::agents_dir(root))? {
        if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            out.push(toml::from_str(&std::fs::read_to_string(path)?)?);
        }
    }
    Ok(out)
}
fn files(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries.map(|e| Ok(e?.path())).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}
fn lane_role(role: Option<&str>) -> bool {
    // Advisors are observers; any other scoped worker suppresses pipeline-idle.
    !matches!(role, Some("advisor" | "product"))
}
fn observe(root: &Path) -> Snapshot {
    let mut s = Snapshot::default();
    let observer = aida_core::pid_observer::PidObserver::current();
    let leases = crate::leases::list_leases_strict_all(root);
    let entries = registry(root);
    if let (Ok(leases), Ok(entries)) = (&leases, &entries) {
        let mut alive = Vec::new();
        for l in leases.iter().filter(|l| lane_role(l.role.as_deref())) {
            let live = if l.hostname != crate::coordination::hostname()
                || !observer.can_observe(l.pid_namespace)
            {
                None
            } else {
                // Prefer the child identity; legacy leases retain their creator identity.
                l.active_pid.or(l.creator_pid).map(|pid| {
                    crate::process_probe::process_identity_is_alive(
                        pid,
                        if l.active_pid.is_some() {
                            l.active_pid_start_time.as_deref()
                        } else {
                            l.creator_pid_start_time.as_deref()
                        },
                    )
                })
            };
            alive.push(live);
            if live == Some(false) {
                s.details.push(format!("Lease {} ({}, branch {}): recorded worker exited; inspect aida session show {}; cleanup: aida session end {}", l.id, l.scope, l.branch, l.id, l.id));
            }
        }
        alive.extend(
            entries
                .iter()
                .filter(|e| lane_role(e.role.as_deref()))
                .map(|e| crate::agent_registry::observed_liveness_from(root, e, observer)),
        );
        s.lanes_running = any(&alive);
    }
    if s.lanes_running.is_none() {
        s.details.push("Worker liveness unknown: unreadable lease/registry data, foreign host/namespace, or missing process identity; run the tick on the host and inspect aida ps".into());
    }
    s.train_running = crate::merge_lock::observed_train_running(root);
    if let Some((prs, raw)) = forge_snapshot(root) {
        let mut readiness = Vec::new();
        let mut blocked = false;
        let mut train = vec![s.train_running];
        for pr in prs.by_branch.values() {
            let row = raw
                .iter()
                .find(|r| r["number"].as_u64() == Some(pr.number))
                .unwrap();
            if pr.head_branch.starts_with("train/") {
                // An open train with unknown owner is never evidence of no train.
                train.push(
                    if crate::pr_train::train_abandoned(
                        row["body"].as_str().unwrap_or(""),
                        &crate::coordination::hostname(),
                        crate::process_probe::process_identity_is_alive,
                    )
                    .is_some()
                    {
                        Some(false)
                    } else {
                        Some(true)
                    },
                );
                continue;
            }
            readiness.push(pr_ready(root, pr));
            let failed = pr.ci_rollup.as_deref() == Some("fail");
            let rejected = pr.review_decision.as_deref() == Some("CHANGES_REQUESTED")
                || current_rejection(root, pr);
            blocked |= failed || rejected;
            let checks: Vec<_> = row["statusCheckRollup"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| {
                    matches!(
                        c["conclusion"].as_str().or(c["state"].as_str()),
                        Some("FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED")
                    )
                })
                .filter_map(|c| c["name"].as_str().or(c["context"].as_str()))
                .collect();
            let ids = crate::pr_ship::extract_spec_ids_from_text(&pr.title);
            let rework = if ids.is_empty() {
                "aida queue rework (select the owning spec)".to_owned()
            } else {
                ids.iter()
                    .map(|id| format!("aida queue rework {id}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            s.details.push(format!("PR-{} head {}: CI {}, failed checks [{}], review {}, mergeability {}; inspect/rework: {}; review: aida queue work PR-{} --role reviewer; ship when gates permit: aida pr ship {}",
                pr.number, pr.head_sha.as_deref().unwrap_or("unknown"), pr.ci_rollup.as_deref().unwrap_or("unknown"), checks.join(", "),
                if rejected { "request-changes" } else { pr.review_decision.as_deref().unwrap_or("local review / unknown") },
                pr.mergeable.as_deref().unwrap_or("unknown"), rework, pr.number, pr.number));
        }
        s.ready = any(&readiness);
        s.train_running = any(&train);
        s.work_waiting = any(&[Some(blocked), queue_waiting(root)]);
    }
    // A merge train is itself an active pipeline lane even without an agent row.
    s.lanes_running = any(&[s.lanes_running, s.train_running]);
    if s.ready.is_none() {
        s.details.push("PR readiness unknown: unsupported forge, incomplete snapshot, unreadable review or missing CI/mergeability evidence".into());
    }
    if s.train_running.is_none() {
        s.details
            .push("Train liveness unknown: unreadable or foreign merge-lease owner".into());
    }
    s
}

fn queue_waiting(root: &Path) -> Option<bool> {
    // Do not shell out to queue list: it opportunistically garbage-collects queues.
    let mut present = Vec::new();
    for path in files(&root.join(".aida-store/registry/queues")).ok()? {
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let bytes = std::fs::read(path).ok()?;
        let rows: Vec<aida_core::QueueEntry> = serde_yaml::from_slice(&bytes).ok()?;
        present.push(Some(!rows.is_empty()));
    }
    if let Some(home) = crate::home_dir() {
        let path = home.join(".aida/queue/implementer.yaml");
        let rows = crate::global_queue::read_queue(&path, "implementer").ok()?;
        present
            .push(Some(rows.iter().any(|r| {
                crate::main_worktree_root_from(&r.project_root) == root
            })));
    } else {
        present.push(None);
    }
    any(&present)
}
fn forge_snapshot(
    root: &Path,
) -> Option<(crate::status_prs::OpenPrSnapshot, Vec<serde_json::Value>)> {
    // The general forge resolver can repair config; observations must not.
    let kind = crate::forge::read_forge_config(root).or_else(|| {
        crate::forge::origin_url(root).map(|url| crate::forge::detect_forge_kind(&url))
    });
    if kind != Some(crate::forge::ForgeKind::GitHub) {
        return None;
    }
    let mut cmd = std::process::Command::new(crate::resolve_gh_binary()?);
    cmd.current_dir(root).args(["pr", "list", "--state", "open", "--limit", "1000", "--json",
        "number,title,headRefName,headRefOid,statusCheckRollup,mergeable,reviewDecision,labels,isDraft,baseRefName,body"]);
    let out = crate::command_output_with_timeout(cmd, crate::FORGE_CLI_CALL_TIMEOUT)?;
    if !out.status.success() {
        return None;
    }
    let raw: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
    // Saturated/truncated or malformed snapshots cannot prove no ready PR.
    if raw.len() >= 1000
        || raw.iter().any(|r| {
            r["number"].as_u64().is_none()
                || ["title", "headRefName", "headRefOid", "body"]
                    .iter()
                    .any(|k| r[k].as_str().is_none())
                || r["isDraft"].as_bool().is_none()
                || r["labels"].as_array().is_none()
                || r["reviewDecision"].as_str().is_none()
                || r["mergeable"].as_str().is_none()
                || r["statusCheckRollup"].as_array().is_none()
        })
    {
        return None;
    }
    let required = crate::status_prs::github_required_status_checks_uncached(root);
    let prs = crate::status_prs::parse_open_pr_snapshot(
        std::str::from_utf8(&out.stdout).ok()?,
        required.as_deref(),
    );
    if prs.by_branch.len() != raw.len() {
        return None;
    }
    Some((prs, raw))
}
fn current_rejection(root: &Path, pr: &crate::status_cleanup::OpenPrItem) -> bool {
    verdict_ids(pr)
        .iter()
        .filter_map(|id| crate::gate::review_verdict::read_recorded_verdict(root, id))
        .any(|v| {
            !v.is_closed()
                && matches!(
                    v.kind,
                    crate::gate::review_verdict::VerdictKind::RequestChanges
                        | crate::gate::review_verdict::VerdictKind::Rejected
                )
                && v.reviewed_sha.as_deref() == pr.head_sha.as_deref()
        })
}
fn verdict_ids(pr: &crate::status_cleanup::OpenPrItem) -> Vec<String> {
    let mut ids = crate::pr_ship::extract_spec_ids_from_text(&pr.title);
    ids.push(format!("PR-{}", pr.number));
    ids
}
fn pr_ready(root: &Path, pr: &crate::status_cleanup::OpenPrItem) -> Option<bool> {
    if pr.is_draft
        || crate::pr_has_merge_hold(root, pr)
        || pr.mergeable.as_deref() == Some("CONFLICTING")
        || matches!(pr.ci_rollup.as_deref(), Some("fail" | "pending"))
        || pr.review_decision.as_deref() == Some("CHANGES_REQUESTED")
    {
        return Some(false);
    }
    if pr.mergeable.as_deref() != Some("MERGEABLE") || pr.ci_rollup.as_deref() != Some("pass") {
        return None;
    }
    let head = pr.head_sha.as_deref().filter(|h| !h.is_empty())?;
    // Refused or malformed evidence cannot establish absence of approval.
    for id in verdict_ids(pr) {
        let path = root
            .join(".aida/review-verdicts")
            .join(format!("{id}.json"));
        match crate::gate::review_verdict::read_verdict_body(&path) {
            Ok(body) => {
                crate::gate::review_verdict::parse_recorded_verdict(&body)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    Some(
        crate::idle_eligible::local_kind(root, pr.number, head, &pr.title, &pr.head_branch)
            .is_some_and(|(kind, _)| kind == "ship-ready"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const MIN: i64 = 60_000;
    fn idle() -> Snapshot {
        Snapshot {
            lanes_running: Some(false),
            train_running: Some(false),
            ready: Some(false),
            work_waiting: Some(true),
            ..Default::default()
        }
    }
    fn at(
        root: &Path,
        s: &Snapshot,
        minutes: i64,
        send: &mut dyn FnMut(&str) -> Result<bool>,
    ) -> Report {
        tick(root, s, minutes * MIN, true, send).unwrap()
    }
    #[test]
    fn boundary_dedup_retry_and_episode_reset() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        let mut sent = 0;
        let mut send = |body: &str| {
            assert!(body.contains("pipeline-idle"));
            sent += 1;
            Ok(true)
        };
        for minute in [0, 10, 20, 29] {
            assert!(at(t.path(), &idle(), minute, &mut send).due.is_empty());
        }
        let before = std::fs::read(t.path().join(".aida/pipeline-stalls.json")).unwrap();
        let dry = tick(t.path(), &idle(), 30 * MIN, false, &mut |_| {
            panic!("dry-run sent")
        })
        .unwrap();
        assert_eq!(dry.due, ["pipeline-idle"]);
        assert_eq!(
            before,
            std::fs::read(t.path().join(".aida/pipeline-stalls.json")).unwrap()
        );
        assert!(at(t.path(), &idle(), 30, &mut |_| Ok(false))
            .accepted
            .is_empty());
        assert!(
            at(t.path(), &idle(), 31, &mut |_| anyhow::bail!("mail failed"))
                .accepted
                .is_empty()
        );
        assert_eq!(
            at(t.path(), &idle(), 32, &mut send).accepted,
            ["pipeline-idle"]
        );
        assert!(at(t.path(), &idle(), 33, &mut send).due.is_empty());
        assert_eq!(sent, 1);
        let mut s = idle();
        s.lanes_running = Some(true);
        assert!(at(t.path(), &s, 34, &mut |_| panic!()).active.is_empty());
        assert!(at(t.path(), &idle(), 35, &mut |_| panic!()).due.is_empty());
    }
    #[test]
    fn unknown_long_gap_and_clock_reversal_restart_timer() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        let mut no = |_: &str| -> Result<bool> { panic!("not due") };
        at(t.path(), &idle(), 0, &mut no);
        let mut unknown = idle();
        unknown.lanes_running = None;
        assert_eq!(
            at(t.path(), &unknown, 10, &mut no).unknown,
            ["pipeline-idle"]
        );
        for minute in [20, 30, 40] {
            assert!(at(t.path(), &idle(), minute, &mut no).due.is_empty());
        }
        assert!(at(t.path(), &idle(), 60, &mut no).due.is_empty()); // gap >15m
        assert!(at(t.path(), &idle(), 59, &mut no).due.is_empty()); // backwards
        assert!(at(t.path(), &idle(), 70, &mut no).due.is_empty());
    }
    #[test]
    fn alarms_have_independent_suppression() {
        let mut s = idle();
        assert_eq!(
            s.predicates(),
            [
                ("pipeline-idle", Some(true)),
                ("ready-without-train", Some(false))
            ]
        );
        s.ready = Some(true);
        s.lanes_running = Some(true);
        assert_eq!(
            s.predicates(),
            [
                ("pipeline-idle", Some(false)),
                ("ready-without-train", Some(true))
            ]
        );
        s.train_running = Some(true);
        assert_eq!(s.predicates()[1].1, Some(false));
        s.train_running = None;
        assert_eq!(s.predicates()[1].1, None);
        s.ready = Some(false);
        s.lanes_running = Some(false);
        s.work_waiting = Some(false);
        assert_eq!(s.predicates()[0].1, Some(false));
        s.work_waiting = None;
        assert_eq!(s.predicates()[0].1, None);
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        s.ready = Some(true);
        s.train_running = Some(false);
        s.lanes_running = None;
        let mut send = |_: &str| Ok(true);
        for m in [0, 10, 20] {
            at(t.path(), &s, m, &mut send);
        }
        assert_eq!(
            at(t.path(), &s, 30, &mut send).accepted,
            ["ready-without-train"]
        );
    }
    fn pr() -> crate::status_cleanup::OpenPrItem {
        crate::status_prs::parse_open_pr_snapshot(
            &serde_json::json!([{
                "number":42,"title":"work","headRefName":"topic","headRefOid":"abcdef1234",
                "mergeable":"MERGEABLE","isDraft":false,
                "statusCheckRollup":[{"name":"build","status":"COMPLETED","conclusion":"SUCCESS"}]
            }])
            .to_string(),
            Some(&[]),
        )
        .by_branch
        .remove("topic")
        .unwrap()
    }
    #[test]
    fn readiness_uses_current_head_verdict_without_idle_age_filter() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join(".aida/review-verdicts")).unwrap();
        let path = root.join(".aida/review-verdicts/PR-42.json");
        let mut p = pr();
        std::fs::write(
            &path,
            r#"{"verdict":"approved","reviewed_sha":"abcdef1234"}"#,
        )
        .unwrap();
        assert_eq!(pr_ready(root, &p), Some(true)); // fresh approval immediately suppresses idle
        p.labels.push("aida:merge-hold".into());
        assert_eq!(pr_ready(root, &p), Some(false));
        p.labels.clear();
        std::fs::write(&path, r#"{"verdict":"approved","reviewed_sha":"old"}"#).unwrap();
        assert_eq!(pr_ready(root, &p), Some(false));
        std::fs::write(
            &path,
            r#"{"verdict":"request-changes","reviewed_sha":"abcdef1234"}"#,
        )
        .unwrap();
        assert_eq!(pr_ready(root, &p), Some(false));
        assert!(current_rejection(root, &p));
        std::fs::write(&path, "broken").unwrap();
        assert_eq!(pr_ready(root, &p), None);
        p.ci_rollup = Some("fail".into());
        assert_eq!(pr_ready(root, &p), Some(false));
        p.ci_rollup = Some("unknown".into());
        assert_eq!(pr_ready(root, &p), None);
    }
    #[test]
    fn preview_creates_nothing_and_busy_tick_does_not_observe_or_send() {
        let t = tempfile::tempdir().unwrap();
        let report = run(t.path(), false, &mut |_| panic!("preview sends")).unwrap();
        assert!(report.accepted.is_empty());
        assert!(!t.path().join(".aida").exists());
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        let path = t.path().join(".aida/pipeline-stalls.lock");
        let f = std::fs::File::create(&path).unwrap();
        f.lock_exclusive().unwrap();
        assert!(run(t.path(), true, &mut |_| panic!()).unwrap().busy);
        assert!(!t.path().join(".aida/pipeline-stalls.json").exists());
        assert!(path.exists());
    }
    #[test]
    fn preview_does_not_repair_forge_configuration() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        let path = t.path().join(".aida/config.toml");
        let body = "[forge]\nprovider = \"pure-git\"\n";
        std::fs::write(&path, body).unwrap();
        assert!(forge_snapshot(t.path()).is_none());
        assert_eq!(std::fs::read_to_string(path).unwrap(), body);
        assert!(bounded_digest(&vec!["x".repeat(2048); 1000]).len() < 25_000);
    }
    #[test]
    fn unreadable_registry_and_foreign_identity_are_unknown() {
        let t = tempfile::tempdir().unwrap();
        let dir = crate::agent_registry::agents_dir(t.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.toml"), "broken [").unwrap();
        assert!(registry(t.path()).is_err());
        assert_eq!(observe(t.path()).lanes_running, None);
        let observer = aida_core::pid_observer::PidObserver {
            namespace: Some(42),
            isolated: false,
        };
        assert!(!observer.can_observe(Some(43)));
        assert_eq!(any(&[Some(false), None]), None);
        assert_eq!(any(&[Some(true), None]), Some(true));
    }
}

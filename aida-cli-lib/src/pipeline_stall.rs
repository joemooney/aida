//! Independent, observation-based pipeline alarms. No completion/merge authority.
// trace:STORY-2476 | ai:codex
use anyhow::Result;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

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
    #[serde(default)]
    delivery_pending: bool,
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
    failed_jobs: Vec<(u64, serde_json::Value)>,
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
    threshold_mins: u32,
    send: &mut dyn FnMut(&str) -> Result<bool>,
) -> Result<Report> {
    run_with_home(
        root,
        crate::home_dir().as_deref(),
        execute,
        threshold_mins,
        send,
    )
}

// Alarm persistence failures must not terminate the oversight watch or hide its report.
pub(super) fn run_for_watch(
    root: &Path,
    execute: bool,
    threshold_mins: u32,
    send: &mut dyn FnMut(&str) -> Result<bool>,
) -> Report {
    run(root, execute, threshold_mins, send).unwrap_or_else(|e| Report {
        execute,
        unknown: vec![format!("pipeline alarm observation failed: {e:#}")],
        ..Default::default()
    })
}

fn run_with_home(
    root: &Path,
    home: Option<&Path>,
    execute: bool,
    threshold_mins: u32,
    send: &mut dyn FnMut(&str) -> Result<bool>,
) -> Result<Report> {
    anyhow::ensure!(
        threshold_mins >= 15,
        "alarm threshold must be at least 15 minutes"
    );
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
    let snapshot = observe(root, home);
    let report = tick(
        root,
        &snapshot,
        chrono::Utc::now().timestamp_millis(),
        execute,
        threshold_mins,
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
    threshold_mins: u32,
    send: &mut dyn FnMut(&str) -> Result<bool>,
) -> Result<Report> {
    tick_with_writer(
        root,
        snapshot,
        now,
        execute,
        threshold_mins,
        send,
        &mut |path, bytes| Ok(aida_core::write_atomic(path, bytes)?),
    )
}
fn tick_with_writer(
    root: &Path,
    snapshot: &Snapshot,
    now: i64,
    execute: bool,
    threshold_mins: u32,
    send: &mut dyn FnMut(&str) -> Result<bool>,
    persist: &mut dyn FnMut(&Path, &[u8]) -> Result<()>,
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
            delivery_pending: false,
        });
        if now < episode.observed_ms || now - episode.observed_ms > MAX_GAP_MS {
            *episode = Episode {
                first_ms: now,
                observed_ms: now,
                accepted: false,
                delivery_pending: false,
            };
        }
        episode.observed_ms = now;
        if episode.delivery_pending {
            report.unknown.push(format!("{kind}: prior delivery outcome unresolved; inspect advisor inbox before resetting state"));
        }
        if now - episode.first_ms >= i64::from(threshold_mins) * 60_000
            && !episode.accepted
            && !episode.delivery_pending
        {
            report.due.push(kind.into());
        }
    }
    if execute && !report.due.is_empty() {
        let mut details = snapshot.details.clone();
        details.extend(failure_details(&snapshot.failed_jobs, &mut |job| {
            if cfg!(test) {
                return None;
            }
            let gh = crate::resolve_gh_binary()?;
            let mut cmd = std::process::Command::new(gh);
            cmd.current_dir(root).args([
                "api",
                &format!("repos/{{owner}}/{{repo}}/actions/jobs/{job}/logs"),
            ]);
            let crate::BoundedCommandOutput::Completed(out) =
                crate::command_output_with_timeout_capped(
                    cmd,
                    std::time::Duration::from_secs(5),
                    1024 * 1024,
                )
            else {
                return None;
            };
            (out.status.success() && out.stdout.len() <= 1024 * 1024)
                .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        }));
        let body = format!("Pipeline needs attention: {} (continuously observed >= {threshold_mins} minutes).\n{}\nInspect: aida ps; aida queue list --for implementer; aida integrate\nNo lanes, leases or PRs were changed by this alarm.",
            report.due.join(", "), bounded_digest(&details));
        // Reserve durably before external delivery. A failed post-send write leaves
        // an unresolved reservation, preventing duplicate urgent mail on later ticks.
        for kind in &report.due {
            state.episodes.get_mut(kind).unwrap().delivery_pending = true;
        }
        persist(&path, &serde_json::to_vec_pretty(&state)?)?;
        let delivered = send(&body);
        for kind in &report.due {
            state.episodes.get_mut(kind).unwrap().delivery_pending = false;
        }
        // Known refused/failed deliveries remain retryable after persistence.
        match delivered {
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
        persist(&path, &serde_json::to_vec_pretty(&state)?)?;
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
    // Only implementation/review lanes suppress idle; interactive observers do not.
    matches!(role, Some("implementer" | "reviewer"))
}
fn observe(root: &Path, home: Option<&Path>) -> Snapshot {
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
        observe_prs(root, home, &prs, &raw, &mut s);
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

// Shared snapshot processing keeps digest, readiness and deferred log selection aligned.
fn observe_prs(
    root: &Path,
    home: Option<&Path>,
    prs: &crate::status_prs::OpenPrSnapshot,
    raw: &[serde_json::Value],
    s: &mut Snapshot,
) {
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
            train.push(train_pr_running(
                root,
                pr.number,
                row["body"].as_str().unwrap_or(""),
                &mut s.details,
            ));
            continue;
        }
        let ready = pr_ready(root, pr);
        readiness.push(ready);
        let rejected = pr.review_decision.as_deref() == Some("CHANGES_REQUESTED")
            || current_rejection(root, pr);
        blocked |= pr.ci_rollup.as_deref() == Some("fail") || rejected;
        if pr.ci_rollup.as_deref() == Some("fail") {
            s.failed_jobs.push((pr.number, row.clone()));
        }
        s.details.extend(pr_digest(root, pr, row, ready, rejected));
    }
    s.ready = any(&readiness);
    s.train_running = any(&train);
    s.work_waiting = any(&[Some(blocked), queue_waiting(root, home)]);
}

fn queue_waiting(root: &Path, home: Option<&Path>) -> Option<bool> {
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
    if let Some(home) = home {
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
    let gh = crate::resolve_gh_binary()?;
    let mut cmd = std::process::Command::new(gh);
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
    let prs = parse_stall_prs(&raw, required.as_deref());
    if prs.by_branch.len() != raw.len() {
        return None;
    }
    Some((prs, raw))
}
// Latest attempts are the detector's evidence for both classification and logs.
// A hold gate is a lifecycle signal (pr_has_merge_hold), not a failing test.
// trace:STORY-2476 | ai:codex
fn check_name(check: &serde_json::Value) -> Option<&str> {
    check["name"].as_str().or(check["context"].as_str())
}
fn ci_checks(row: &serde_json::Value) -> Vec<&serde_json::Value> {
    let mut latest = BTreeMap::new();
    let mut unnamed = Vec::new();
    for check in row["statusCheckRollup"].as_array().into_iter().flatten() {
        let Some(name) = check_name(check) else {
            unnamed.push(check);
            continue;
        };
        if name == "merge-hold-gate" {
            continue;
        }
        // Compare parsed instants, including offsets. Missing timestamps retain
        // input order as a tie-breaker; a newer queued attempt supersedes old red.
        let time = |c: &serde_json::Value| {
            ["completedAt", "startedAt", "createdAt"]
                .iter()
                .filter_map(|key| c[key].as_str())
                .filter_map(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .max()
        };
        let previous = latest.entry(name).or_insert(check);
        if time(check) >= time(previous) {
            *previous = check;
        }
    }
    unnamed.extend(latest.into_values());
    unnamed
}
fn check_failed(check: &serde_json::Value) -> bool {
    matches!(
        check["conclusion"].as_str().or(check["state"].as_str()),
        Some("FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED")
    )
}

// Keep shared status displays unchanged and required-check unknown guards intact.
fn parse_stall_prs(
    raw: &[serde_json::Value],
    required: Option<&[String]>,
) -> crate::status_prs::OpenPrSnapshot {
    let mut normalized = raw.to_vec();
    for row in &mut normalized {
        let checks: Vec<_> = ci_checks(row).into_iter().cloned().collect();
        row["statusCheckRollup"] = checks.into();
        if let Some(checks) = row["statusCheckRollup"].as_array_mut() {
            for check in checks {
                // StatusContext uses state/context rather than conclusion/name.
                if let Some(state) = check["state"].as_str() {
                    match state {
                        "SUCCESS" => check["conclusion"] = "SUCCESS".into(),
                        "FAILURE" | "ERROR" => check["conclusion"] = "FAILURE".into(),
                        "PENDING" => check["status"] = "PENDING".into(),
                        _ => {}
                    }
                }
                if check["status"] == "COMPLETED"
                    && matches!(check["conclusion"].as_str(), Some("SKIPPED" | "NEUTRAL"))
                {
                    check["conclusion"] = "SUCCESS".into();
                }
            }
        }
    }
    let ci_required = required.map(|names| {
        names
            .iter()
            .filter(|n| n.as_str() != "merge-hold-gate")
            .cloned()
            .collect::<Vec<_>>()
    });
    crate::status_prs::parse_open_pr_snapshot(
        &serde_json::to_string(&normalized).unwrap(),
        ci_required.as_deref(),
    )
}

fn train_pr_running(
    root: &Path,
    number: u64,
    body: &str,
    details: &mut Vec<String>,
) -> Option<bool> {
    if crate::pr_train::train_abandoned(
        body,
        &crate::coordination::hostname(),
        crate::process_probe::process_identity_is_alive,
    )
    .is_some()
    {
        Some(false)
    } else {
        details.push(format!("Train PR-{number}: owner is live or liveness is unknown; inspect: gh pr view {number}; inspect train lease with aida integrate from {}", root.display()));
        None
    }
}

// Fetch at most four job logs per alarm, only when delivery is due. Job IDs
// come from GitHub's snapshot, never from executing stored URLs or shell text.
fn failure_details(
    prs: &[(u64, serde_json::Value)],
    fetch: &mut dyn FnMut(u64) -> Option<String>,
) -> Vec<String> {
    let mut details = Vec::new();
    let mut remaining = 4;
    for (pr, row) in prs {
        for check in ci_checks(row) {
            if !check_failed(check) {
                continue;
            }
            let name = check_name(check).unwrap_or("unknown check");
            let job = check["detailsUrl"]
                .as_str()
                .and_then(|url| url.split_once("/job/"))
                .and_then(|(_, tail)| tail.split(['/', '?', '#']).next())
                .and_then(|id| id.parse::<u64>().ok());
            let lines = if remaining > 0 {
                job.and_then(|id| {
                    remaining -= 1;
                    fetch(id)
                })
                .map(|log| crate::ci_failures::extract_failure_lines(&log))
            } else {
                None
            };
            match lines.filter(|lines| !lines.is_empty()) {
                Some(lines) => details.extend(lines.into_iter().map(|line| format!("PR-{pr} {name}: {line}"))),
                None => details.push(format!("PR-{pr} {name}: failing tests unavailable (log missing, no test failures, or alarm log budget exhausted); inspect: gh pr checks {pr}")),
            }
        }
    }
    details
}

fn pr_digest(
    root: &Path,
    pr: &crate::status_cleanup::OpenPrItem,
    row: &serde_json::Value,
    ready: Option<bool>,
    rejected: bool,
) -> Vec<String> {
    let n = pr.number;
    if ready == Some(true) {
        return vec![format!("PR-{n}: ship-ready; aida pr ship {n}")];
    }
    let mut blockers = Vec::new();
    if pr.is_draft {
        blockers.push(format!("draft; gh pr ready {n}"));
    }
    if crate::pr_has_merge_hold(root, pr) {
        blockers.push(format!("merge hold; aida merge-hold auto-release {n} (eligible Rework approval) or human/advisor: aida merge-hold clear {n}"));
    }
    let checks: Vec<_> = ci_checks(row)
        .into_iter()
        .filter(|c| check_failed(c))
        .filter_map(check_name)
        .collect();
    if rejected
        || pr.ci_rollup.as_deref() == Some("fail")
        || pr.mergeable.as_deref() == Some("CONFLICTING")
    {
        let ids = crate::pr_ship::extract_spec_ids_from_text(&pr.title);
        let rework = if ids.is_empty() {
            format!("gh pr view {n} (identify owning spec for aida queue rework)")
        } else {
            ids.iter()
                .map(|id| format!("aida queue rework {id}"))
                .collect::<Vec<_>>()
                .join("; ")
        };
        if rejected {
            blockers.push(format!("current-head request-changes; {rework}"));
        }
        if pr.ci_rollup.as_deref() == Some("fail") {
            blockers.push(format!(
                "red CI [{}]; {rework}; gh pr checks {n}",
                checks.join(", ")
            ));
        }
        if pr.mergeable.as_deref() == Some("CONFLICTING") {
            blockers.push(format!("merge conflict; {rework}"));
        }
    }
    match pr.ci_rollup.as_deref() {
        Some("pending") => blockers.push(format!("CI pending; gh pr checks {n}")),
        Some("missing" | "unknown" | "?") | None => {
            blockers.push(format!("CI evidence missing/unknown; gh pr checks {n}"))
        }
        _ => {}
    }
    if pr.mergeable.as_deref() != Some("MERGEABLE")
        && pr.mergeable.as_deref() != Some("CONFLICTING")
    {
        blockers.push(format!(
            "mergeability unknown; gh pr view {n} --json mergeable"
        ));
    }
    if blockers.is_empty() {
        blockers.push(format!("current-head review missing, active or unreadable; aida queue work PR-{n} --role reviewer"));
    }
    vec![format!(
        "PR-{n} head {}: {}",
        pr.head_sha.as_deref().unwrap_or("unknown"),
        blockers.join("; ")
    )]
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
        || current_rejection(root, pr)
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
        if crate::gate::review_verdict::verdict_file_refused(&path) {
            return None;
        }
        if let Some(current) = crate::gate::review_verdict::current_verdict(root, &id) {
            crate::gate::review_verdict::parse_recorded_verdict(&current.body)?;
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
        tick(root, s, minutes * MIN, true, 30, send).unwrap()
    }
    // trace:STORY-2476 | ai:codex
    #[test]
    fn nondefault_threshold_controls_due_time_and_digest() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        for minute in [0, 10, 20, 30, 40, 44] {
            assert!(tick(t.path(), &idle(), minute * MIN, true, 45, &mut |_| {
                panic!("alarm fired before configured threshold")
            })
            .unwrap()
            .due
            .is_empty());
        }
        let report = tick(t.path(), &idle(), 45 * MIN, true, 45, &mut |body| {
            assert!(body.contains(">= 45 minutes"));
            Ok(true)
        })
        .unwrap();
        assert_eq!(report.accepted, ["pipeline-idle"]);
    }

    #[test]
    fn watch_reports_unwritable_alarm_state_as_unknown() {
        let t = tempfile::tempdir().unwrap();
        // A directory at the lock path refuses open even for a privileged test user.
        std::fs::create_dir_all(t.path().join(".aida/pipeline-stalls.lock")).unwrap();
        let report = run_for_watch(t.path(), true, 30, &mut |_| panic!("must not send"));
        assert!(report.accepted.is_empty());
        assert_eq!(report.unknown.len(), 1);
        assert!(report.summary().contains("observation failed"));
        assert!(!t.path().join(".aida/pipeline-stalls.json").exists());
    }

    #[test]
    fn only_implementer_and_reviewer_roles_suppress_idle() {
        for role in [None, Some("integrator"), Some("advisor"), Some("product")] {
            assert!(!lane_role(role), "{role:?}");
        }
        for role in [Some("implementer"), Some("reviewer")] {
            assert!(lane_role(role), "{role:?}");
        }
        let t = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let leases = crate::leases::leases_dir(t.path());
        std::fs::create_dir_all(&leases).unwrap();
        // A live role-less interactive lease must not mask an idle pipeline.
        let lease = format!(
            r#"
            id = "interactive"
            scope = "STORY-2476"
            slug = "interactive"
            owner = "test"
            worktree_path = {}
            branch = "interactive"
            started_at = "2026-11-04T00:00:00Z"
            hostname = "{}"
            creator_pid = {}
        "#,
            toml::Value::String(t.path().to_string_lossy().into_owned()),
            crate::coordination::hostname(),
            std::process::id()
        );
        std::fs::write(leases.join("interactive.toml"), lease).unwrap();
        let parsed = crate::leases::list_leases_strict_all(t.path()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].role.is_none());
        assert_eq!(
            observe(t.path(), Some(home.path())).lanes_running,
            Some(false)
        );
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
        let dry = tick(t.path(), &idle(), 30 * MIN, false, 30, &mut |_| {
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
    // trace:STORY-2476 | ai:codex
    #[test]
    fn delivery_reservation_prevents_send_on_write_failure_and_duplicates_after_send() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        for minute in [0, 10, 20] {
            at(t.path(), &idle(), minute, &mut |_| panic!());
        }
        assert!(tick_with_writer(
            t.path(),
            &idle(),
            30 * MIN,
            true,
            30,
            &mut |_| panic!("must persist before send"),
            &mut |_, _| anyhow::bail!("injected reservation write failure")
        )
        .is_err());
        let mut writes = 0;
        let mut sent = 0;
        assert!(tick_with_writer(
            t.path(),
            &idle(),
            30 * MIN,
            true,
            30,
            &mut |_| {
                sent += 1;
                Ok(true)
            },
            &mut |path, bytes| {
                writes += 1;
                if writes == 2 {
                    anyhow::bail!("injected post-send write failure");
                }
                Ok(aida_core::write_atomic(path, bytes)?)
            }
        )
        .is_err());
        assert_eq!(sent, 1);
        let report = at(t.path(), &idle(), 31, &mut |_| panic!("duplicate mail"));
        assert!(report.due.is_empty());
        assert!(report
            .unknown
            .iter()
            .any(|s| s.contains("outcome unresolved")));
    }

    #[test]
    fn skipped_neutral_checks_allow_ready_but_unknown_ci_never_hides_rejection() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join(".aida/review-verdicts")).unwrap();
        let path = t.path().join(".aida/review-verdicts/PR-42.json");
        let raw = serde_json::json!([{
            "number":42,"title":"work","headRefName":"topic","headRefOid":"abcdef1234",
            "mergeable":"MERGEABLE","isDraft":false,
            "statusCheckRollup":[
                {"name":"build","status":"COMPLETED","conclusion":"SUCCESS"},
                {"name":"Release","status":"COMPLETED","conclusion":"SKIPPED"},
                {"name":"optional","status":"COMPLETED","conclusion":"NEUTRAL"}]
        }]);
        let mut p = parse_stall_prs(raw.as_array().unwrap(), Some(&["build".into()]))
            .by_branch
            .remove("topic")
            .unwrap();
        std::fs::write(
            &path,
            r#"{"verdict":"approved","reviewed_sha":"abcdef1234"}"#,
        )
        .unwrap();
        assert_eq!(pr_ready(t.path(), &p), Some(true));
        let missing = parse_stall_prs(raw.as_array().unwrap(), Some(&["missing".into()]));
        assert_eq!(
            missing.by_branch["topic"].ci_rollup.as_deref(),
            Some("missing")
        );
        assert_eq!(
            parse_stall_prs(raw.as_array().unwrap(), None).by_branch["topic"]
                .ci_rollup
                .as_deref(),
            Some("unknown")
        );
        p.ci_rollup = Some("?".into());
        std::fs::write(
            &path,
            r#"{"verdict":"request-changes","reviewed_sha":"abcdef1234"}"#,
        )
        .unwrap();
        assert_eq!(pr_ready(t.path(), &p), Some(false));
        let mut s = idle();
        s.ready = pr_ready(t.path(), &p);
        assert_eq!(s.predicates()[0].1, Some(true));
    }

    // trace:STORY-2476 | ai:codex
    #[test]
    fn held_green_pr_is_not_red_ci_and_never_fetches_hold_logs() {
        let t = tempfile::tempdir().unwrap();
        let row = serde_json::json!({
            "number":42,"title":"work (STORY-2476)","headRefName":"topic",
            "headRefOid":"abcdef1234","mergeable":"MERGEABLE","isDraft":false,
            "labels":[{"name":"aida:merge-hold"}],
            "statusCheckRollup":[
                {"name":"merge-hold-gate","status":"COMPLETED","conclusion":"FAILURE",
                 "detailsUrl":"https://github.com/o/r/actions/runs/1/job/123"},
                {"name":"Build (ubuntu-latest)","status":"COMPLETED","conclusion":"SUCCESS"}]
        });
        let raw = vec![row.clone()];
        let required = vec!["merge-hold-gate".into(), "Build (ubuntu-latest)".into()];
        let prs = parse_stall_prs(&raw, Some(&required));
        assert_eq!(prs.by_branch["topic"].ci_rollup.as_deref(), Some("pass"));
        let mut s = idle();
        observe_prs(t.path(), Some(t.path()), &prs, &raw, &mut s);
        assert_eq!(s.ready, Some(false));
        assert_eq!(s.work_waiting, Some(false));
        assert!(s.failed_jobs.is_empty());
        let digest = s.details.join("\n");
        assert!(digest.contains("merge hold"));
        assert!(!digest.contains("queue rework"));
        assert!(!digest.contains("red CI"));
        assert!(failure_details(&[(42, row)], &mut |_| panic!("hold log fetched")).is_empty());
    }

    #[test]
    fn latest_check_attempt_controls_readiness_digest_and_log_selection() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join(".aida/review-verdicts")).unwrap();
        std::fs::write(
            t.path().join(".aida/review-verdicts/PR-42.json"),
            r#"{"verdict":"approved","reviewed_sha":"abcdef1234"}"#,
        )
        .unwrap();
        let mut row = serde_json::json!({
            "number":42,"title":"work","headRefName":"topic","headRefOid":"abcdef1234",
            "mergeable":"MERGEABLE","isDraft":false,
            "statusCheckRollup":[
                {"name":"build","status":"COMPLETED","conclusion":"SUCCESS",
                 "startedAt":"2026-11-05T05:00:00Z","completedAt":"2026-11-05T05:01:00Z"},
                {"name":"build","status":"COMPLETED","conclusion":"FAILURE",
                 "startedAt":"2026-11-05T04:00:00Z","completedAt":"2026-11-05T04:01:00Z"}]
        });
        // Timestamp order wins over array order in both directions.
        for _ in 0..2 {
            row["statusCheckRollup"].as_array_mut().unwrap().reverse();
            let prs = parse_stall_prs(&[row.clone()], Some(&["build".into()]));
            let p = &prs.by_branch["topic"];
            assert_eq!(p.ci_rollup.as_deref(), Some("pass"));
            assert_eq!(pr_ready(t.path(), p), Some(true));
            assert!(!pr_digest(t.path(), p, &row, Some(false), false)
                .join("\n")
                .contains("red CI"));
            assert!(failure_details(&[(42, row.clone())], &mut |_| panic!(
                "superseded log fetched"
            ))
            .is_empty());
        }
        row["statusCheckRollup"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "name":"build","status":"IN_PROGRESS","conclusion":null,
                "startedAt":"2026-11-05T06:00:00Z","completedAt":null
            }));
        assert_eq!(
            parse_stall_prs(&[row], Some(&["build".into()])).by_branch["topic"]
                .ci_rollup
                .as_deref(),
            Some("pending")
        );
    }

    #[test]
    fn failed_status_context_gets_named_unavailable_test_evidence() {
        let t = tempfile::tempdir().unwrap();
        for state in ["FAILURE", "ERROR"] {
            let row = serde_json::json!({
                "number":42,"title":"work (STORY-2476)","headRefName":"topic",
                "statusCheckRollup":[{"context":"legacy-ci","state":state}]
            });
            let prs = parse_stall_prs(&[row.clone()], Some(&["legacy-ci".into()]));
            let p = &prs.by_branch["topic"];
            assert_eq!(p.ci_rollup.as_deref(), Some("fail"));
            assert!(pr_digest(t.path(), p, &row, Some(false), false)
                .join("\n")
                .contains("red CI [legacy-ci]"));
            let details = failure_details(&[(42, row)], &mut |_| panic!("context has no job"));
            assert_eq!(details.len(), 1);
            assert!(details[0].contains("legacy-ci: failing tests unavailable"));
        }
    }

    #[test]
    fn failure_logs_have_a_shared_four_job_budget() {
        let row = serde_json::json!({"statusCheckRollup":[{"name":"build","conclusion":"FAILURE","detailsUrl":"https://github.com/o/r/actions/runs/1/job/123"}]});
        let hold = serde_json::json!({"statusCheckRollup":[{"name":"merge-hold-gate","conclusion":"FAILURE","detailsUrl":"https://github.com/o/r/actions/runs/1/job/456"}]});
        let mut prs = (10..=15).map(|pr| (pr, hold.clone())).collect::<Vec<_>>();
        prs.extend((1..=6).map(|pr| (pr, row.clone())));
        let mut calls = 0;
        let lines = failure_details(&prs, &mut |id| {
            assert_eq!(id, 123, "holds must not consume log slots");
            calls += 1;
            Some("test broken ... FAILED".into())
        });
        assert_eq!(calls, 4);
        assert_eq!(lines.len(), 6);
        assert!(lines[4].contains("unavailable"));
    }

    #[test]
    fn unproven_train_owner_is_unknown_with_inspection_evidence() {
        let t = tempfile::tempdir().unwrap();
        for body in [
            "",
            "legacy owner",
            "<!-- aida-train-owner: host=foreign pid=123 -->",
            &crate::pr_train::owner_line_for_self(),
        ] {
            let mut details = Vec::new();
            assert_eq!(train_pr_running(t.path(), 88, body, &mut details), None);
            assert!(details[0].contains("Train PR-88"));
            assert!(details[0].contains("gh pr view 88"));
            assert!(details[0].contains("live or liveness is unknown"));
            assert!(!details[0].contains("gh pr close"));
        }
    }

    #[test]
    fn digest_commands_match_blockers_and_include_actual_failure_lines() {
        let t = tempfile::tempdir().unwrap();
        let mut p = pr();
        let row = serde_json::json!({"statusCheckRollup":[{"name":"build","conclusion":"FAILURE","detailsUrl":"https://github.com/o/r/actions/runs/1/job/123"}]});
        let ready = pr_digest(t.path(), &p, &row, Some(true), false).join("\n");
        assert!(ready.contains("aida pr ship 42"));
        assert!(!ready.contains("rework"));
        p.is_draft = true;
        let draft = pr_digest(t.path(), &p, &row, Some(false), false).join("\n");
        assert!(draft.contains("draft; gh pr ready 42"));
        assert!(!draft.contains("rework"));
        p.is_draft = false;
        p.labels.push("aida:merge-hold".into());
        let held = pr_digest(t.path(), &p, &row, Some(false), false).join("\n");
        assert!(held.contains("merge hold"));
        assert!(held.contains("aida merge-hold clear 42"));
        assert!(!held.contains("queue rework"));
        p.labels.clear();
        p.title = "work (STORY-2476)".into();
        p.ci_rollup = Some("fail".into());
        let red = pr_digest(t.path(), &p, &row, Some(false), false).join("\n");
        assert!(red.contains("red CI [build]"));
        assert!(red.contains("aida queue rework STORY-2476"));
        assert!(!red.contains("aida pr ship"));
        let rejected = pr_digest(t.path(), &p, &row, Some(false), true).join("\n");
        assert!(rejected.contains("current-head request-changes"));
        let logs = failure_details(&[(42, row.clone())], &mut |id| {
            assert_eq!(id, 123);
            Some("test suite::broken ... FAILED\nthread 'x' panicked at src/x.rs:9:\nassertion failed".into())
        });
        assert!(logs.iter().any(|s| s.contains("suite::broken")));
        assert!(logs.iter().any(|s| s.contains("assertion failed")));
        assert!(
            failure_details(&[(42, row)], &mut |_| None)[0].contains("failing tests unavailable")
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
        let report = run_with_home(t.path(), Some(t.path()), false, 30, &mut |_| {
            panic!("preview sends")
        })
        .unwrap();
        assert!(report.accepted.is_empty());
        assert!(!t.path().join(".aida").exists());
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        let path = t.path().join(".aida/pipeline-stalls.lock");
        let f = std::fs::File::create(&path).unwrap();
        f.lock_exclusive().unwrap();
        assert!(
            run_with_home(t.path(), Some(t.path()), true, 30, &mut |_| panic!())
                .unwrap()
                .busy
        );
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
        assert_eq!(observe(t.path(), Some(t.path())).lanes_running, None);
        let observer = aida_core::pid_observer::PidObserver {
            namespace: Some(42),
            isolated: false,
        };
        assert!(!observer.can_observe(Some(43)));
        assert_eq!(any(&[Some(false), None]), None);
        assert_eq!(any(&[Some(true), None]), Some(true));
    }
}

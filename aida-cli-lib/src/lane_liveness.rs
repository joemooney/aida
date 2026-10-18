//! Deterministic lane-liveness substrate guard (`lane-liveness`).
//!
//! Runs every 10 minutes via `aida schedule tick` with zero LLM tokens and no
//! model-side polling. Checks four classes of fleet stalls:
//!
//! 1. **Lane process / log liveness**: for each registered lane (relay,
//!    factory, headless exec), the process is alive and its log moved within
//!    `N` minutes while it has assigned work.
//! 2. **Review pickup**: an open factory PR has had a `FACTORY READY` at its
//!    current head for more than 30 minutes with no review started.
//! 3. **Gate waits**: a PR has carried `awaiting advisor signoff` or
//!    `needs operator hold clear` for more than 15 minutes.
//! 4. **Orphaned work**: a spec lease is held by a dead process with no pushed
//!    progress for more than 30 minutes.
//!
//! Each finding routes to the owning seat as a due seat job in the schedule
//! ledger; a human-only gate wait additionally pushes an operator notification
//! through `aida notify`. Never relaunches a lane on its own.
// trace:STORY-1688 | ai:antigravity

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::events::{self, Event, EventKind};
use crate::schedule_ledger;
use crate::DoctorFinding;

pub(crate) const CATEGORY: &str = "lane-liveness";

/// Default log-staleness threshold for a lane with assigned work (minutes).
pub(crate) const DEFAULT_LOG_STALE_MINS: i64 = 30;
/// Default review-pickup threshold for a `FACTORY READY` PR at its current head (minutes).
pub(crate) const DEFAULT_REVIEW_PICKUP_MINS: i64 = 30;
/// Default gate-wait threshold for `awaiting advisor signoff` or `needs operator hold clear` (minutes).
pub(crate) const DEFAULT_GATE_WAIT_MINS: i64 = 15;
/// Default orphaned-work threshold for a dead-process spec lease with no pushed progress (minutes).
pub(crate) const DEFAULT_ORPHANED_WORK_MINS: i64 = 30;

/// Notification rule name used when pushing a human-only gate wait to the operator.
pub(crate) const OPERATOR_GATE_NOTIFY_RULE: &str = "lane-liveness-operator-gate";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LaneLivenessPolicy {
    pub log_stale_mins: i64,
    pub review_pickup_mins: i64,
    pub gate_wait_mins: i64,
    pub orphaned_work_mins: i64,
}

impl Default for LaneLivenessPolicy {
    fn default() -> Self {
        Self {
            log_stale_mins: DEFAULT_LOG_STALE_MINS,
            review_pickup_mins: DEFAULT_REVIEW_PICKUP_MINS,
            gate_wait_mins: DEFAULT_GATE_WAIT_MINS,
            orphaned_work_mins: DEFAULT_ORPHANED_WORK_MINS,
        }
    }
}

impl LaneLivenessPolicy {
    pub(crate) fn from_config(cfg: Option<&toml::Value>) -> Self {
        let mut policy = Self::default();
        let Some(table) = cfg
            .and_then(|v| v.get("lane_liveness"))
            .and_then(|v| v.as_table())
        else {
            return policy;
        };
        if let Some(n) = table
            .get("log_stale_mins")
            .and_then(|v| v.as_integer())
            .filter(|n| *n > 0)
        {
            policy.log_stale_mins = n;
        }
        if let Some(n) = table
            .get("review_pickup_mins")
            .and_then(|v| v.as_integer())
            .filter(|n| *n > 0)
        {
            policy.review_pickup_mins = n;
        }
        if let Some(n) = table
            .get("gate_wait_mins")
            .and_then(|v| v.as_integer())
            .filter(|n| *n > 0)
        {
            policy.gate_wait_mins = n;
        }
        if let Some(n) = table
            .get("orphaned_work_mins")
            .and_then(|v| v.as_integer())
            .filter(|n| *n > 0)
        {
            policy.orphaned_work_mins = n;
        }
        policy
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LaneKind {
    Relay,
    Factory,
    HeadlessExec,
}

impl LaneKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Relay => "relay",
            Self::Factory => "factory",
            Self::HeadlessExec => "headless-exec",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "relay" => Some(Self::Relay),
            "factory" | "drain" => Some(Self::Factory),
            "headless-exec" | "headless" | "exec" | "agent" => Some(Self::HeadlessExec),
            _ => None,
        }
    }
}

/// Facts for one registered lane (relay, factory, or headless exec).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegisteredLane {
    pub id: String,
    pub kind: LaneKind,
    #[serde(default)]
    pub pid: Option<u32>,
    pub process_alive: bool,
    #[serde(default)]
    pub log_path: Option<PathBuf>,
    #[serde(default)]
    pub log_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub assigned_work: Vec<String>,
    #[serde(default = "default_advisor_seat")]
    pub owning_seat: String,
    #[serde(default)]
    pub max_log_stale_mins: Option<i64>,
    #[serde(default)]
    pub stopped_reason: Option<String>,
}

fn default_advisor_seat() -> String {
    "advisor".to_string()
}

fn default_merge_seat() -> String {
    "merge".to_string()
}

/// Facts for one open PR relevant to review pickup and gate-wait checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OpenPrFact {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    pub head_sha: String,
    #[serde(default)]
    pub is_factory: bool,
    /// `(ready_head_sha, ready_posted_at)` from a `FACTORY READY` comment.
    #[serde(default)]
    pub factory_ready: Option<(String, DateTime<Utc>)>,
    /// True if a review lease, In-Progress review story, verdict at `head_sha`,
    /// or reviewer comment at `head_sha` exists.
    #[serde(default)]
    pub review_started: bool,
    /// `(gate_head_sha, gate_posted_at)` when the PR carries `awaiting advisor signoff`
    /// at its current head without a subsequent advisor signoff.
    #[serde(default)]
    pub awaiting_advisor_signoff: Option<(String, DateTime<Utc>)>,
    /// `(gate_head_sha, gate_posted_at)` when the PR carries `needs operator hold clear`
    /// at its current head without a subsequent hold clearance.
    #[serde(default)]
    pub needs_operator_hold_clear: Option<(String, DateTime<Utc>)>,
    /// Seat responsible for picking up the review (defaults to `"merge"`).
    #[serde(default = "default_merge_seat")]
    pub review_seat: String,
}

/// Facts for one spec lease relevant to the orphaned-work check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OrphanedLeaseFact {
    pub lease_id: String,
    pub spec_id: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub pid: Option<u32>,
    pub process_alive: bool,
    pub last_progress_at: DateTime<Utc>,
    #[serde(default = "default_advisor_seat")]
    pub owning_seat: String,
}

/// Complete snapshot evaluated by `evaluate_snapshot`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LaneLivenessSnapshot {
    #[serde(default)]
    pub lanes: Vec<RegisteredLane>,
    #[serde(default)]
    pub open_prs: Vec<OpenPrFact>,
    #[serde(default)]
    pub leases: Vec<OrphanedLeaseFact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FindingKind {
    DeadLane,
    StaleLog,
    UnpickedReadyPr,
    StaleGateWait,
    OrphanedWork,
}

impl FindingKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DeadLane => "dead-lane",
            Self::StaleLog => "stale-log",
            Self::UnpickedReadyPr => "unpicked-ready-pr",
            Self::StaleGateWait => "stale-gate-wait",
            Self::OrphanedWork => "orphaned-work",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LaneLivenessFinding {
    pub id: String,
    pub kind: FindingKind,
    pub owning_seat: String,
    pub human_only_gate: bool,
    pub summary: String,
    pub action: String,
}

impl LaneLivenessFinding {
    pub(crate) fn into_doctor_finding(self) -> DoctorFinding {
        DoctorFinding {
            category: CATEGORY.to_string(),
            id: self.id,
            summary: self.summary,
            action: self.action,
            safe_heal: false,
        }
    }
}

/// Pure evaluation over a [`LaneLivenessSnapshot`]. Zero I/O, deterministic.
// trace:STORY-1688 | ai:antigravity
pub(crate) fn evaluate_snapshot(
    snapshot: &LaneLivenessSnapshot,
    policy: &LaneLivenessPolicy,
    now: DateTime<Utc>,
) -> Vec<LaneLivenessFinding> {
    let mut findings = Vec::new();

    // 1. Lane process / log liveness
    for lane in &snapshot.lanes {
        if lane.assigned_work.is_empty() {
            continue;
        }
        let work_desc = lane.assigned_work.join(", ");
        let pid_desc = lane
            .pid
            .map(|p| format!("pid {p}"))
            .unwrap_or_else(|| "no pid".to_string());

        if !lane.process_alive {
            let reason_suffix = lane
                .stopped_reason
                .as_deref()
                .filter(|r| !r.is_empty())
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            findings.push(LaneLivenessFinding {
                id: format!("{CATEGORY}:dead-lane:{}", lane.id),
                kind: FindingKind::DeadLane,
                owning_seat: lane.owning_seat.clone(),
                human_only_gate: false,
                summary: format!(
                    "{} lane `{}` is dead ({pid_desc}){reason_suffix} while assigned work [{}] remains incomplete",
                    lane.kind.as_str(),
                    lane.id,
                    work_desc,
                ),
                action: format!(
                    "route to seat `{}`: inspect lane `{}` log{} and reassign or resume [{}]",
                    lane.owning_seat,
                    lane.id,
                    lane.log_path
                        .as_ref()
                        .map(|p| format!(" ({})", p.display()))
                        .unwrap_or_default(),
                    work_desc,
                ),
            });
            continue;
        }

        let max_stale = lane
            .max_log_stale_mins
            .filter(|m| *m > 0)
            .unwrap_or(policy.log_stale_mins);
        if let Some(updated_at) = lane.log_updated_at {
            let age_mins = (now - updated_at).num_minutes();
            if age_mins > max_stale {
                let reason_suffix = lane
                    .stopped_reason
                    .as_deref()
                    .filter(|r| !r.is_empty())
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default();
                findings.push(LaneLivenessFinding {
                    id: format!("{CATEGORY}:stale-log:{}", lane.id),
                    kind: FindingKind::StaleLog,
                    owning_seat: lane.owning_seat.clone(),
                    human_only_gate: false,
                    summary: format!(
                        "{} lane `{}` ({pid_desc}) is alive but its log has not moved for {}m (threshold {}m){reason_suffix} while assigned [{}]",
                        lane.kind.as_str(),
                        lane.id,
                        age_mins,
                        max_stale,
                        work_desc,
                    ),
                    action: format!(
                        "route to seat `{}`: inspect stalled {} lane `{}` and unblock or restart [{}]",
                        lane.owning_seat,
                        lane.kind.as_str(),
                        lane.id,
                        work_desc,
                    ),
                });
            }
        }
    }

    // 2. Review pickup & 3. Gate waits
    for pr in &snapshot.open_prs {
        if pr.is_factory && !pr.review_started {
            if let Some((ref ready_sha, ready_at)) = pr.factory_ready {
                if sha_matches(&pr.head_sha, ready_sha) {
                    let wait_mins = (now - ready_at).num_minutes();
                    if wait_mins > policy.review_pickup_mins {
                        findings.push(LaneLivenessFinding {
                            id: format!("{CATEGORY}:unpicked-ready-pr:PR-{}", pr.number),
                            kind: FindingKind::UnpickedReadyPr,
                            owning_seat: pr.review_seat.clone(),
                            human_only_gate: false,
                            summary: format!(
                                "factory PR #{} ({}) has carried FACTORY READY at head {} for {}m (threshold {}m) with no review started",
                                pr.number,
                                pr.title,
                                short_sha(&pr.head_sha),
                                wait_mins,
                                policy.review_pickup_mins,
                            ),
                            action: format!(
                                "route to seat `{}`: start independent review with `aida queue work PR-{} --role reviewer --no-human --vendor claude`",
                                pr.review_seat,
                                pr.number,
                            ),
                        });
                    }
                }
            }
        }

        if let Some((ref gate_sha, gate_at)) = pr.awaiting_advisor_signoff {
            if sha_matches(&pr.head_sha, gate_sha) {
                let wait_mins = (now - gate_at).num_minutes();
                if wait_mins > policy.gate_wait_mins {
                    findings.push(LaneLivenessFinding {
                        id: format!("{CATEGORY}:gate-wait-advisor:PR-{}", pr.number),
                        kind: FindingKind::StaleGateWait,
                        owning_seat: "advisor".to_string(),
                        human_only_gate: false,
                        summary: format!(
                            "PR #{} ({}) has waited on `awaiting advisor signoff` at head {} for {}m (threshold {}m)",
                            pr.number,
                            pr.title,
                            short_sha(&pr.head_sha),
                            wait_mins,
                            policy.gate_wait_mins,
                        ),
                        action: format!(
                            "route to seat `advisor`: review and post advisor signoff on PR #{} at {}",
                            pr.number,
                            short_sha(&pr.head_sha),
                        ),
                    });
                }
            }
        }

        if let Some((ref gate_sha, gate_at)) = pr.needs_operator_hold_clear {
            if sha_matches(&pr.head_sha, gate_sha) {
                let wait_mins = (now - gate_at).num_minutes();
                if wait_mins > policy.gate_wait_mins {
                    findings.push(LaneLivenessFinding {
                        id: format!("{CATEGORY}:gate-wait-operator:PR-{}", pr.number),
                        kind: FindingKind::StaleGateWait,
                        owning_seat: "operator".to_string(),
                        human_only_gate: true,
                        summary: format!(
                            "PR #{} ({}) has waited on `needs operator hold clear` at head {} for {}m (threshold {}m)",
                            pr.number,
                            pr.title,
                            short_sha(&pr.head_sha),
                            wait_mins,
                            policy.gate_wait_mins,
                        ),
                        action: format!(
                            "notify operator: run `aida merge-hold clear {}` at an interactive TTY, then ship PR #{}",
                            pr.number,
                            pr.number,
                        ),
                    });
                }
            }
        }
    }

    // 4. Orphaned work
    for lease in &snapshot.leases {
        if lease.process_alive {
            continue;
        }
        let stale_mins = (now - lease.last_progress_at).num_minutes();
        if stale_mins > policy.orphaned_work_mins {
            let pid_desc = lease
                .pid
                .map(|p| format!("pid {p}"))
                .unwrap_or_else(|| "no pid".to_string());
            findings.push(LaneLivenessFinding {
                id: format!("{CATEGORY}:orphaned-work:{}", lease.spec_id),
                kind: FindingKind::OrphanedWork,
                owning_seat: lease.owning_seat.clone(),
                human_only_gate: false,
                summary: format!(
                    "spec `{}` lease `{}` (owner `{}`, {pid_desc}) is held by a dead process with no pushed progress for {}m (threshold {}m)",
                    lease.spec_id,
                    lease.lease_id,
                    lease.owner,
                    stale_mins,
                    policy.orphaned_work_mins,
                ),
                action: format!(
                    "route to seat `{}`: salvage or resume `{}` (`aida queue work {}`) or reap the dead lease (`aida session reap`)",
                    lease.owning_seat,
                    lease.spec_id,
                    lease.spec_id,
                ),
            });
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    findings
}

fn sha_matches(head: &str, candidate: &str) -> bool {
    let h = head.trim();
    let c = candidate.trim();
    if h.is_empty() || c.is_empty() {
        return false;
    }
    if c == "*" {
        return true;
    }
    h.starts_with(c) || c.starts_with(h)
}

fn short_sha(sha: &str) -> &str {
    let s = sha.trim();
    &s[..s.len().min(10)]
}

/// Route findings to their owning seats as due seat jobs in the schedule
/// ledger via the canonical CAS store writer (`schedule_ledger::write_cas`),
/// and send an operator push notification for any human-only gate wait.
// trace:STORY-1688 | ai:antigravity
pub(crate) fn route_findings(
    project_root: &Path,
    findings: &[LaneLivenessFinding],
    policy: &LaneLivenessPolicy,
    now: DateTime<Utc>,
) {
    if findings.is_empty() {
        return;
    }

    let store_root = crate::detect_distributed_store_from(project_root)
        .unwrap_or_else(|| project_root.join(".aida-store"));

    // Group findings by owning seat
    let mut by_seat: BTreeMap<String, Vec<&LaneLivenessFinding>> = BTreeMap::new();
    for f in findings {
        by_seat.entry(f.owning_seat.clone()).or_default().push(f);
    }

    for (seat, seat_findings) in &by_seat {
        let job_name = format!("lane-liveness-{seat}");
        let summary_joined = seat_findings
            .iter()
            .map(|f| f.summary.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let already_due = schedule_ledger::load(&store_root, &job_name)
            .as_ref()
            .is_some_and(|l| l.due_since.is_some());
        let trip_id = format!(
            "{job_name}@{}",
            now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        );
        if let Err(err) = schedule_ledger::write_cas(&store_root, &job_name, |ledger| {
            if ledger.due_since.is_none() {
                ledger.due_since = Some(now);
            }
            ledger.due_reason = Some(format!("on CronJobFired:{job_name}"));
            ledger.note = Some(summary_joined.clone());
            ledger.due_failure = Some(schedule_ledger::RoutedFailure {
                trip_id: trip_id.clone(),
                performance: Vec::new(),
                job: Some(CATEGORY.to_string()),
                error: Some(summary_joined.clone()),
            });
        }) {
            eprintln!("warning: could not write schedule ledger for '{job_name}': {err}");
        }
        if !already_due {
            events::emit(
                project_root,
                &Event::new(
                    None,
                    "",
                    EventKind::CronJobFired {
                        job: job_name,
                        seat: seat.clone(),
                    },
                ),
            );
        }
    }

    // Human-only gate waits push an operator notification via `aida notify`.
    let human_gates: Vec<&LaneLivenessFinding> =
        findings.iter().filter(|f| f.human_only_gate).collect();
    if !human_gates.is_empty() {
        let title = format!(
            "AIDA operator gate wait ({} PR(s) waiting >{}m)",
            human_gates.len(),
            policy.gate_wait_mins
        );
        let message = human_gates
            .iter()
            .map(|f| format!("{}\n  -> {}", f.summary, f.action))
            .collect::<Vec<_>>()
            .join("\n");
        let _ =
            crate::notify::send_direct(project_root, OPERATOR_GATE_NOTIFY_RULE, &title, &message);
    }
}

/// Scan `<project_root>` for lane-liveness findings without routing side effects.
pub(crate) fn scan(project_root: &Path) -> Vec<DoctorFinding> {
    scan_at(project_root, Utc::now())
        .into_iter()
        .map(LaneLivenessFinding::into_doctor_finding)
        .collect()
}

/// Scan `<project_root>` and, when `route` is true (invoked from the scheduled
/// tick), route findings to owning seats and push operator notifications for
/// human-only gate waits.
// trace:STORY-1688 | ai:antigravity
pub(crate) fn scan_and_route(project_root: &Path, route: bool) -> Vec<DoctorFinding> {
    scan_and_route_at(project_root, Utc::now(), route)
}

pub(crate) fn scan_and_route_at(
    project_root: &Path,
    now: DateTime<Utc>,
    route: bool,
) -> Vec<DoctorFinding> {
    let cfg = crate::read_project_config_value(project_root);
    let policy = LaneLivenessPolicy::from_config(cfg.as_ref());
    let snapshot = load_snapshot(project_root, now);
    let findings = evaluate_snapshot(&snapshot, &policy, now);
    if route && !findings.is_empty() {
        route_findings(project_root, &findings, &policy, now);
    }
    findings
        .into_iter()
        .map(LaneLivenessFinding::into_doctor_finding)
        .collect()
}

pub(crate) fn scan_at(project_root: &Path, now: DateTime<Utc>) -> Vec<LaneLivenessFinding> {
    let cfg = crate::read_project_config_value(project_root);
    let policy = LaneLivenessPolicy::from_config(cfg.as_ref());
    let snapshot = load_snapshot(project_root, now);
    evaluate_snapshot(&snapshot, &policy, now)
}

fn parse_rfc3339_or_git_ts(raw: &str) -> Option<DateTime<Utc>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(trimmed)
        .or_else(|_| DateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%z"))
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn file_mtime_utc(path: &Path) -> Option<DateTime<Utc>> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(DateTime::<Utc>::from(modified))
}

/// Register a headless-exec child lane in `<project_root>/.aida/lanes/headless-<id>.json`
/// when `aida` spawns a headless vendor session (`run_vendor_headless_child` or
/// `spawn_claude_headless_resume_granted`).
// trace:STORY-1688 | ai:antigravity
pub(crate) fn register_headless_lane_for_child(
    log_path: &Path,
    session_id: &str,
    lease_id: Option<&str>,
    vendor: &str,
    child_pid: u32,
) {
    let Some(project_root) = log_path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
    else {
        return;
    };
    let lane_key = lease_id
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(session_id);
    if lane_key.trim().is_empty() {
        return;
    }
    let assigned_work: Vec<String> = crate::leases::list_leases(project_root)
        .into_iter()
        .filter(|l| {
            lease_id.is_some_and(|id| l.id == id)
                || l.id == session_id
                || l.scope.eq_ignore_ascii_case(lane_key)
        })
        .map(|l| l.scope)
        .filter(|s| !s.trim().is_empty())
        .collect();
    if assigned_work.is_empty() {
        return;
    }
    let lanes_dir = project_root.join(".aida").join("lanes");
    let _ = std::fs::create_dir_all(&lanes_dir);
    let lane_id = format!("{vendor}-headless-{lane_key}");
    let pid_start_time = crate::process_probe::process_start_identity(child_pid);
    let payload = serde_json::json!({
        "id": lane_id,
        "kind": "headless-exec",
        "pid": child_pid,
        "pid_start_time": pid_start_time,
        "log_path": log_path,
        "assigned_work": assigned_work,
        "owning_seat": "advisor",
    });
    let file_path = lanes_dir.join(format!("headless-{lane_key}.json"));
    let _ = aida_core::write_atomic(&file_path, payload.to_string().as_bytes());
}

/// Update or remove a headless-exec child lane record when the child exits.
/// Clean exits remove the transient lane descriptor; non-zero exits mark the
/// lane dead with its exit reason so the next `lane-liveness` tick reports the
/// dead headless run if its assigned work remains incomplete.
// trace:STORY-1688 | ai:antigravity
pub(crate) fn finish_headless_lane_for_child(
    log_path: &Path,
    session_id: &str,
    lease_id: Option<&str>,
    status: Option<std::process::ExitStatus>,
) {
    let Some(project_root) = log_path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
    else {
        return;
    };
    let lane_key = lease_id
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(session_id);
    if lane_key.trim().is_empty() {
        return;
    }
    let file_path = project_root
        .join(".aida")
        .join("lanes")
        .join(format!("headless-{lane_key}.json"));
    if status.is_some_and(|s| s.success()) {
        let _ = std::fs::remove_file(&file_path);
        return;
    }
    let Ok(text) = std::fs::read_to_string(&file_path) else {
        return;
    };
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    if let Some(obj) = val.as_object_mut() {
        obj.insert("alive".into(), serde_json::Value::Bool(false));
        let reason = match status {
            Some(s) => format!("exited with {s}"),
            None => "wait failed".to_string(),
        };
        obj.insert("stopped_reason".into(), serde_json::Value::String(reason));
        let _ = aida_core::write_atomic(&file_path, val.to_string().as_bytes());
    }
}

/// Load a [`LaneLivenessSnapshot`] from `<project_root>/.aida/`.
// trace:STORY-1688 | ai:antigravity
pub(crate) fn load_snapshot(project_root: &Path, now: DateTime<Utc>) -> LaneLivenessSnapshot {
    let mut snapshot = LaneLivenessSnapshot::default();
    let aida_dir = project_root.join(".aida");

    // Optional full snapshot file `.aida/lane-liveness/snapshot.json`
    let snapshot_file = aida_dir.join("lane-liveness").join("snapshot.json");
    if let Ok(text) = std::fs::read_to_string(&snapshot_file) {
        if let Ok(loaded) = serde_json::from_str::<LaneLivenessSnapshot>(&text) {
            snapshot.lanes.extend(loaded.lanes);
            snapshot.open_prs.extend(loaded.open_prs);
            snapshot.leases.extend(loaded.leases);
        }
    }

    // 1a. Explicit lane descriptors in `.aida/lanes/*.toml` or `.aida/lanes/*.json`
    // (written by headless child launchers and operator/fixture registrations)
    load_explicit_lanes(&aida_dir.join("lanes"), &mut snapshot.lanes);

    // 1b. Relay runs in `.aida/relay/*/relay.status`
    load_relay_lanes(project_root, &mut snapshot.lanes);

    // 1c. Factory / drain runs in `.aida/drain-state.json`
    load_drain_lane(project_root, &mut snapshot.lanes);

    // 1d. Registered drivers in `.aida/drivers/*.json`
    load_driver_lanes(project_root, now, &mut snapshot.lanes);

    // 2 & 3. Open PRs from `.aida/lane-liveness/prs.json` or live `gh pr list --json`,
    // overlaid with local reviewer leases and recorded review verdicts.
    load_open_prs(project_root, &mut snapshot.open_prs);

    // 4. Spec leases from `.aida/sessions/*.toml` and `.aida/lane-liveness/leases.json`
    load_orphaned_leases(project_root, now, &mut snapshot.leases);

    snapshot
}

#[derive(Debug, Deserialize)]
struct ExplicitLaneFile {
    id: Option<String>,
    name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    pid_start_time: Option<String>,
    #[serde(default)]
    alive: Option<bool>,
    #[serde(default)]
    log_path: Option<PathBuf>,
    #[serde(default)]
    log_updated_at: Option<String>,
    #[serde(default)]
    assigned_work: Vec<String>,
    #[serde(default)]
    current_spec: Option<String>,
    #[serde(default)]
    owning_seat: Option<String>,
    #[serde(default)]
    seat: Option<String>,
    #[serde(default)]
    max_log_stale_mins: Option<i64>,
    #[serde(default)]
    stopped_reason: Option<String>,
}

fn load_explicit_lanes(lanes_dir: &Path, out: &mut Vec<RegisteredLane>) {
    let Ok(entries) = std::fs::read_dir(lanes_dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let parsed: Option<ExplicitLaneFile> = match ext {
            "json" => serde_json::from_str(&text).ok(),
            "toml" => toml::from_str(&text).ok(),
            _ => None,
        };
        let Some(rec) = parsed else {
            continue;
        };
        let id = rec
            .id
            .or(rec.name)
            .or_else(|| path.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let Some(kind) = rec
            .kind
            .as_deref()
            .map_or(Some(LaneKind::Factory), LaneKind::parse)
        else {
            continue;
        };
        let process_alive = rec.alive.unwrap_or_else(|| {
            rec.pid.is_some_and(|pid| {
                aida_core::liveness::process_identity_is_alive(pid, rec.pid_start_time.as_deref())
            })
        });
        let log_updated_at = rec
            .log_updated_at
            .as_deref()
            .and_then(parse_rfc3339_or_git_ts)
            .or_else(|| rec.log_path.as_deref().and_then(file_mtime_utc));
        let mut assigned_work = rec.assigned_work;
        if let Some(spec) = rec.current_spec.filter(|s| !s.trim().is_empty()) {
            if !assigned_work.contains(&spec) {
                assigned_work.push(spec);
            }
        }
        let owning_seat = rec
            .owning_seat
            .or(rec.seat)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(default_advisor_seat);
        out.push(RegisteredLane {
            id,
            kind,
            pid: rec.pid,
            process_alive,
            log_path: rec.log_path,
            log_updated_at,
            assigned_work,
            owning_seat,
            max_log_stale_mins: rec.max_log_stale_mins,
            stopped_reason: rec.stopped_reason,
        });
    }
}

fn load_relay_lanes(project_root: &Path, out: &mut Vec<RegisteredLane>) {
    let relay_root = project_root.join(".aida").join("relay");
    let Ok(entries) = std::fs::read_dir(&relay_root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let seen_ids: BTreeSet<String> = out.iter().map(|l| l.id.clone()).collect();
    for dir in dirs {
        let status_path = dir.join("relay.status");
        let Ok(text) = std::fs::read_to_string(&status_path) else {
            continue;
        };
        let status: BTreeMap<String, String> = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let run = status
            .get("run")
            .filter(|r| !r.is_empty())
            .cloned()
            .unwrap_or_else(|| {
                dir.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
        if run.is_empty() || seen_ids.contains(&run) {
            continue;
        }
        // Only inspect relay runs that declare assigned work (`ticket=` or `assigned_work=`)
        // so historical finished relay directories stay with `relay-stall` unless scoped.
        let mut assigned_work: Vec<String> = status
            .get("ticket")
            .into_iter()
            .chain(status.get("assigned_work"))
            .flat_map(|s| s.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        assigned_work.dedup();
        if assigned_work.is_empty() {
            continue;
        }
        let state = status.get("state").map(String::as_str).unwrap_or("");
        let pid = status
            .get("pid")
            .and_then(|p| p.parse::<u32>().ok())
            .filter(|p| *p > 0);
        let updated = status
            .get("updated")
            .and_then(|u| parse_rfc3339_or_git_ts(u));
        let process_alive = match state {
            "blocked" | "exited" => false,
            _ => relay_owner_alive(&dir, pid, updated),
        };
        let (newest_log, newest_log_ts) = newest_session_log(&dir.join("sessions"));
        let log_updated_at = newest_log_ts.or(updated);
        let seat = status
            .get("seat")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(default_advisor_seat);
        let stopped_reason = status
            .get("reason")
            .filter(|r| !r.is_empty())
            .cloned()
            .or_else(|| {
                matches!(state, "blocked" | "exited").then(|| format!("relay state={state}"))
            });
        out.push(RegisteredLane {
            id: run,
            kind: LaneKind::Relay,
            pid,
            process_alive,
            log_path: newest_log,
            log_updated_at,
            assigned_work,
            owning_seat: seat,
            max_log_stale_mins: None,
            stopped_reason,
        });
    }
}

/// Production factory/drain lane source: `.aida/drain-state.json`.
// trace:STORY-1688 | ai:antigravity
fn load_drain_lane(project_root: &Path, out: &mut Vec<RegisteredLane>) {
    let Some(state) = crate::drain_state::DrainState::read(project_root) else {
        return;
    };
    let assigned_work: Vec<String> = state
        .members
        .iter()
        .filter(|m| {
            m.state != crate::drain_state::STATE_COMPLETED
                && m.state != crate::drain_state::STATE_FAILED
        })
        .map(|m| m.spec.clone())
        .collect();
    if assigned_work.is_empty() {
        return;
    }
    let id = match state.batch.as_deref().filter(|b| !b.is_empty()) {
        Some(batch) => format!("drain-batch-{batch}"),
        None => format!("drain-{}", state.mode),
    };
    if out.iter().any(|l| l.id == id) {
        return;
    }
    let process_alive = state.orchestrator_alive() && state.stopped_at.is_none();
    let state_path = crate::drain_state::drain_state_path(project_root);
    let (headless_log, headless_ts) =
        newest_headless_log(project_root, state.current_session_id.as_deref());
    let state_ts = state
        .phase_started_at
        .as_deref()
        .and_then(parse_rfc3339_or_git_ts)
        .or_else(|| parse_rfc3339_or_git_ts(&state.started_at))
        .or_else(|| file_mtime_utc(&state_path));
    let log_updated_at = match (headless_ts, state_ts) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    out.push(RegisteredLane {
        id,
        kind: LaneKind::Factory,
        pid: Some(state.orchestrator_pid),
        process_alive,
        log_path: headless_log.or(Some(state_path)),
        log_updated_at,
        assigned_work,
        owning_seat: default_advisor_seat(),
        max_log_stale_mins: None,
        stopped_reason: state.stopped_reason,
    });
}

/// Production driver-registry lane source: `.aida/drivers/*.json`.
// trace:STORY-1688 | ai:antigravity
fn load_driver_lanes(project_root: &Path, now: DateTime<Utc>, out: &mut Vec<RegisteredLane>) {
    let records = aida_core::driver_registry::read_all(&crate::driver_view::driver_registry_root(
        project_root,
    ));
    if records.is_empty() {
        return;
    }
    let leases = crate::leases::list_leases(project_root);
    let seen_ids: BTreeSet<String> = out.iter().map(|l| l.id.clone()).collect();
    for rec in records {
        if seen_ids.contains(&rec.id) || seen_ids.contains(&rec.label()) {
            continue;
        }
        let label = rec.label();
        let mut assigned_work: Vec<String> = leases
            .iter()
            .filter(|l| l.driver.as_deref().map(str::trim) == Some(label.as_str()))
            .map(|l| l.scope.clone())
            .filter(|s| !s.trim().is_empty())
            .collect();
        if let Some(sel) = rec.selector.as_ref().filter(|s| !s.trim().is_empty()) {
            if !assigned_work.contains(sel) {
                assigned_work.push(sel.clone());
            }
        }
        assigned_work.dedup();
        if assigned_work.is_empty() {
            continue;
        }
        let kind = match rec.kind {
            aida_core::driver_registry::DriverKind::Relay => LaneKind::Relay,
            aida_core::driver_registry::DriverKind::Drain => LaneKind::Factory,
            aida_core::driver_registry::DriverKind::Agent => LaneKind::HeadlessExec,
        };
        let process_alive = rec.is_live(now);
        let handoff_path = rec.handoff.as_deref().map(PathBuf::from);
        let handoff_ts = handoff_path.as_deref().and_then(file_mtime_utc);
        let log_updated_at = match handoff_ts {
            Some(ts) => Some(ts.max(rec.heartbeat_at)),
            None => Some(rec.heartbeat_at),
        };
        let owning_seat = rec
            .seat
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(default_advisor_seat);
        out.push(RegisteredLane {
            id: rec.id,
            kind,
            pid: Some(rec.pid),
            process_alive,
            log_path: handoff_path,
            log_updated_at,
            assigned_work,
            owning_seat,
            max_log_stale_mins: None,
            stopped_reason: None,
        });
    }
}

fn newest_headless_log(
    project_root: &Path,
    session_id: Option<&str>,
) -> (Option<PathBuf>, Option<DateTime<Utc>>) {
    let dir = project_root.join(".aida").join("headless-logs");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (None, None);
    };
    let suffix = session_id
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("-{s}.jsonl"));
    let mut best: Option<(PathBuf, DateTime<Utc>)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.ends_with(".jsonl") {
            continue;
        }
        if let Some(ref want) = suffix {
            if !name.ends_with(want) {
                continue;
            }
        }
        if let Some(ts) = file_mtime_utc(&path) {
            if best.as_ref().is_none_or(|(_, b_ts)| ts > *b_ts) {
                best = Some((path, ts));
            }
        }
    }
    match best {
        Some((p, ts)) => (Some(p), Some(ts)),
        None => (None, None),
    }
}

fn relay_owner_alive(run_dir: &Path, pid: Option<u32>, updated: Option<DateTime<Utc>>) -> bool {
    let mut lock_held = false;
    match std::fs::File::open(run_dir.join("relay.lock")) {
        Ok(lock) => match FileExt::try_lock_shared(&lock) {
            Ok(()) => {
                let _ = FileExt::unlock(&lock);
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || aida_core::file_lock::is_lock_contended(&error) =>
            {
                lock_held = true;
            }
            Err(_) => {}
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
    let Some(pid) = pid else {
        return lock_held;
    };
    if !crate::process_probe::pid_is_alive(pid) {
        return false;
    }
    if let (Some(identity), Some(updated)) =
        (crate::process_probe::process_start_identity(pid), updated)
    {
        if let Ok(started) = DateTime::parse_from_rfc3339(&identity) {
            return started.with_timezone(&Utc) <= updated;
        }
    }
    true
}

fn newest_session_log(sessions_dir: &Path) -> (Option<PathBuf>, Option<DateTime<Utc>>) {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return (None, None);
    };
    let mut best: Option<(PathBuf, DateTime<Utc>)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("log") {
            continue;
        }
        if let Some(ts) = file_mtime_utc(&path) {
            if best.as_ref().is_none_or(|(_, b_ts)| ts > *b_ts) {
                best = Some((path, ts));
            }
        }
    }
    match best {
        Some((p, ts)) => (Some(p), Some(ts)),
        None => (None, None),
    }
}

#[derive(Debug, Deserialize)]
struct GhPrComment {
    #[serde(default)]
    body: String,
    #[serde(default, alias = "createdAt", alias = "created_at")]
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct GhPrLabel {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawPrEntry {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default, alias = "headRefOid")]
    head_sha: String,
    #[serde(default)]
    is_factory: Option<bool>,
    #[serde(default)]
    labels: Vec<GhPrLabel>,
    #[serde(default)]
    comments: Vec<GhPrComment>,
    #[serde(default)]
    factory_ready: Option<(String, DateTime<Utc>)>,
    #[serde(default)]
    review_started: Option<bool>,
    #[serde(default)]
    awaiting_advisor_signoff: Option<(String, DateTime<Utc>)>,
    #[serde(default)]
    needs_operator_hold_clear: Option<(String, DateTime<Utc>)>,
    #[serde(default)]
    review_seat: Option<String>,
}

fn fetch_open_prs_from_gh(project_root: &Path) -> Option<String> {
    if cfg!(test) && std::env::var_os("AIDA_LANE_LIVENESS_GH").is_none() {
        return None;
    }
    if !project_root.join(".git").exists() {
        return None;
    }
    let gh_bin = crate::resolve_forge_cli(crate::forge::ForgeKind::GitHub)?;
    let mut cmd = std::process::Command::new(gh_bin);
    cmd.current_dir(project_root)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number,title,headRefOid,labels,comments",
        ]);
    let out = crate::command_output_with_timeout(cmd, std::time::Duration::from_secs(15))?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn load_open_prs(project_root: &Path, out: &mut Vec<OpenPrFact>) {
    let prs_path = project_root
        .join(".aida")
        .join("lane-liveness")
        .join("prs.json");
    let Some(text) = std::fs::read_to_string(&prs_path)
        .ok()
        .or_else(|| fetch_open_prs_from_gh(project_root))
    else {
        return;
    };
    let Ok(raw_list) = serde_json::from_str::<Vec<RawPrEntry>>(&text) else {
        return;
    };

    let leases = crate::leases::list_leases(project_root);
    let now = Utc::now();

    for raw in raw_list {
        let mut is_factory = raw.is_factory.unwrap_or_else(|| {
            raw.labels
                .iter()
                .any(|l| l.name.eq_ignore_ascii_case("factory"))
        });
        let mut factory_ready = raw.factory_ready;
        let mut awaiting_advisor_signoff = raw.awaiting_advisor_signoff;
        let mut needs_operator_hold_clear = raw.needs_operator_hold_clear;
        let mut comment_review_started = false;

        for c in &raw.comments {
            let Some(ts) = parse_rfc3339_or_git_ts(&c.created_at) else {
                continue;
            };
            let body_lower = c.body.to_ascii_lowercase();
            if c.body.contains("FACTORY READY") {
                is_factory = true;
                let sha =
                    extract_sha_after(&c.body, "green at ").unwrap_or_else(|| raw.head_sha.clone());
                factory_ready = Some((sha, ts));
            }
            if body_lower.contains("review (merge):")
                || body_lower.contains("verdict: approved")
                || body_lower.contains("verdict: request-changes")
            {
                if raw.head_sha.is_empty() || comment_mentions_sha(&c.body, &raw.head_sha) {
                    comment_review_started = true;
                }
            }
            if body_lower.contains("awaiting advisor signoff") {
                let sha = extract_sha_after(&c.body, "approved at ")
                    .unwrap_or_else(|| raw.head_sha.clone());
                awaiting_advisor_signoff = Some((sha, ts));
            }
            if body_lower.contains("advisor signoff")
                && !body_lower.contains("awaiting advisor signoff")
            {
                awaiting_advisor_signoff = None;
            }
            if body_lower.contains("needs operator hold clear") {
                let sha = extract_sha_after(&c.body, "at ")
                    .or_else(|| extract_sha_after(&c.body, "head "))
                    .unwrap_or_else(|| raw.head_sha.clone());
                needs_operator_hold_clear = Some((sha, ts));
            }
            if (body_lower.contains("merge-hold cleared")
                || body_lower.contains("hold cleared")
                || body_lower.contains("cleared merge hold")
                || body_lower.contains("cleared merge-hold")
                || body_lower.contains("operator hold cleared"))
                && !body_lower.contains("needs operator hold clear")
            {
                needs_operator_hold_clear = None;
            }
        }

        // Check local review leases (`PR-<n>`) and `.aida/review-verdicts/PR-<n>.json`
        let has_live_review_lease = leases.iter().any(|l| {
            l.scope.eq_ignore_ascii_case(&format!("PR-{}", raw.number))
                && matches!(crate::lease_state_for(l, &[], now), crate::LeaseState::Live)
        });
        let verdict_path = project_root
            .join(".aida")
            .join("review-verdicts")
            .join(format!("PR-{}.json", raw.number));
        let has_verdict_at_head = std::fs::read_to_string(&verdict_path)
            .ok()
            .and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok())
            .and_then(|v| {
                v.get("reviewed_sha")
                    .and_then(|s| s.as_str())
                    .map(|s| sha_matches(&raw.head_sha, s))
            })
            .unwrap_or(false);

        let review_started = raw
            .review_started
            .unwrap_or(has_live_review_lease || has_verdict_at_head || comment_review_started);

        out.push(OpenPrFact {
            number: raw.number,
            title: raw.title,
            head_sha: raw.head_sha,
            is_factory,
            factory_ready,
            review_started,
            awaiting_advisor_signoff,
            needs_operator_hold_clear,
            review_seat: raw.review_seat.unwrap_or_else(default_merge_seat),
        });
    }
}

fn comment_mentions_sha(body: &str, head_sha: &str) -> bool {
    body.split(|c: char| !c.is_ascii_hexdigit())
        .filter(|tok| (7..=40).contains(&tok.len()))
        .any(|tok| sha_matches(head_sha, tok))
}

fn extract_sha_after(text: &str, needle: &str) -> Option<String> {
    let idx = text.find(needle)?;
    let rest = &text[idx + needle.len()..];
    let token: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    (token.len() >= 7).then_some(token)
}

fn load_orphaned_leases(project_root: &Path, now: DateTime<Utc>, out: &mut Vec<OrphanedLeaseFact>) {
    let leases_json = project_root
        .join(".aida")
        .join("lane-liveness")
        .join("leases.json");
    if let Ok(text) = std::fs::read_to_string(&leases_json) {
        if let Ok(loaded) = serde_json::from_str::<Vec<OrphanedLeaseFact>>(&text) {
            out.extend(loaded);
        }
    }

    let seen_specs: BTreeSet<String> = out.iter().map(|l| l.spec_id.clone()).collect();
    let live_drivers = aida_core::driver_registry::live_drivers(
        &crate::driver_view::driver_registry_root(project_root),
        now,
    );

    for lease in crate::leases::list_leases(project_root) {
        let scope = lease.scope.trim();
        if scope.is_empty()
            || crate::parse_review_scope(scope).is_some()
            || scope.eq_ignore_ascii_case("harness-worktree")
            || lease.manual_enter_at.is_some()
            || lease.interrupted_at.is_some()
            || seen_specs.contains(scope)
        {
            continue;
        }
        // Check if vouched for by a live driver
        if lease
            .driver
            .as_deref()
            .is_some_and(|d| live_drivers.iter().any(|r| r.label() == d.trim()))
        {
            continue;
        }
        let pid = lease.active_pid.or(lease.creator_pid);
        let pid_start = if lease.active_pid.is_some() {
            lease.active_pid_start_time.as_deref()
        } else {
            lease.creator_pid_start_time.as_deref()
        };
        let Some(pid_val) = pid else {
            continue;
        };
        let process_alive = aida_core::liveness::process_identity_is_alive(pid_val, pid_start);
        let last_progress_at = pushed_branch_progress_ts(&lease.worktree_path, &lease.branch)
            .or_else(|| pushed_branch_progress_ts(project_root, &lease.branch))
            .map(|commit_ts| commit_ts.max(lease.started_at))
            .unwrap_or(lease.started_at);
        out.push(OrphanedLeaseFact {
            lease_id: lease.id,
            spec_id: scope.to_string(),
            owner: lease.owner,
            pid: Some(pid_val),
            process_alive,
            last_progress_at,
            owning_seat: default_advisor_seat(),
        });
    }
}

fn pushed_branch_progress_ts(repo: &Path, branch: &str) -> Option<DateTime<Utc>> {
    let branch = branch.trim();
    if !repo.exists() || branch.is_empty() {
        return None;
    }
    let remote_ref = format!("refs/remotes/origin/{branch}");
    if let Some(ts) = branch_latest_commit_ts(repo, &remote_ref) {
        return Some(ts);
    }
    // If the repository has an origin remote (or origin/main tracking ref) and
    // `refs/remotes/origin/<branch>` is absent, no progress has been pushed yet.
    if aida_core::git_ops::has_remote(repo, "origin")
        || branch_latest_commit_ts(repo, "refs/remotes/origin/main").is_some()
    {
        return None;
    }
    branch_latest_commit_ts(repo, branch)
}

fn branch_latest_commit_ts(repo: &Path, branch: &str) -> Option<DateTime<Utc>> {
    if !repo.exists() || branch.trim().is_empty() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "-1", "--format=%cI", branch])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_rfc3339_or_git_ts(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
#[path = "tests/story_1688_lane_liveness_tests.rs"]
mod tests;

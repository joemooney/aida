//! Deterministic lane-liveness substrate guard (`lane-liveness`).
//!
//! Runs every 10 minutes via `aida schedule tick` with zero LLM tokens and no
//! model-side polling. Every check reads a structured record; none parses PR
//! or comment prose:
//!
//! 1. **Dead lane / stale log**: a driver registry record, a crashed drain
//!    (`.aida/drain-state.json` with no `stopped_at`), or a headless child lane
//!    record (`.aida/lanes/headless-<lease>.json`) whose process is dead, or
//!    alive with a log that has not moved, while its lease-held work is open.
//! 2. **Review pickup**: an open non-draft PR whose head has had every check
//!    green for longer than the pickup budget, with no review lease and no
//!    recorded verdict at that head (`gh pr list --json statusCheckRollup`).
//! 3. **Gate waits**: a human-only merge hold (`.aida/merge-holds/PR-<n>`)
//!    older than the gate budget, and a request mailed to the advisor with no
//!    reply in its thread.
//! 4. **Orphaned work**: a spec lease with no live session (the canonical
//!    lease state) and no pushed progress.
//!
//! Each finding routes once: a seat finding becomes a `lane-liveness-<seat>`
//! due job for a registered agent role, and a human-only hold becomes an
//! operator push notification. Records older than `max_age_hours` are out of
//! scope, so an old backlog of dead leases does not trip the guard. The job
//! never relaunches a lane.
// trace:STORY-1688 | ai:claude

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::events::{self, Event, EventKind};
use crate::schedule_ledger;
use crate::DoctorFinding;

pub(crate) const CATEGORY: &str = "lane-liveness";

/// Default log-staleness threshold for a live lane with open work (minutes).
pub(crate) const DEFAULT_LOG_STALE_MINS: i64 = 30;
/// Default time a PR may sit green at its head with no review started (minutes).
pub(crate) const DEFAULT_REVIEW_PICKUP_MINS: i64 = 30;
/// Default time a human-only hold or an advisor request may wait (minutes).
pub(crate) const DEFAULT_GATE_WAIT_MINS: i64 = 15;
/// Default time a dead-process spec lease may sit with no pushed progress (minutes).
pub(crate) const DEFAULT_ORPHANED_WORK_MINS: i64 = 30;
/// Default age past which dead lanes, dead leases and advisor requests are no
/// longer reported (hours). Matches relay-stall's exit-report window.
pub(crate) const DEFAULT_MAX_AGE_HOURS: i64 = 24;
/// Bound on the `gh pr list` call; a slower forge skips the review-pickup check.
const GH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// Finding-id prefix of the review-pickup check, whose ids survive a tick
/// on which the forge read failed.
const UNPICKED_READY_PR_PREFIX: &str = "lane-liveness:unpicked-ready-pr:";
/// The CI check that fails at every head while a PR carries a merge hold.
const MERGE_HOLD_GATE_CHECK: &str = "merge-hold-gate";
/// Bound on the operator notify command.
const NOTIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Notification rule name used when pushing a human-only gate wait to the operator.
pub(crate) const OPERATOR_GATE_NOTIFY_RULE: &str = "lane-liveness-operator-gate";

/// One routed seat job: the single source of the prompt each seat receives.
pub(crate) struct SeatRoute {
    pub seat: &'static str,
    pub prompt: &'static str,
}

/// Every seat a finding can route to, one entry per registered agent role.
/// `builtin_routed_seat_task` builds the `lane-liveness-<seat>` jobs from
/// this table; `.aida/config.toml` carries no copies.
pub(crate) const ROUTES: &[SeatRoute] = &[
    SeatRoute {
        seat: "advisor",
        prompt: "Lane liveness routed findings to the advisor seat. Read the trip evidence in `aida schedule due --seat advisor` (or run `aida doctor check lane-liveness`), then act on each finding: reassign or resume work stranded on a dead or stalled lane, salvage, resume or reap an orphaned spec lease, and answer each request mailed to you that has no reply. Then run `aida schedule done lane-liveness-advisor`.",
    },
    SeatRoute {
        seat: "reviewer",
        prompt: "Lane liveness found PRs that have been green at their current head past the review pickup budget with no review started. Read the trip evidence in `aida schedule due --seat reviewer` (or run `aida doctor check lane-liveness`) and start an independent review of each named PR at that head. Then run `aida schedule done lane-liveness-reviewer`.",
    },
    SeatRoute {
        seat: "integrator",
        prompt: "Lane liveness found a lane registered to the integrator seat that is dead or stalled with open work. Read the trip evidence in `aida schedule due --seat integrator` (or run `aida doctor check lane-liveness`), then restart or retire that lane and its held work. Then run `aida schedule done lane-liveness-integrator`.",
    },
    SeatRoute {
        seat: "implementer",
        prompt: "Lane liveness found a lane registered to the implementer seat that is dead or stalled with open work. Read the trip evidence in `aida schedule due --seat implementer` (or run `aida doctor check lane-liveness`), then resume or hand back that lane's held work. Then run `aida schedule done lane-liveness-implementer`.",
    },
];

/// Schedule job name for a routed seat.
pub(crate) fn route_job_name(seat: &str) -> String {
    format!("{CATEGORY}-{seat}")
}

/// The route behind a `lane-liveness-<seat>` job name, if it is one.
pub(crate) fn route_for_job(job: &str) -> Option<&'static SeatRoute> {
    ROUTES.iter().find(|r| route_job_name(r.seat) == job)
}

/// A registered route seat for `raw`, falling back to the advisor.
fn registered_seat(raw: Option<&str>) -> &'static str {
    raw.map(crate::canonical_role_name)
        .and_then(|seat| ROUTES.iter().find(|r| r.seat == seat))
        .map_or("advisor", |r| r.seat)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaneLivenessPolicy {
    pub log_stale_mins: i64,
    pub review_pickup_mins: i64,
    pub gate_wait_mins: i64,
    pub orphaned_work_mins: i64,
    pub max_age_hours: i64,
    /// When non-empty, only PRs carrying one of these labels are checked for
    /// review pickup.
    pub review_pickup_labels: Vec<String>,
}

impl Default for LaneLivenessPolicy {
    fn default() -> Self {
        Self {
            log_stale_mins: DEFAULT_LOG_STALE_MINS,
            review_pickup_mins: DEFAULT_REVIEW_PICKUP_MINS,
            gate_wait_mins: DEFAULT_GATE_WAIT_MINS,
            orphaned_work_mins: DEFAULT_ORPHANED_WORK_MINS,
            max_age_hours: DEFAULT_MAX_AGE_HOURS,
            review_pickup_labels: Vec::new(),
        }
    }
}

impl LaneLivenessPolicy {
    /// Read `[lane_liveness]` from the project config; absent or non-positive
    /// values keep their defaults.
    pub(crate) fn from_config(cfg: Option<&toml::Value>) -> Self {
        let mut policy = Self::default();
        let Some(table) = cfg
            .and_then(|v| v.get("lane_liveness"))
            .and_then(|v| v.as_table())
        else {
            return policy;
        };
        let positive = |key: &str| {
            table
                .get(key)
                .and_then(|v| v.as_integer())
                .filter(|n| *n > 0)
        };
        for (key, slot) in [
            ("log_stale_mins", &mut policy.log_stale_mins),
            ("review_pickup_mins", &mut policy.review_pickup_mins),
            ("gate_wait_mins", &mut policy.gate_wait_mins),
            ("orphaned_work_mins", &mut policy.orphaned_work_mins),
            ("max_age_hours", &mut policy.max_age_hours),
        ] {
            if let Some(n) = positive(key) {
                *slot = n;
            }
        }
        if let Some(labels) = table.get("review_pickup_labels").and_then(|v| v.as_array()) {
            policy.review_pickup_labels = labels
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect();
        }
        policy
    }

    fn max_age(&self) -> Duration {
        Duration::hours(self.max_age_hours)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaneKind {
    Factory,
    HeadlessExec,
    Driver,
}

impl LaneKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Factory => "factory",
            Self::HeadlessExec => "headless-exec",
            Self::Driver => "driver",
        }
    }
}

/// One lane with open work, gathered from a production lane record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaneFact {
    pub id: String,
    pub kind: LaneKind,
    pub pid: Option<u32>,
    pub process_alive: bool,
    pub log_path: Option<PathBuf>,
    /// `None` when the lane has no progress signal to judge staleness by.
    pub log_updated_at: Option<DateTime<Utc>>,
    pub open_work: Vec<String>,
    /// Leases this lane holds, so orphaned-work does not report them twice.
    pub lease_ids: Vec<String>,
    pub seat: &'static str,
    pub stopped_reason: Option<String>,
    /// The headless record behind this lane, garbage-collected once its work closes.
    pub record_path: Option<PathBuf>,
}

/// An open PR, as the forge reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenPrFact {
    pub number: u64,
    pub title: String,
    pub head_sha: String,
    /// When the last check at the head completed, if every check is green.
    pub checks_green_at: Option<DateTime<Utc>>,
    /// A live review lease or a recorded verdict exists at the head.
    pub review_started: bool,
}

/// A merge hold only a human can release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HumanHoldFact {
    pub pr: u64,
    pub reason_kind: &'static str,
    pub target_head_sha: Option<String>,
    pub waiting_since: DateTime<Utc>,
}

/// A request mailed to the advisor with no reply in its thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdvisorRequestFact {
    pub msg_id: String,
    pub from: String,
    pub subject: String,
    pub sent_at: DateTime<Utc>,
}

/// A spec lease with no live session behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrphanedLeaseFact {
    pub lease_id: String,
    pub spec_id: String,
    pub owner: String,
    pub pid: Option<u32>,
    pub last_progress_at: DateTime<Utc>,
}

/// Everything `evaluate_snapshot` judges, read from production records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LaneLivenessSnapshot {
    pub lanes: Vec<LaneFact>,
    pub open_prs: Vec<OpenPrFact>,
    /// Why the forge read failed; the review-pickup check is skipped this tick.
    pub forge_error: Option<String>,
    pub human_holds: Vec<HumanHoldFact>,
    pub advisor_requests: Vec<AdvisorRequestFact>,
    pub orphaned_leases: Vec<OrphanedLeaseFact>,
    /// Headless lane records whose lease or spec has closed, or that aged out.
    pub stale_lane_records: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Where one finding goes. Exactly one route per finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Seat(&'static str),
    OperatorNotify,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaneLivenessFinding {
    pub id: String,
    pub kind: FindingKind,
    pub route: Route,
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

fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

/// Pure evaluation over a [`LaneLivenessSnapshot`]. Zero I/O, deterministic.
// trace:STORY-1688 | ai:claude
pub(crate) fn evaluate_snapshot(
    snapshot: &LaneLivenessSnapshot,
    policy: &LaneLivenessPolicy,
    now: DateTime<Utc>,
) -> Vec<LaneLivenessFinding> {
    let mut findings = Vec::new();
    let mut lane_leases: HashSet<&str> = HashSet::new();

    // 1. Lane process / log liveness
    for lane in &snapshot.lanes {
        if lane.open_work.is_empty() {
            continue;
        }
        lane_leases.extend(lane.lease_ids.iter().map(String::as_str));
        let work = lane.open_work.join(", ");
        let pid = lane
            .pid
            .map_or_else(|| "no pid".to_string(), |p| format!("pid {p}"));
        let reason = lane
            .stopped_reason
            .as_deref()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        let log = lane
            .log_path
            .as_ref()
            .map(|p| format!(" (log {})", p.display()))
            .unwrap_or_default();
        if !lane.process_alive {
            findings.push(LaneLivenessFinding {
                id: format!("{CATEGORY}:dead-lane:{}", lane.id),
                kind: FindingKind::DeadLane,
                route: Route::Seat(lane.seat),
                summary: format!(
                    "{} lane `{}` is dead ({pid}){reason} while its work [{work}] is still open",
                    lane.kind.as_str(),
                    lane.id,
                ),
                action: format!(
                    "seat `{}`: read lane `{}`{log}, then resume or reassign [{work}]",
                    lane.seat, lane.id,
                ),
            });
            continue;
        }
        let Some(updated_at) = lane.log_updated_at else {
            continue;
        };
        let age = (now - updated_at).num_minutes();
        if age > policy.log_stale_mins {
            findings.push(LaneLivenessFinding {
                id: format!("{CATEGORY}:stale-log:{}", lane.id),
                kind: FindingKind::StaleLog,
                route: Route::Seat(lane.seat),
                summary: format!(
                    "{} lane `{}` ({pid}) is alive but has shown no progress for {age}m (threshold {}m) while its work [{work}] is open",
                    lane.kind.as_str(),
                    lane.id,
                    policy.log_stale_mins,
                ),
                action: format!(
                    "seat `{}`: inspect stalled lane `{}`{log}, then unblock or restart it",
                    lane.seat, lane.id,
                ),
            });
        }
    }

    // 2. Review pickup (skipped for the tick when the forge read failed)
    for pr in &snapshot.open_prs {
        let Some(green_at) = pr.checks_green_at else {
            continue;
        };
        if pr.review_started {
            continue;
        }
        let wait = (now - green_at).num_minutes();
        if wait > policy.review_pickup_mins {
            findings.push(LaneLivenessFinding {
                id: format!(
                    "{UNPICKED_READY_PR_PREFIX}PR-{}@{}",
                    pr.number,
                    short_sha(&pr.head_sha)
                ),
                kind: FindingKind::UnpickedReadyPr,
                route: Route::Seat("reviewer"),
                summary: format!(
                    "PR #{} ({}) has been green at head {} for {wait}m (threshold {}m) with no review lease or verdict at that head",
                    pr.number,
                    pr.title,
                    short_sha(&pr.head_sha),
                    policy.review_pickup_mins,
                ),
                action: format!(
                    "seat `reviewer`: review PR #{} at {} (`aida queue work PR-{} --role reviewer`)",
                    pr.number,
                    short_sha(&pr.head_sha),
                    pr.number,
                ),
            });
        }
    }

    // 3a. Human-only merge holds
    for hold in &snapshot.human_holds {
        let wait = (now - hold.waiting_since).num_minutes();
        if wait > policy.gate_wait_mins {
            let head = hold.target_head_sha.as_deref().map_or("unknown", short_sha);
            findings.push(LaneLivenessFinding {
                id: format!("{CATEGORY}:gate-wait-operator:PR-{}@{head}", hold.pr),
                kind: FindingKind::StaleGateWait,
                route: Route::OperatorNotify,
                summary: format!(
                    "PR #{} has waited {wait}m (threshold {}m) on a {} merge hold only a human can release",
                    hold.pr, policy.gate_wait_mins, hold.reason_kind,
                ),
                action: format!(
                    "operator: inspect PR #{} and run `aida merge-hold clear {}` or `aida pr ship {}` at a terminal",
                    hold.pr, hold.pr, hold.pr,
                ),
            });
        }
    }

    // 3b. Advisor requests with no reply
    for req in &snapshot.advisor_requests {
        if now - req.sent_at > policy.max_age() {
            continue;
        }
        let wait = (now - req.sent_at).num_minutes();
        if wait > policy.gate_wait_mins {
            findings.push(LaneLivenessFinding {
                id: format!("{CATEGORY}:gate-wait-advisor:{}", req.msg_id),
                kind: FindingKind::StaleGateWait,
                route: Route::Seat("advisor"),
                summary: format!(
                    "request `{}` from `{}` has waited {wait}m (threshold {}m) for an advisor reply",
                    req.subject, req.from, policy.gate_wait_mins,
                ),
                action: format!(
                    "seat `advisor`: read and answer it (`aida mailbox read {}`)",
                    req.msg_id
                ),
            });
        }
    }

    // 4. Orphaned work
    for lease in &snapshot.orphaned_leases {
        if lane_leases.contains(lease.lease_id.as_str())
            || now - lease.last_progress_at > policy.max_age()
        {
            continue;
        }
        let stale = (now - lease.last_progress_at).num_minutes();
        if stale > policy.orphaned_work_mins {
            findings.push(LaneLivenessFinding {
                id: format!("{CATEGORY}:orphaned-work:{}", lease.lease_id),
                kind: FindingKind::OrphanedWork,
                route: Route::Seat("advisor"),
                summary: format!(
                    "spec `{}` lease `{}` (owner `{}`, {}) has no live session and no pushed progress for {stale}m (threshold {}m)",
                    lease.spec_id,
                    lease.lease_id,
                    lease.owner,
                    lease
                        .pid
                        .map_or_else(|| "no pid".to_string(), |p| format!("last pid {p}")),
                    policy.orphaned_work_mins,
                ),
                action: format!(
                    "seat `advisor`: salvage or resume `{}` (`aida queue work {}`), or release the dead lease",
                    lease.spec_id, lease.spec_id,
                ),
            });
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    findings
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

/// Finding ids already routed, so a finding wakes its seat once rather than
/// every tick. Ids that stop appearing are dropped, so a recurrence re-routes.
fn routed_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".aida")
        .join("lane-liveness")
        .join("routed.json")
}

fn load_routed(project_root: &Path) -> BTreeMap<String, DateTime<Utc>> {
    std::fs::read_to_string(routed_path(project_root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Route new findings: one `lane-liveness-<seat>` due job per seat with a new
/// finding, and one operator notification for new human-only gate waits.
/// Also deletes headless lane records whose work has closed.
// trace:STORY-1688 | ai:claude
pub(crate) fn route_findings(
    project_root: &Path,
    snapshot: &LaneLivenessSnapshot,
    findings: &[LaneLivenessFinding],
    now: DateTime<Utc>,
) {
    for path in &snapshot.stale_lane_records {
        let _ = std::fs::remove_file(path);
    }

    let previous = load_routed(project_root);
    let is_new = |f: &LaneLivenessFinding| !previous.contains_key(&f.id);
    // Only a route that succeeded is recorded, so a failed one retries next
    // tick. Review-pickup ids the forge could not re-check this tick are
    // carried forward, not forgotten and re-woken on the next good read.
    let mut routed: BTreeMap<String, DateTime<Utc>> = previous
        .iter()
        .filter(|(id, _)| {
            findings.iter().any(|f| &f.id == *id)
                || (snapshot.forge_error.is_some() && id.starts_with(UNPICKED_READY_PR_PREFIX))
        })
        .map(|(id, at)| (id.clone(), *at))
        .collect();

    let store_root = crate::detect_distributed_store_from(project_root)
        .unwrap_or_else(|| project_root.join(".aida-store"));
    let mut by_seat: BTreeMap<&'static str, Vec<&LaneLivenessFinding>> = BTreeMap::new();
    for f in findings {
        if let Route::Seat(seat) = f.route {
            by_seat.entry(seat).or_default().push(f);
        }
    }
    for (seat, seat_findings) in &by_seat {
        if !seat_findings.iter().any(|f| is_new(f)) {
            continue;
        }
        let job_name = route_job_name(seat);
        let evidence = seat_findings
            .iter()
            .map(|f| format!("[{}] {}", f.id, f.summary))
            .collect::<Vec<_>>()
            .join("; ");
        let already_due =
            schedule_ledger::load(&store_root, &job_name).is_some_and(|l| l.due_since.is_some());
        let trip_id = format!(
            "{job_name}@{}",
            now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        );
        if let Err(err) = schedule_ledger::write_cas(&store_root, &job_name, |ledger| {
            if ledger.due_since.is_none() {
                ledger.due_since = Some(now);
            }
            ledger.due_reason = Some(format!("on CronJobFired:{job_name}"));
            ledger.note = Some(evidence.clone());
            ledger.due_failure = Some(schedule_ledger::RoutedFailure {
                trip_id: trip_id.clone(),
                performance: Vec::new(),
                job: Some(CATEGORY.to_string()),
                error: Some(evidence.clone()),
            });
        }) {
            eprintln!("warning: could not write schedule ledger for '{job_name}': {err}");
            continue;
        }
        for f in seat_findings {
            routed.entry(f.id.clone()).or_insert(now);
        }
        if !already_due {
            events::emit(
                project_root,
                &Event::new(
                    None,
                    "",
                    EventKind::CronJobFired {
                        job: job_name,
                        seat: seat.to_string(),
                    },
                ),
            );
        }
    }

    let human: Vec<&LaneLivenessFinding> = findings
        .iter()
        .filter(|f| f.route == Route::OperatorNotify && is_new(f))
        .collect();
    if !human.is_empty() {
        let title = format!("AIDA: {} merge hold(s) waiting on a human", human.len());
        let message = human
            .iter()
            .map(|f| format!("{}\n  -> {}", f.summary, f.action))
            .collect::<Vec<_>>()
            .join("\n");
        match crate::notify::send_direct_bounded(
            project_root,
            OPERATOR_GATE_NOTIFY_RULE,
            &title,
            &message,
            Some(NOTIFY_TIMEOUT),
        ) {
            // Unconfigured notify reaches no one and a rate-limited notice
            // was not delivered: leave both unrouted so they retry.
            Ok(outcome) if outcome.configured && outcome.suppressed == 0 => {
                for f in &human {
                    routed.entry(f.id.clone()).or_insert(now);
                }
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("warning: lane-liveness operator notification failed: {err:#}");
            }
        }
    }

    if routed != previous {
        let path = routed_path(project_root);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(body) = serde_json::to_string_pretty(&routed) {
            let _ = aida_core::write_atomic(&path, body.as_bytes());
        }
    }
}

/// Read-only scan of `<project_root>` (the `aida doctor check` path).
pub(crate) fn scan(project_root: &Path) -> Vec<DoctorFinding> {
    scan_and_route(project_root, false)
}

/// Scan `<project_root>` and, when `route` is true (the scheduled tick),
/// route new findings and delete closed headless lane records.
// trace:STORY-1688 | ai:claude
pub(crate) fn scan_and_route(project_root: &Path, route: bool) -> Vec<DoctorFinding> {
    scan_and_route_at(project_root, Utc::now(), route, &fetch_open_prs_from_gh)
}

/// The source of `gh pr list --json` output: the live forge in production,
/// captured forge output in tests.
pub(crate) type PrSource<'a> = &'a dyn Fn(&Path) -> Result<String, String>;

pub(crate) fn scan_and_route_at(
    project_root: &Path,
    now: DateTime<Utc>,
    route: bool,
    prs: PrSource<'_>,
) -> Vec<DoctorFinding> {
    let cfg = crate::read_project_config_value(project_root);
    let policy = LaneLivenessPolicy::from_config(cfg.as_ref());
    let snapshot = load_snapshot(project_root, now, &policy, prs);
    if let Some(err) = &snapshot.forge_error {
        eprintln!("warning: lane-liveness skipped the review-pickup check this tick: {err}");
    }
    let findings = evaluate_snapshot(&snapshot, &policy, now);
    if route {
        route_findings(project_root, &snapshot, &findings, now);
    }
    findings
        .into_iter()
        .map(LaneLivenessFinding::into_doctor_finding)
        .collect()
}

// ---------------------------------------------------------------------------
// Headless child lane records (written by `run_vendor_headless_child`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HeadlessLaneRecord {
    lease_id: String,
    spec: String,
    vendor: String,
    pid: u32,
    #[serde(default)]
    pid_start_time: Option<String>,
    log_path: PathBuf,
    started_at: DateTime<Utc>,
    #[serde(default)]
    exited_at: Option<DateTime<Utc>>,
    #[serde(default)]
    exit_status: Option<String>,
}

fn lanes_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("lanes")
}

fn headless_record_path(project_root: &Path, lease_id: &str) -> PathBuf {
    lanes_dir(project_root).join(format!("headless-{lease_id}.json"))
}

/// The project root owning `<root>/.aida/headless-logs/<file>`.
fn project_root_for_log(log_path: &Path) -> Option<PathBuf> {
    log_path
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == ".aida"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// Register the headless child holding `lease_id`. Without a lease there is
/// no held work to strand, so nothing is recorded.
// trace:STORY-1688 | ai:claude
pub(crate) fn register_headless_lane_for_child(
    log_path: &Path,
    lease_id: Option<&str>,
    vendor: &str,
    child_pid: u32,
) {
    let Some(lease_id) = lease_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let Some(project_root) = project_root_for_log(log_path) else {
        return;
    };
    let Some(lease) = crate::leases::list_leases(&project_root)
        .into_iter()
        .find(|l| l.id == lease_id)
    else {
        return;
    };
    let record = HeadlessLaneRecord {
        lease_id: lease_id.to_string(),
        spec: lease.scope,
        vendor: vendor.to_string(),
        pid: child_pid,
        pid_start_time: crate::process_probe::process_start_identity(child_pid),
        log_path: log_path.to_path_buf(),
        started_at: Utc::now(),
        exited_at: None,
        exit_status: None,
    };
    let _ = std::fs::create_dir_all(lanes_dir(&project_root));
    if let Ok(body) = serde_json::to_string_pretty(&record) {
        let _ = aida_core::write_atomic(
            &headless_record_path(&project_root, lease_id),
            body.as_bytes(),
        );
    }
}

/// Close out a headless child's lane record when the child exits. A child
/// that released its lease finished its work, so the record is removed. A
/// child that exits on ANY status while its lease is still held stranded that
/// work, so the record stays, stamped with the exit, for the next tick.
// trace:STORY-1688 | ai:claude
pub(crate) fn finish_headless_lane_for_child(
    log_path: &Path,
    lease_id: Option<&str>,
    status: Option<std::process::ExitStatus>,
) {
    let Some(lease_id) = lease_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let Some(project_root) = project_root_for_log(log_path) else {
        return;
    };
    let path = headless_record_path(&project_root, lease_id);
    let lease_held = crate::leases::list_leases(&project_root)
        .iter()
        .any(|l| l.id == lease_id);
    if !lease_held {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let Some(mut record) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<HeadlessLaneRecord>(&t).ok())
    else {
        return;
    };
    record.exited_at = Some(Utc::now());
    record.exit_status = Some(match status {
        Some(s) => format!("exited with {s}"),
        None => "wait failed".to_string(),
    });
    if let Ok(body) = serde_json::to_string_pretty(&record) {
        let _ = aida_core::write_atomic(&path, body.as_bytes());
    }
}

// ---------------------------------------------------------------------------
// Snapshot loading
// ---------------------------------------------------------------------------

fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
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

fn is_terminal(status: &aida_core::RequirementStatus) -> bool {
    use aida_core::RequirementStatus as S;
    matches!(status, S::Done | S::Completed | S::Rejected | S::Superseded)
}

/// Load a [`LaneLivenessSnapshot`] from the production records under
/// `<project_root>/.aida/`, the driver registry, the mailbox, and the forge.
// trace:STORY-1688 | ai:claude
pub(crate) fn load_snapshot(
    project_root: &Path,
    now: DateTime<Utc>,
    policy: &LaneLivenessPolicy,
    prs: PrSource<'_>,
) -> LaneLivenessSnapshot {
    let mut snapshot = LaneLivenessSnapshot::default();
    let leases = crate::leases::list_leases(project_root);

    load_headless_lanes(project_root, &leases, now, policy, &mut snapshot);
    load_drain_lane(project_root, &leases, now, policy, &mut snapshot);
    // One live-session probe per tick, shared by every lease-liveness read.
    let mut live_sessions = None;
    load_driver_lanes(
        project_root,
        &leases,
        now,
        policy,
        &mut live_sessions,
        &mut snapshot,
    );
    load_open_prs(project_root, &leases, now, policy, prs, &mut snapshot);
    load_human_holds(project_root, &mut snapshot);
    load_advisor_requests(project_root, &mut snapshot);
    load_orphaned_leases(
        project_root,
        &leases,
        now,
        policy,
        &mut live_sessions,
        &mut snapshot,
    );

    // Work on a finished, rejected or superseded spec is not stranded.
    let scopes: Vec<String> = snapshot
        .lanes
        .iter()
        .flat_map(|l| l.open_work.iter().cloned())
        .chain(snapshot.orphaned_leases.iter().map(|l| l.spec_id.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !scopes.is_empty() {
        let closed = crate::session_reap::scopes_with_status(project_root, &scopes, is_terminal);
        let is_closed = |s: &String| closed.contains(&s.to_ascii_uppercase());
        for lane in &mut snapshot.lanes {
            lane.open_work.retain(|s| !is_closed(s));
            if lane.open_work.is_empty() {
                if let Some(path) = lane.record_path.take() {
                    snapshot.stale_lane_records.push(path);
                }
            }
        }
        snapshot.lanes.retain(|l| !l.open_work.is_empty());
        snapshot.orphaned_leases.retain(|l| !is_closed(&l.spec_id));
    }
    snapshot
}

fn load_headless_lanes(
    project_root: &Path,
    leases: &[crate::leases::SessionLease],
    now: DateTime<Utc>,
    policy: &LaneLivenessPolicy,
    snapshot: &mut LaneLivenessSnapshot,
) {
    let Ok(entries) = std::fs::read_dir(lanes_dir(project_root)) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("headless-") && n.ends_with(".json"))
        })
        .collect();
    paths.sort();
    for path in paths {
        let Some(rec) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<HeadlessLaneRecord>(&t).ok())
        else {
            continue;
        };
        let log_updated_at = file_mtime_utc(&rec.log_path);
        let last_seen = rec.exited_at.or(log_updated_at).unwrap_or(rec.started_at);
        let lease_held = leases.iter().any(|l| l.id == rec.lease_id);
        if !lease_held || now - last_seen > policy.max_age() {
            snapshot.stale_lane_records.push(path);
            continue;
        }
        let process_alive = rec.exited_at.is_none()
            && aida_core::liveness::process_identity_is_alive(
                rec.pid,
                rec.pid_start_time.as_deref(),
            );
        snapshot.lanes.push(LaneFact {
            id: format!("{}-headless-{}", rec.vendor, rec.lease_id),
            kind: LaneKind::HeadlessExec,
            pid: Some(rec.pid),
            process_alive,
            log_path: Some(rec.log_path),
            log_updated_at,
            open_work: vec![rec.spec],
            lease_ids: vec![rec.lease_id],
            seat: "advisor",
            stopped_reason: rec.exit_status,
            record_path: Some(path),
        });
    }
}

/// A drain halted on purpose (`stopped_at` set) is not a lane; a drain whose
/// orchestrator died without stopping is.
fn load_drain_lane(
    project_root: &Path,
    leases: &[crate::leases::SessionLease],
    now: DateTime<Utc>,
    policy: &LaneLivenessPolicy,
    snapshot: &mut LaneLivenessSnapshot,
) {
    let Some(state) = crate::drain_state::DrainState::read(project_root) else {
        return;
    };
    if state.stopped_at.is_some() {
        return;
    }
    let open_work: Vec<String> = state
        .members
        .iter()
        .filter(|m| {
            m.state != crate::drain_state::STATE_COMPLETED
                && m.state != crate::drain_state::STATE_FAILED
        })
        .map(|m| m.spec.clone())
        .collect();
    if open_work.is_empty() {
        return;
    }
    let state_path = crate::drain_state::drain_state_path(project_root);
    let (log_path, log_ts) = newest_headless_log(project_root, state.current_session_id.as_deref());
    let progress = [
        log_ts,
        state.phase_started_at.as_deref().and_then(parse_ts),
        parse_ts(&state.started_at),
        file_mtime_utc(&state_path),
    ]
    .into_iter()
    .flatten()
    .max();
    if progress.is_some_and(|ts| now - ts > policy.max_age()) {
        return;
    }
    let id = match state.batch.as_deref().filter(|b| !b.is_empty()) {
        Some(batch) => format!("drain-batch-{batch}"),
        None => format!("drain-{}", state.mode),
    };
    let lease_ids = leases
        .iter()
        .filter(|l| open_work.iter().any(|s| s.eq_ignore_ascii_case(&l.scope)))
        .map(|l| l.id.clone())
        .collect();
    snapshot.lanes.push(LaneFact {
        id,
        kind: LaneKind::Factory,
        pid: Some(state.orchestrator_pid),
        process_alive: state.orchestrator_alive(),
        log_path: log_path.or(Some(state_path)),
        log_updated_at: progress,
        open_work,
        lease_ids,
        seat: "advisor",
        stopped_reason: None,
        record_path: None,
    });
}

/// Driver registry lanes: open work is the leases a driver vouches for.
fn load_driver_lanes(
    project_root: &Path,
    leases: &[crate::leases::SessionLease],
    now: DateTime<Utc>,
    policy: &LaneLivenessPolicy,
    live_sessions: &mut Option<Vec<crate::process_probe::LiveSession>>,
    snapshot: &mut LaneLivenessSnapshot,
) {
    let records = aida_core::driver_registry::read_all(&crate::driver_view::driver_registry_root(
        project_root,
    ));
    for rec in records {
        let label = rec.label();
        let process_alive = rec.is_live(now);
        // A review is not the driver's work. Once the driver is dead, a lease
        // whose own session is still live only inherited the label: that
        // work is not stranded, and orphaned-work judges it on its own.
        let held: Vec<&crate::leases::SessionLease> = leases
            .iter()
            .filter(|l| l.driver.as_deref().map(str::trim) == Some(label.as_str()))
            .filter(|l| !l.scope.trim().is_empty() && crate::parse_review_scope(&l.scope).is_none())
            .filter(|l| {
                process_alive || !crate::driver_view::lease_is_live_lazy(l, live_sessions, now)
            })
            .collect();
        if held.is_empty() {
            continue;
        }
        let handoff_path = rec.handoff.as_deref().map(PathBuf::from);
        // Only a TTL'd heartbeat or a handoff file is a progress signal; a
        // pid-only record has none, so it is never judged stale.
        let progress = [
            handoff_path.as_deref().and_then(file_mtime_utc),
            rec.ttl_secs().map(|_| rec.heartbeat_at),
        ]
        .into_iter()
        .flatten()
        .max();
        let last_seen = progress.unwrap_or(rec.heartbeat_at);
        if !process_alive && now - last_seen > policy.max_age() {
            continue;
        }
        snapshot.lanes.push(LaneFact {
            id: label,
            kind: LaneKind::Driver,
            pid: Some(rec.pid),
            process_alive,
            log_path: handoff_path,
            log_updated_at: progress,
            open_work: held.iter().map(|l| l.scope.clone()).collect(),
            lease_ids: held.iter().map(|l| l.id.clone()).collect(),
            seat: registered_seat(rec.seat.as_deref()),
            stopped_reason: None,
            record_path: None,
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
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.ends_with(".jsonl") && suffix.as_deref().is_none_or(|want| n.ends_with(want))
            })
        })
        .filter_map(|p| file_mtime_utc(&p).map(|ts| (p, ts)))
        .max_by_key(|(_, ts)| *ts)
        .map_or((None, None), |(p, ts)| (Some(p), Some(ts)))
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    head_ref_oid: String,
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default)]
    status_check_rollup: Vec<serde_json::Value>,
}

/// Fields requested from `gh pr list`; tests feed output of this shape.
pub(crate) const GH_PR_FIELDS: &str = "number,title,isDraft,headRefOid,labels,statusCheckRollup";

/// The production [`PrSource`]: `gh pr list` with a hard timeout. Any failure
/// is an `Err`, which skips the review-pickup check for the tick.
fn fetch_open_prs_from_gh(project_root: &Path) -> Result<String, String> {
    let gh = crate::resolve_forge_cli(crate::forge::ForgeKind::GitHub)
        .ok_or_else(|| "no gh binary found".to_string())?;
    let mut cmd = std::process::Command::new(gh);
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
            GH_PR_FIELDS,
        ]);
    let out = crate::command_output_with_timeout(cmd, GH_TIMEOUT).ok_or_else(|| {
        format!(
            "`gh pr list` timed out after {}s or failed to start",
            GH_TIMEOUT.as_secs()
        )
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "`gh pr list` failed: {}",
            stderr.lines().next().unwrap_or("").trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// When every check in a head's `statusCheckRollup` passed, the time the last
/// one finished. `None` while any check is pending or failing, or when the
/// head has no checks at all (no readiness signal). With `skip_hold_gate`,
/// the `merge-hold-gate` check, red at every head while a hold's label is on
/// the PR, is left out.
fn rollup_green_at(rollup: &[serde_json::Value], skip_hold_gate: bool) -> Option<DateTime<Utc>> {
    let mut latest: Option<DateTime<Utc>> = None;
    for check in rollup {
        let field = |k: &str| check.get(k).and_then(|v| v.as_str()).unwrap_or("");
        if skip_hold_gate && field("name") == MERGE_HOLD_GATE_CHECK {
            continue;
        }
        let (green, finished) = if field("__typename") == "StatusContext" {
            (
                field("state") == "SUCCESS",
                parse_ts(field("completedAt"))
                    .or_else(|| parse_ts(field("startedAt")))
                    .or_else(|| parse_ts(field("createdAt"))),
            )
        } else {
            (
                field("status") == "COMPLETED"
                    && matches!(field("conclusion"), "SUCCESS" | "NEUTRAL" | "SKIPPED"),
                parse_ts(field("completedAt")),
            )
        };
        if !green {
            return None;
        }
        let finished = finished?;
        latest = Some(latest.map_or(finished, |l| l.max(finished)));
    }
    latest
}

/// Every `reviewed_sha` on record under `.aida/review-verdicts/`.
fn recorded_verdict_shas(project_root: &Path) -> Vec<String> {
    let dir = project_root.join(".aida").join("review-verdicts");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|body| crate::review_verdict::parse_recorded_verdict(&body))
        .filter_map(|v| v.reviewed_sha)
        .collect()
}

fn load_open_prs(
    project_root: &Path,
    leases: &[crate::leases::SessionLease],
    now: DateTime<Utc>,
    policy: &LaneLivenessPolicy,
    prs: PrSource<'_>,
    snapshot: &mut LaneLivenessSnapshot,
) {
    let raw = match prs(project_root) {
        Ok(raw) => raw,
        Err(err) => {
            snapshot.forge_error = Some(err);
            return;
        }
    };
    let list: Vec<GhPr> = match serde_json::from_str(&raw) {
        Ok(list) => list,
        Err(err) => {
            snapshot.forge_error = Some(format!("unreadable `gh pr list` output: {err}"));
            return;
        }
    };
    let mut verdict_shas: Option<Vec<String>> = None;
    for pr in list {
        if pr.is_draft || pr.head_ref_oid.is_empty() {
            continue;
        }
        if !policy.review_pickup_labels.is_empty()
            && !pr.labels.iter().any(|l| {
                policy
                    .review_pickup_labels
                    .iter()
                    .any(|want| want.eq_ignore_ascii_case(&l.name))
            })
        {
            continue;
        }
        let head = pr.head_ref_oid.as_str();
        // A rework push past a reviewer refusal's head awaits re-review; the
        // hold's own gate check stays red until that review, so it is not a
        // readiness signal there.
        let reworked_past_refusal = crate::merge_hold::read_hold_record(project_root, pr.number)
            .is_some_and(|h| {
                h.is_refusal()
                    && h.target_head_sha
                        .as_deref()
                        .is_some_and(|t| !crate::review_verdict::same_reviewed_sha(t, head))
            });
        let checks_green_at = rollup_green_at(&pr.status_check_rollup, reworked_past_refusal);
        let scope = format!("PR-{}", pr.number);
        // A reviewer session (`aida queue work PR-<n> --role reviewer`) has a
        // worktree and no agent pid, so the canonical state reads it Dormant
        // until a live session is probed; either way the review has started.
        // Only a Stale lease (worktree gone or past 24h) has not.
        let review_lease = leases.iter().any(|l| {
            l.scope.eq_ignore_ascii_case(&scope)
                && !matches!(
                    crate::lease_state_for(l, &[], now),
                    crate::LeaseState::Stale
                )
                && l.pr_head_sha
                    .as_deref()
                    .is_none_or(|sha| crate::review_verdict::same_reviewed_sha(sha, head))
        });
        let review_started = review_lease
            || (checks_green_at.is_some()
                && verdict_shas
                    .get_or_insert_with(|| recorded_verdict_shas(project_root))
                    .iter()
                    .any(|sha| crate::review_verdict::same_reviewed_sha(sha, head)));
        snapshot.open_prs.push(OpenPrFact {
            number: pr.number,
            title: pr.title,
            head_sha: pr.head_ref_oid,
            checks_green_at,
            review_started,
        });
    }
}

/// Holds that only a human releases: everything but a reviewer refusal,
/// which a fresh verdict at the new head releases instead.
fn load_human_holds(project_root: &Path, snapshot: &mut LaneLivenessSnapshot) {
    let holds: Vec<crate::merge_hold::MergeHoldRecord> =
        crate::merge_hold::list_holds(project_root)
            .into_iter()
            .filter_map(|(pr, _)| crate::merge_hold::read_hold_record(project_root, pr))
            .filter(|h| !h.is_refusal())
            .collect();
    if holds.is_empty() {
        return;
    }
    // The current episode of each hold starts at its first placement after
    // its most recent release.
    let mut placed_since: BTreeMap<u64, DateTime<Utc>> = BTreeMap::new();
    for event in events::read_all(project_root) {
        if let EventKind::MergeHoldChanged { pr, placed, .. } = event.kind {
            let pr = u64::from(pr);
            if placed {
                placed_since.entry(pr).or_insert(event.ts);
            } else {
                placed_since.remove(&pr);
            }
        }
    }
    for hold in holds {
        let Some(waiting_since) = placed_since
            .get(&hold.pr)
            .copied()
            .or_else(|| file_mtime_utc(&crate::merge_hold::hold_path(project_root, hold.pr)))
        else {
            continue;
        };
        snapshot.human_holds.push(HumanHoldFact {
            pr: hold.pr,
            reason_kind: hold.reason_kind.as_str(),
            target_head_sha: hold.target_head_sha,
            waiting_since,
        });
    }
}

/// Requests (or handoffs) mailed to the advisor that nobody but the sender
/// has written to since, in the same thread.
fn load_advisor_requests(project_root: &Path, snapshot: &mut LaneLivenessSnapshot) {
    use aida_core::mailbox::{Intent, Recipient};
    let mut messages = crate::mailbox_store::read_local_messages(project_root).unwrap_or_default();
    if let Some(store) = crate::detect_distributed_store_from(project_root) {
        messages.extend(crate::mailbox_store::read_canonical_messages(&store).unwrap_or_default());
    }
    let mut seen = HashSet::new();
    messages.retain(|m| seen.insert(m.id.clone()));
    let is_advisor = |name: &str| crate::canonical_role_name(name) == "advisor";
    for msg in &messages {
        let Recipient::Agent(to) = &msg.to else {
            continue;
        };
        if !is_advisor(to)
            || is_advisor(&msg.from)
            || !matches!(msg.intent, Intent::Request | Intent::Handoff)
            || msg.retracted
            || msg.deleted
            || msg.archived
        {
            continue;
        }
        let replied = messages.iter().any(|r| {
            r.id != msg.id
                && r.from != msg.from
                && r.timestamp >= msg.timestamp
                && (r.thread_id == msg.thread_id || r.in_reply_to.as_deref() == Some(&msg.id))
        });
        if replied {
            continue;
        }
        let Some(sent_at) = DateTime::<Utc>::from_timestamp_millis(msg.timestamp) else {
            continue;
        };
        let subject = msg
            .subject
            .clone()
            .or_else(|| {
                msg.body
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        snapshot.advisor_requests.push(AdvisorRequestFact {
            msg_id: msg.id.clone(),
            from: msg.from.clone(),
            subject: subject.chars().take(80).collect(),
            sent_at,
        });
    }
}

fn load_orphaned_leases(
    project_root: &Path,
    leases: &[crate::leases::SessionLease],
    now: DateTime<Utc>,
    policy: &LaneLivenessPolicy,
    live_sessions: &mut Option<Vec<crate::process_probe::LiveSession>>,
    snapshot: &mut LaneLivenessSnapshot,
) {
    let live_drivers = aida_core::driver_registry::live_drivers(
        &crate::driver_view::driver_registry_root(project_root),
        now,
    );
    for lease in leases {
        let scope = lease.scope.trim();
        if scope.is_empty()
            || crate::parse_review_scope(scope).is_some()
            || scope.eq_ignore_ascii_case("harness-worktree")
            || lease.manual_enter_at.is_some()
            || lease.interrupted_at.is_some()
        {
            continue;
        }
        if lease
            .driver
            .as_deref()
            .is_some_and(|d| live_drivers.iter().any(|r| r.label() == d.trim()))
        {
            continue;
        }
        // The canonical lease state: a worktree session is judged by the
        // live-session matrix, never by the short-lived shell that created it.
        if crate::driver_view::lease_is_live_lazy(lease, live_sessions, now) {
            continue;
        }
        // Past the age window with its worktree gone, the lease is out of
        // scope; skip the git probes rather than run them on every old file.
        if !lease.worktree_path.exists() && now - lease.started_at > policy.max_age() {
            continue;
        }
        let last_progress_at = pushed_branch_progress_ts(&lease.worktree_path, &lease.branch)
            .or_else(|| pushed_branch_progress_ts(project_root, &lease.branch))
            .map_or(lease.started_at, |ts| ts.max(lease.started_at));
        snapshot.orphaned_leases.push(OrphanedLeaseFact {
            lease_id: lease.id.clone(),
            spec_id: scope.to_string(),
            owner: lease.owner.clone(),
            pid: lease.active_pid.or(lease.creator_pid),
            last_progress_at,
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
    // With an origin remote, an absent `origin/<branch>` means nothing was pushed.
    if aida_core::git_ops::has_remote(repo, "origin")
        || branch_latest_commit_ts(repo, "refs/remotes/origin/main").is_some()
    {
        return None;
    }
    branch_latest_commit_ts(repo, branch)
}

/// `repo` may be a lease's tree; a seat clone is refused (BUG-2169).
// trace:TASK-2053 | ai:claude
pub(crate) fn branch_latest_commit_ts(repo: &Path, branch: &str) -> Option<DateTime<Utc>> {
    let out = aida_core::git_ops::host_git_command(repo)
        .args(["log", "-1", "--format=%cI", branch])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_ts(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
#[path = "tests/story_1688_lane_liveness_tests.rs"]
mod tests;

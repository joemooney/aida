//! STORY-1700 (ADR-77): the drive-mandate tick.
//!
//! While a human-minted DRIVE mandate is active, the `mandate-drive`
//! substrate job runs `aida fleet drive --tick` every 15 minutes. One tick:
//!
//! 1. resolves its authority from disk only (the mandate, the human gates,
//!    the principal) — never from an inherited seat grant, role or purpose;
//! 2. takes `.aida/mandate-drive.lock` (in addition to the scheduler's tick
//!    lock), so a manual tick and a scheduled one cannot overlap;
//! 3. does the substrate actions itself (dead-lease expiry, operator notices,
//!    due-item emission and the ledger) and ROUTES every seat action (ship,
//!    sketch signoff, dispatch, answering a blocked relay) to the advisor seat
//!    as a deduplicated `MandateDriveDue` item, or to the operator when the
//!    action is human-gated or needs a human;
//! 4. re-reads the mandate immediately before every side effect, so a stop or
//!    an expiry mid-tick refuses the rest.
//!
//! The tick never mints, holds or forwards a grant, never ships and never
//! clears a hold. Its one exec is STORY-1706's restart of a crashed relay,
//! from the human launch record alone ([`restart_verdict`]). Every decision
//! is a `MandateDriveAction` row in `.aida/events.jsonl`.
// trace:STORY-1700 | ai:claude
// trace:STORY-1706 | ai:claude

use crate::fleet_lanes::{Lane, LaneClass, SeatKind};
use crate::mandate::{Mandate, MandateRefusal, MandateState, Scope};
use crate::mandate_gates::{GateOp, HumanGates};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// trace:STORY-1728 | ai:claude
#[path = "mandate_drive_args.rs"]
pub(crate) mod mandate_drive_args;
pub(crate) use mandate_drive_args::*;

/// The scheduled job's name (and its run ledger at `schedule/<job>.yaml`).
pub(crate) const JOB: &str = "mandate-drive";
/// The scheduled job's command.
pub(crate) const JOB_COMMAND: &str = "fleet drive --tick";
const LOCK_FILE: &str = "mandate-drive.lock";
const STATE_FILE: &str = "mandate-drive-state.json";
pub(crate) const DEFAULT_MAX_ACTIONS: usize = 10;
const TICK_INTERVAL: Duration = Duration::minutes(15);
/// Backoff after a failed attempt, in ticks: 1, 2, then 4.
const BACKOFF_TICKS: [i32; 3] = [1, 2, 4];
const ADVISOR_LIVE_WITHIN: Duration = Duration::minutes(30);
/// A routed dispatch that no owner has taken up within this window is
/// re-routed: the fixer died before it claimed (TASK-1915 F8).
const DISPATCH_STALE: Duration = Duration::hours(2);
/// A work-queue entry older than this is abandoned, not an owner: a spec
/// left in a stale queue is dispatched again (TASK-1915 F8).
const QUEUE_ENTRY_FRESH: Duration = Duration::days(7);
/// The session variables a tick never acts on and scrubs from its children.
pub(crate) const SESSION_ENV: [&str; 3] = [
    crate::seat_authority::GRANT_ENV,
    "AIDA_SESSION_ROLE",
    "AIDA_SESSION_PURPOSE",
];
const NOTIFY_RULE: &str = "mandate-drive";

// ── Actions and plans ───────────────────────────────────────────────────────

/// The next action for a lane.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriveAction {
    /// Ship an approved, held PR (`aida pr ship`).
    Ship,
    /// Sign off the sketch a blocked relay waits on.
    SketchSignoff,
    /// Answer the question a blocked relay waits on.
    Answer,
    /// Re-dispatch a lane: a crash, a session cap, or no free slot.
    Redispatch,
    /// Fix a setup or usage-binary error before relaunching.
    FixSetup,
    /// Needs the operator (credits, an advisor-seat run, a gated item).
    OperatorAttention,
}

impl DriveAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ship => "ship",
            Self::SketchSignoff => "sketch-signoff",
            Self::Answer => "answer",
            Self::Redispatch => "dispatch",
            Self::FixSetup => "fix-setup",
            Self::OperatorAttention => "operator-attention",
        }
    }

    /// The human gate that can keep this action human.
    pub(crate) fn gate_op(self) -> Option<GateOp> {
        match self {
            Self::Ship => Some(GateOp::Ship),
            Self::SketchSignoff => Some(GateOp::SketchSignoff),
            Self::Redispatch => Some(GateOp::Dispatch),
            // Answering a blocked relay is an advisor ruling like a signoff:
            // the same human gate holds it. trace:STORY-1700 | ai:claude
            Self::Answer => Some(GateOp::SketchSignoff),
            Self::FixSetup | Self::OperatorAttention => None,
        }
    }

    /// The mandate scope the action needs.
    pub(crate) fn scope(self) -> Option<Scope> {
        match self {
            Self::Ship => Some(Scope::ShipHeld),
            Self::SketchSignoff => Some(Scope::SketchSignoff),
            Self::Redispatch => Some(Scope::Dispatch),
            Self::Answer => Some(Scope::SketchSignoff),
            Self::FixSetup | Self::OperatorAttention => None,
        }
    }
}

/// Who a planned action goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Advisor,
    Operator,
}

impl Route {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Advisor => "advisor",
            Self::Operator => "operator",
        }
    }
}

/// One lane's planned next action.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Planned {
    /// `relay:<run>` or `pr:<n>`.
    pub lane: String,
    pub subject: String,
    pub action: DriveAction,
    pub route: Route,
    /// What makes a new item distinct on the same lane: the relay's
    /// `updated`/exit, or the PR's head SHA.
    pub head: String,
    pub command: String,
    pub detail: String,
    /// The spec the action concerns, when known (operator-only check).
    pub spec: Option<String>,
    /// Gated whatever the config says (e.g. a PR that touches the gates).
    pub forced_gate: Option<String>,
    /// Further specs the action touches (every store spec a blocked relay
    /// names); each takes the same always-human floor as `spec`.
    // trace:STORY-1700 | ai:claude
    pub other_specs: Vec<String>,
}

impl Planned {
    pub(crate) fn key(&self) -> String {
        format!("{}|{}|{}", self.lane, self.action.as_str(), self.head)
    }

    fn backoff_key(&self) -> String {
        format!("{}|{}", self.lane, self.action.as_str())
    }
}

/// A lane's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaneDecision {
    /// Nothing to do (running, complete, acknowledged, not stalled).
    None,
    /// Deliberately left alone; ledgered once.
    Skip {
        lane: String,
        subject: String,
        head: String,
        why: String,
    },
    Act(Planned),
}

/// STORY-1700 (R6): the advisor's resolving action for a BLOCKED relay run:
/// the question (handoff + `BLOCKED_ON_HUMAN` marker line), the resolving
/// command, the resume instruction and the acknowledgement step. The tick
/// itself never restarts or re-execs anything.
// trace:STORY-1700 | ai:claude
pub(crate) fn resolving_action(
    lane: &Lane,
    spec_signoff: &dyn Fn(&str) -> Option<bool>,
) -> (DriveAction, String, String, Vec<String>) {
    let handoff = lane.evidence.field("handoff").map(|h| {
        let p = PathBuf::from(h);
        if p.is_absolute() {
            p
        } else {
            lane.run_dir.join(p)
        }
    });
    let marker_line = handoff
        .as_ref()
        .and_then(|h| std::fs::read_to_string(h).ok())
        .and_then(|text| {
            text.lines()
                .find(|l| l.contains("BLOCKED_ON_HUMAN"))
                .map(|l| l.trim().to_string())
        });
    // Every token the store knows is one of the lane's specs (`PR-12` in the
    // question is just prose), and each takes the operator-only floor. The
    // lane is a signoff request if ANY of them awaits one, i.e. the store
    // records no ruling newer than its last activity (fail closed).
    let named: Vec<(String, bool)> = marker_line
        .as_deref()
        .map(|line| {
            let mut seen = std::collections::BTreeSet::new();
            spec_ids_in(line)
                .into_iter()
                .filter(|id| seen.insert(id.clone()))
                .filter_map(|id| spec_signoff(&id).map(|awaiting| (id, awaiting)))
                .collect()
        })
        .unwrap_or_default();
    let awaiting = named.iter().find(|(_, a)| *a).map(|(id, _)| id.clone());
    // The awaiting spec (if any) leads, so it is the item's primary spec.
    let mut specs: Vec<String> = awaiting.iter().cloned().collect();
    specs.extend(
        named
            .iter()
            .map(|(id, _)| id.clone())
            .filter(|id| Some(id) != awaiting.as_ref()),
    );
    let handoff_text = handoff
        .as_ref()
        .map(|h| h.display().to_string())
        .unwrap_or_else(|| "(no handoff recorded)".into());
    let (action, resolve) = match (&awaiting, specs.first()) {
        (Some(spec), _) => (
            DriveAction::SketchSignoff,
            format!(
                "aida show {spec} --comments; then record the ruling: aida comment add {spec} \"ADVISOR SIGNOFF: ...\""
            ),
        ),
        (None, Some(_)) => (
            DriveAction::Answer,
            format!(
                "answer the question in {handoff_text} (about {})",
                specs.join(", ")
            ),
        ),
        (None, None) => (
            DriveAction::Answer,
            format!("answer the question in {handoff_text}"),
        ),
    };
    let status_file = lane.run_dir.join("relay.status");
    let profiles = lane
        .run_dir
        .parent()
        .map(|relay| relay.join("profiles"))
        .filter(|p| p.is_dir());
    let mut resume = format!(
        "relaunch the run with bin/relay.sh against the same handoff ({handoff_text}; run {}, see {})",
        lane.run_id,
        status_file.display()
    );
    if let Some(p) = profiles {
        resume.push_str(&format!("; profiles: {}", p.display()));
    }
    let ack = crate::fleet_lanes::acknowledgement_marker(&lane.run_dir, &lane.evidence.status)
        .map(|m| format!("touch {}", m.display()))
        .unwrap_or_else(|| "retire the run directory".into());
    let command = format!("1. {resolve}\n2. {resume}\n3. acknowledge: {ack}");
    let detail = format!(
        "relay run {} is blocked; question: {} ({})",
        lane.run_id,
        handoff_text,
        marker_line
            .as_deref()
            .unwrap_or("no BLOCKED_ON_HUMAN marker line found")
    );
    (action, command, detail, specs)
}

/// The spec a relay run was working, from its run record (`ticket=` in
/// relay.status). `None` when the record names none.
// trace:STORY-1700 | ai:claude
///
/// Only an id the loaded store knows (`known`) counts: a `PREFIX-N` token
/// the store has never seen is not the lane's spec, so an unknown spec still
/// fails closed.
// trace:TASK-1955 | ai:claude (TASK-1925)
pub(crate) fn run_spec(lane: &Lane, known: &dyn Fn(&str) -> bool) -> Option<String> {
    ["ticket", "spec"]
        .iter()
        .filter_map(|k| lane.evidence.field(k))
        .flat_map(spec_ids_in)
        .find(|id| known(id))
}

/// Why a lane whose run spec is unknown is gated (fail closed).
const RUN_SPEC_UNKNOWN: &str =
    "the spec this relay run was working could not be determined from its run record";

/// Every `PREFIX-123` token in `text`, in order.
fn spec_ids_in(text: &str) -> Vec<String> {
    static ID: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = ID.get_or_init(|| regex::Regex::new(r"\b([A-Z][A-Z0-9]*-[0-9]+)\b").unwrap());
    re.captures_iter(text).map(|c| c[1].to_string()).collect()
}

/// Is `req` a spec awaiting an advisor signoff? Fail closed: an open spec is
/// awaiting one unless a comment starting `ADVISOR SIGNOFF` is newer than its
/// every other comment. No keyword in the request is needed.
// trace:STORY-1700 | ai:claude
pub(crate) fn awaits_signoff(req: &aida_core::Requirement) -> bool {
    if crate::is_terminal_status_str(&req.status.to_string()) {
        return false;
    }
    // A ruling starts `ADVISOR SIGNOFF`; "READY for advisor signoff" in a
    // sketch comment is a request, not a ruling.
    let is_signoff = |c: &aida_core::Comment| {
        c.content
            .trim_start()
            .to_ascii_uppercase()
            .starts_with("ADVISOR SIGNOFF")
    };
    let last_signoff = req
        .comments
        .iter()
        .filter(|c| is_signoff(c))
        .map(|c| c.created_at)
        .max();
    let Some(last_signoff) = last_signoff else {
        return true;
    };
    req.comments
        .iter()
        .any(|c| !is_signoff(c) && c.created_at > last_signoff)
}

/// The routing for one relay lane, per the R7 classification table.
// trace:STORY-1700 | ai:claude
pub(crate) fn plan_relay_lane(
    lane: &Lane,
    spec_signoff: &dyn Fn(&str) -> Option<bool>,
) -> LaneDecision {
    let key_lane = format!("relay:{}", lane.run_id);
    let head = format!(
        "{}/{}",
        lane.evidence.field("updated").unwrap_or("?"),
        lane.evidence.field("exit").unwrap_or("?")
    );
    let subject = format!("relay run {}", lane.run_id);
    let handoff = lane
        .evidence
        .field("handoff")
        .unwrap_or("(none recorded)")
        .to_string();
    let act = |action, route, command: String, detail: String, specs: Vec<String>| {
        let mut specs = specs.into_iter();
        LaneDecision::Act(Planned {
            lane: key_lane.clone(),
            subject: subject.clone(),
            action,
            route,
            head: head.clone(),
            command,
            detail,
            spec: specs.next(),
            forced_gate: None,
            other_specs: specs.collect(),
        })
    };
    // Re-dispatching or fixing a run acts on the spec it was working: carry
    // it for the floors, and gate the lane when it is unknown (fail closed).
    // trace:STORY-1700 | ai:claude
    let act_on_run = |action, command: String, detail: String| {
        let spec = run_spec(lane, &|id| spec_signoff(id).is_some());
        let missing = spec.is_none();
        let mut d = act(
            action,
            Route::Advisor,
            command,
            detail,
            spec.into_iter().collect(),
        );
        if let (true, LaneDecision::Act(p)) = (missing, &mut d) {
            p.forced_gate = Some(RUN_SPEC_UNKNOWN.into());
        }
        d
    };
    match lane.class {
        LaneClass::Running | LaneClass::Complete | LaneClass::Acknowledged => LaneDecision::None,
        LaneClass::SignalStopped => LaneDecision::Skip {
            lane: key_lane.clone(),
            subject,
            head,
            why: "deliberate stop (signal exit)".into(),
        },
        LaneClass::RunLocked => LaneDecision::Skip {
            lane: key_lane.clone(),
            subject,
            head,
            why: "duplicate launch; the live run continues".into(),
        },
        class if lane.seat_kind == SeatKind::Advisor => act(
            DriveAction::OperatorAttention,
            Route::Operator,
            format!("read {handoff} and relaunch the advisor relay yourself"),
            format!(
                "advisor-seat relay run {} is {} (the tick never relaunches an advisor seat)",
                lane.run_id,
                class.as_str()
            ),
            Vec::new(),
        ),
        LaneClass::TerminalApi => act(
            DriveAction::OperatorAttention,
            Route::Operator,
            "restore API credits or quota, then relaunch the run".into(),
            format!(
                "relay run {} stopped on a terminal API error (exit 7); handoff: {handoff}",
                lane.run_id
            ),
            Vec::new(),
        ),
        LaneClass::Blocked => {
            let (action, command, detail, spec) = resolving_action(lane, spec_signoff);
            act(action, Route::Advisor, command, detail, spec)
        }
        LaneClass::Crashed => act_on_run(
            DriveAction::Redispatch,
            format!(
                "read {handoff}, then relaunch the run with bin/relay.sh against the same handoff (as a separate session, never an in-session subagent)"
            ),
            format!(
                "relay run {} crashed: its owner died without the exit trap running",
                lane.run_id
            ),
        ),
        LaneClass::SessionCap => act_on_run(
            DriveAction::Redispatch,
            format!(
                "dispatch a continuation: relaunch with bin/relay.sh against {handoff} (a separate session)"
            ),
            format!(
                "relay run {} reached its session cap with the task not complete",
                lane.run_id
            ),
        ),
        LaneClass::NoSlot => act_on_run(
            DriveAction::Redispatch,
            format!("relaunch with bin/relay.sh against {handoff} when a relay slot frees"),
            format!("relay run {} found no free relay slot (exit 8)", lane.run_id),
        ),
        LaneClass::SetupError | LaneClass::UsageBin => act_on_run(
            DriveAction::FixSetup,
            format!(
                "fix the relay's setup (exit {}), then relaunch against {handoff}",
                lane.evidence.field("exit").unwrap_or("?")
            ),
            format!(
                "relay run {} stopped before any session ({})",
                lane.run_id,
                lane.class.as_str()
            ),
        ),
    }
}

// ── Crashed-relay restart (STORY-1706) ──────────────────────────────────────

/// The least mandate time left for a restart.
const RESTART_MANDATE_FLOOR: Duration = Duration::minutes(30);

/// A restart the tick may run: everything comes from the launch record and
/// the lineage file, nothing from the environment or the current config.
// trace:STORY-1706 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestartPlan {
    pub lane: String,
    pub run_id: String,
    pub run_dir: PathBuf,
    pub relay_sh: PathBuf,
    pub cwd: PathBuf,
    /// The record's `arg=` lines, then `--max-sessions <remaining>` (C1).
    pub argv: Vec<String>,
    /// The record's sha256: the lineage key and `AIDA_RELAY_RESTART`.
    pub launch_sha: String,
    pub generation: u32,
    pub max_sessions: u32,
    pub used_before: u32,
    /// The crashed status this restart answers (`updated/state/session`).
    pub crashed_head: String,
}

/// What the tick does about one crashed lane.
// trace:STORY-1706 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RestartVerdict {
    Restart(RestartPlan),
    /// Backing off: the tick retries later, so the lane is not routed.
    Wait(String),
    /// Not restartable: the lane routes to the advisor as before.
    No(String),
    /// The lineage used its restarts: one operator notice per launch sha.
    CrashLoop(String),
}

fn crashed_head(lane: &Lane) -> String {
    let f = |k| lane.evidence.field(k).unwrap_or("-");
    format!("{}/{}/{}", f("updated"), f("state"), f("session"))
}

/// The handoff's first line says the task is done or blocked.
fn handoff_settled(lane: &Lane) -> bool {
    let Some(h) = lane.evidence.field("handoff") else {
        return false;
    };
    let p = PathBuf::from(h);
    let p = if p.is_absolute() {
        p
    } else {
        lane.run_dir.join(p)
    };
    std::fs::read_to_string(p).is_ok_and(|t| {
        t.lines().find(|l| !l.trim().is_empty()).is_some_and(|l| {
            let l = l.trim_start();
            l.starts_with("TASK_COMPLETE") || l.starts_with("BLOCKED_ON_HUMAN")
        })
    })
}

/// Is any process of this user still carrying `AIDA_RELAY_RUN=<run>` (every
/// relay session does)? `None` when `/proc` cannot be read: unknown, so the
/// caller refuses. Processes of other users are unreadable and skipped.
// trace:STORY-1706 | ai:claude
pub(crate) fn run_process_alive(run: &str) -> Option<bool> {
    let needle = format!("AIDA_RELAY_RUN={run}");
    let me = std::process::id().to_string();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        if pid == me {
            continue;
        }
        let Ok(environ) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        if environ
            .split(|b| *b == 0)
            .any(|var| var == needle.as_bytes())
        {
            return Some(true);
        }
    }
    Some(false)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(p)
        .ok()
        .filter(|m| m.is_file())
        .or_else(|| std::fs::metadata(p).ok().filter(|m| m.is_file()))
        .is_some_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// STORY-1706 (R4): may the tick restart this lane? `None` when the lane is
/// not the crash ending at all. Every condition of the advisor's ruling must
/// hold; the first that fails is the ledgered reason. Reads the record, the
/// lineage, the marker and the profile from disk afresh.
// trace:STORY-1706 | ai:claude
pub(crate) fn restart_verdict(
    lane: &Lane,
    mandate: &Mandate,
    project_root: &Path,
    gates: &std::result::Result<HumanGates, String>,
    spec_always_human: &dyn Fn(&str) -> Option<String>,
    spec_known: &dyn Fn(&str) -> bool,
    now: DateTime<Utc>,
) -> Option<RestartVerdict> {
    use crate::relay_launch as rl;
    use RestartVerdict::{CrashLoop, No, Wait};
    if lane.class != LaneClass::Crashed || !lane.evidence.trap_never_ran() {
        return None;
    }
    let no = |why: String| Some(No(why));
    if !mandate.covers(Scope::RelayRestart) {
        return no("the mandate does not cover relay-restart (start a new drive mandate)".into());
    }
    if let Some(why) = crate::mandate_gates::gate_reason(gates, GateOp::RelayRestart) {
        return no(format!("gated: {why}"));
    }
    if lane
        .run_dir
        .join(rl::NORESTART_FILE)
        .symlink_metadata()
        .is_ok()
    {
        return no("the operator marked this run --no-restart".into());
    }
    if handoff_settled(lane) {
        return no("its handoff says the task is complete or blocked".into());
    }
    if lane.seat_kind == SeatKind::Advisor {
        return no("an advisor-seat run is never restarted".into());
    }
    // A relay killed alone leaves its agent session running (A2).
    match run_process_alive(&lane.run_id) {
        Some(false) => {}
        Some(true) => {
            return no("an agent session of this run is still running; stop it first".into())
        }
        None => return no("cannot tell whether a session of this run is still running".into()),
    }
    let record = match rl::load(&lane.run_dir) {
        Ok(r) => r,
        Err(why) => return no(format!("no usable launch record: {why}")),
    };
    if record.run != lane.run_id {
        return no(format!(
            "the launch record names run {}, not {}",
            record.run, lane.run_id
        ));
    }
    if !same_dir(&record.project_root, project_root) {
        return no("the launch record names another project root".into());
    }
    if !executable_file(&record.relay_sh) {
        return no(format!(
            "{} is not an executable file",
            record.relay_sh.display()
        ));
    }
    if record.principal.trim() != mandate.principal.trim() {
        return no(format!(
            "launched by {}, not the mandate's principal {}",
            record.principal, mandate.principal
        ));
    }
    if !record.launched_from_tty {
        return no("not started by a human at a terminal".into());
    }
    if record.force_seat || record.session_role.trim().eq_ignore_ascii_case("advisor") {
        return no("an advisor or --force-seat run is never restarted".into());
    }
    if let (Some(file), Some(want)) = (&record.profile_file, &record.profile_sha256) {
        if rl::sha256_file(file).as_deref() != Some(want.as_str()) {
            return no(format!("its profile {} changed or is gone", file.display()));
        }
    }
    // The status's spec, else the one the launch named (exact id, known to
    // the store); neither means unknown, refused.
    let Some(spec) = run_spec(lane, spec_known).or_else(|| {
        record
            .spec
            .clone()
            .filter(|id| spec_ids_in(id).first() == Some(id) && spec_known(id))
    }) else {
        return no(RUN_SPEC_UNKNOWN.into());
    };
    if let Some(class) = spec_always_human(&spec) {
        return no(format!("{spec} is {class}"));
    }
    let lineage = match rl::restarts(&lane.run_dir, &record.sha256) {
        Ok(l) => l,
        Err(why) => return no(format!("the restart lineage is unreadable: {why}")),
    };
    if lineage.len() >= rl::MAX_RESTARTS {
        return Some(CrashLoop(record.sha256));
    }
    if let Some(last) = lineage.last() {
        let ticks = BACKOFF_TICKS[(lineage.len() - 1).min(BACKOFF_TICKS.len() - 1)];
        let eligible = last.at + TICK_INTERVAL * ticks;
        if now < eligible {
            return Some(Wait(format!(
                "restart {} backs off until {}",
                lineage.len() + 1,
                eligible.to_rfc3339()
            )));
        }
    }
    // Sessions: the crashed instance started at the record's session (the
    // first launch) or at 1 (a restart's resume argv), and the session it
    // crashed in counts as used.
    let (instance_max, used_before, instance_start) = match lineage.last() {
        Some(l) => (l.max_sessions, l.used_before, 1),
        None => (record.max_sessions, 0, record.start_session),
    };
    let session = match lane.evidence.field("session") {
        None => instance_start.saturating_sub(1),
        Some(s) => match s.parse::<u32>() {
            Ok(n) => n,
            Err(_) => return no(format!("its status names session `{s}`")),
        },
    };
    let used_now = (session + 1)
        .saturating_sub(instance_start)
        .min(instance_max);
    let budget = (record.max_sessions + 1).saturating_sub(record.start_session);
    let used = used_before + used_now;
    let remaining = budget.saturating_sub(used);
    if remaining == 0 {
        return no(format!("no sessions left of its {budget}"));
    }
    if mandate.expires_at - now < RESTART_MANDATE_FLOOR {
        return no("less than 30 minutes left on the mandate".into());
    }
    let mut argv = record.args.clone();
    argv.extend(["--max-sessions".to_string(), remaining.to_string()]);
    Some(RestartVerdict::Restart(RestartPlan {
        lane: format!("relay:{}", lane.run_id),
        run_id: lane.run_id.clone(),
        run_dir: lane.run_dir.clone(),
        relay_sh: record.relay_sh.clone(),
        cwd: record.launch_cwd.clone(),
        argv,
        launch_sha: record.sha256,
        generation: lineage.len() as u32 + 1,
        max_sessions: remaining,
        used_before: used,
        crashed_head: crashed_head(lane),
    }))
}

/// A held PR the tick may route for shipping.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeldPr {
    pub pr: u64,
    /// A typed hold of a kind a mandate may release.
    pub releasable: bool,
    pub head: Option<String>,
    pub spec: Option<String>,
    /// A non-closed approval is recorded at `head`.
    pub approved_at_head: bool,
    /// Every path the PR changes at `head`; `None` = unresolved.
    pub changed_paths: Option<Vec<String>>,
}

/// The routing for one held PR: an approved, releasable hold is a ship for
/// the advisor; a PR that touches the gates, or whose files cannot be
/// resolved, is gated (R1). Anything else is not a drive lane.
// trace:STORY-1700 | ai:claude
pub(crate) fn plan_held_pr(pr: &HeldPr) -> LaneDecision {
    let (true, true, Some(head)) = (pr.releasable, pr.approved_at_head, pr.head.as_ref()) else {
        return LaneDecision::None;
    };
    let forced_gate = match &pr.changed_paths {
        None => Some("the files it changes could not be resolved".to_string()),
        Some(paths) => crate::mandate_gates::touches_gates(paths)
            .map(|p| format!("it changes `{p}`, part of the mandate and its gates")),
    };
    LaneDecision::Act(Planned {
        lane: format!("pr:{}", pr.pr),
        subject: format!("PR #{}", pr.pr),
        action: DriveAction::Ship,
        route: Route::Advisor,
        head: head.clone(),
        command: format!("aida pr ship {}", pr.pr),
        detail: format!(
            "PR #{} is held with an attested approval at {}{}",
            pr.pr,
            crate::review_verdict::short_sha(head),
            pr.spec
                .as_deref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default()
        ),
        spec: pr.spec.clone(),
        forced_gate,
        other_specs: Vec::new(),
    })
}

// ── Ledger rows and effects ─────────────────────────────────────────────────

/// How one action ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Outcome {
    Done,
    Routed,
    Gated,
    Refused,
    Skipped,
}

impl Outcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Routed => "routed",
            Self::Gated => "gated",
            Self::Refused => "refused",
            Self::Skipped => "skipped",
        }
    }
}

/// One `MandateDriveAction` row.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ActionRow {
    pub lane: String,
    pub subject: String,
    pub action: String,
    pub outcome: Outcome,
    pub why: Option<String>,
}

impl ActionRow {
    pub(crate) fn new(
        lane: &str,
        subject: &str,
        action: &str,
        outcome: Outcome,
        why: Option<String>,
    ) -> Self {
        Self {
            lane: lane.into(),
            subject: subject.into(),
            action: action.into(),
            outcome,
            why,
        }
    }
}

/// Everything a tick does to the world. Production writes events, mail and
/// notifications; tests record. Only [`DriveEffects::restart_relay`] spawns
/// anything: a crashed relay, from its launch record (STORY-1706).
// trace:STORY-1700 | ai:claude
pub(crate) trait DriveEffects {
    /// Start `plan`'s relay detached, with the scrubbed environment; `Ok` is
    /// its pid. A backend that cannot spawn refuses (fail closed).
    // trace:STORY-1706 | ai:claude
    fn restart_relay(&mut self, plan: &RestartPlan) -> Result<u32> {
        anyhow::bail!("this backend cannot restart {}", plan.run_id)
    }
    /// Emit the advisor due item (event + mail nudge).
    fn emit_due(&mut self, mandate_id: &str, item: &Planned) -> Result<()>;
    /// Tell the operator; `Ok` names the delivery.
    fn notify_operator(&mut self, title: &str, body: &str) -> Result<String>;
    /// Reap sessions whose owner process is dead: one row per thing the reap
    /// did, skipped or failed (lease release, worktree teardown, branch
    /// deletion, agent-registry GC).
    // trace:TASK-1915 | ai:claude
    fn reap_dead_leases(&mut self) -> Vec<ReapRow>;
    /// Ledger one row.
    fn ledger(&mut self, mandate_id: &str, row: &ActionRow);
}

/// One outcome of the dead-session reap (TASK-1915 F4).
// trace:TASK-1915 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReapRow {
    pub subject: String,
    pub outcome: Outcome,
    pub why: String,
}

/// The rows for one quiet reap: the agent-registry GC when it removed or
/// failed anything, and every reapable lease as `reap_one` reported it.
// trace:TASK-1915 | ai:claude
pub(crate) fn reap_rows(reap: crate::session_reap::QuietReap) -> Vec<ReapRow> {
    let mut rows = Vec::new();
    match reap.agent_gc {
        Ok(0) => {}
        Ok(n) => rows.push(ReapRow {
            subject: "agent registry".into(),
            outcome: Outcome::Done,
            why: format!("removed {n} dead agent record(s)"),
        }),
        Err(e) => rows.push(ReapRow {
            subject: "agent registry".into(),
            outcome: Outcome::Refused,
            why: format!("agent registry GC failed: {e}"),
        }),
    }
    rows.extend(reap.leases.into_iter().map(|(session, outcome)| {
        let kind = if outcome.starts_with("reaped") {
            Outcome::Done
        } else if outcome.starts_with("skipped") {
            Outcome::Skipped
        } else {
            Outcome::Refused
        };
        ReapRow {
            subject: format!("session {session}"),
            outcome: kind,
            why: outcome,
        }
    }));
    rows
}

// ── Shared state ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Backoff {
    pub attempts: u32,
    pub last_at: Option<DateTime<Utc>>,
    pub next_eligible_at: Option<DateTime<Utc>>,
}

/// Per-mandate tick state, shared by every invocation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PerMandate {
    /// Due / gated / skip keys already acted on (dedup across ticks).
    #[serde(default)]
    pub emitted: BTreeSet<String>,
    #[serde(default)]
    pub backoff: BTreeMap<String, Backoff>,
    #[serde(default)]
    pub actions: u64,
    /// When the current no-advisor episode began (one notice per episode).
    #[serde(default)]
    pub absence_episode: Option<DateTime<Utc>>,
    #[serde(default)]
    pub mismatch_notified: bool,
    /// Per approved spec, its dispatch epoch (TASK-1915 F8).
    #[serde(default)]
    pub dispatch: BTreeMap<String, DispatchEpoch>,
}

/// Where a spec's unowned-dispatch lane stands. A routed dispatch opens an
/// epoch; it closes once the spec was seen owned since (its fixer took it
/// and is gone again) or no owner showed up within [`DISPATCH_STALE`] (the
/// fixer died before claiming). The next unowned sighting is then a new item.
// trace:TASK-1915 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DispatchEpoch {
    #[serde(default)]
    pub epoch: u32,
    #[serde(default)]
    pub routed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub owned_since_route: bool,
}

impl PerMandate {
    /// `s`'s dispatch epoch now, advancing it per [`DispatchEpoch`]. An
    /// unknown PR state neither opens nor closes an epoch.
    // trace:TASK-1915 | ai:claude
    fn dispatch_epoch(&mut self, s: &UnownedSpec, now: DateTime<Utc>) -> u32 {
        let d = self.dispatch.entry(s.id.to_ascii_uppercase()).or_default();
        if s.owned() {
            if d.routed_at.is_some() {
                d.owned_since_route = true;
            }
        } else if s.open_pr.is_some() {
            if let Some(at) = d.routed_at {
                if d.owned_since_route || now - at >= DISPATCH_STALE {
                    d.epoch += 1;
                    d.routed_at = None;
                    d.owned_since_route = false;
                }
            }
        }
        d.epoch
    }
}

/// `.aida/mandate-drive-state.json`, keyed by mandate id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DriveState {
    #[serde(default)]
    pub mandates: BTreeMap<String, PerMandate>,
}

pub(crate) fn state_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(STATE_FILE)
}

/// The drive state. Absent is a fresh state; unreadable or corrupt is an
/// error, never a silent reset of dedup, backoff and notices (TASK-1915 F6).
// trace:TASK-1915 | ai:claude
pub(crate) fn load_state(project_root: &Path) -> Result<DriveState> {
    let path = state_path(project_root);
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("{} is corrupt", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DriveState::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn save_state(project_root: &Path, state: &DriveState) -> Result<()> {
    let path = state_path(project_root);
    let dir = path.parent().context("state path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{STATE_FILE}.tmp-{}", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The refusal already ledgered for an unreadable drive state, beside it.
// trace:TASK-1915 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StateRefusal {
    fingerprint: String,
    notified: bool,
}

fn state_refusal_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".aida")
        .join("mandate-drive-state.refused.json")
}

fn read_state_refusal(project_root: &Path) -> Option<StateRefusal> {
    serde_json::from_slice(&std::fs::read(state_refusal_path(project_root)).ok()?).ok()
}

fn write_state_refusal(project_root: &Path, refusal: &StateRefusal) {
    if let Ok(bytes) = serde_json::to_vec(refusal) {
        let _ = aida_core::fs_atomic::write_atomic(&state_refusal_path(project_root), &bytes);
    }
}

fn clear_state_refusal(project_root: &Path) {
    let _ = std::fs::remove_file(state_refusal_path(project_root));
}

/// What identifies one unreadable state: its bytes (FNV-1a), or the read
/// error when it has none.
fn state_fingerprint(project_root: &Path, err: &anyhow::Error) -> String {
    let bytes =
        std::fs::read(state_path(project_root)).unwrap_or_else(|_| format!("{err:#}").into_bytes());
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// A held drive lock. Drop releases it with an explicit unlock rather than
/// by closing the file: a child forked by any concurrent thread holds a copy
/// of the descriptor until its exec and would keep the lock held through it
/// (the BUG-2160 shift-lock lesson).
struct DriveLock(std::fs::File);

impl Drop for DriveLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

/// The dedicated drive lock, or `None` when another tick holds it.
fn try_drive_lock(project_root: &Path) -> Result<Option<DriveLock>> {
    use fs2::FileExt;
    let dir = project_root.join(".aida");
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(LOCK_FILE))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(DriveLock(file))),
        Err(err) if aida_core::file_lock::is_lock_contended(&err) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

// ── Authority ───────────────────────────────────────────────────────────────

/// Is any unexpired, unstopped DRIVE mandate record present, whoever its
/// principal? The `mandate.drive_active` predicate; the tick decides
/// authority itself.
// trace:STORY-1700 | ai:claude
pub(crate) fn drive_record_active(now: DateTime<Utc>) -> bool {
    crate::mandate::mandate_dir()
        .map(|dir| !active_drive_records(&dir, now).is_empty())
        .unwrap_or(false)
}

/// Every active drive record in `dir`, unfiltered by principal (the R4
/// mismatch probe). Never authority: only [`crate::mandate::load_valid`] is.
fn active_drive_records(dir: &Path, now: DateTime<Utc>) -> Vec<Mandate> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let stem = p.file_stem()?.to_str()?.to_string();
            if p.extension()?.to_str()? != "json" || uuid::Uuid::parse_str(&stem).is_err() {
                return None;
            }
            let m: Mandate = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            let stopped = std::fs::symlink_metadata(dir.join(format!("{stem}.stopped"))).is_ok();
            (m.id == stem
                && m.is_drive()
                && !stopped
                && m.revoked_at.is_none()
                && now < m.expires_at)
                .then_some(m)
        })
        .collect()
}

/// What a tick reads to act.
// trace:STORY-1700 | ai:claude
pub(crate) struct TickInputs<'a> {
    /// Where the lock, the state file and the ledger live.
    pub project_root: &'a Path,
    pub mandate_dir: &'a Path,
    /// The principal this tick resolves (`current_user_id`).
    pub principal: &'a str,
    /// The tick's clock, read afresh before every action.
    pub clock: &'a dyn Fn() -> DateTime<Utc>,
    pub gates: std::result::Result<HumanGates, String>,
    pub lanes: Vec<Lane>,
    pub held_prs: Vec<HeldPr>,
    pub advisor_live: bool,
    /// Did the tick's own environment carry a session grant/role/purpose?
    pub inherited_session_env: bool,
    /// `Some(class)` when a spec is always human (operator-only, decide).
    pub spec_always_human: &'a dyn Fn(&str) -> Option<String>,
    /// `None` = not a store spec; `Some(awaiting a signoff)` otherwise.
    pub spec_signoff: &'a dyn Fn(&str) -> Option<bool>,
    /// Could the store be read? When not, every gated seat action is
    /// withheld: the spec checks it needs cannot be made (fail closed).
    pub store_readable: bool,
    /// Approved specs and who (if anyone) already owns them (AC1).
    pub unowned_specs: Vec<UnownedSpec>,
}

/// Test seams.
#[derive(Default)]
pub(crate) struct TickHooks<'a> {
    /// Runs before side effect `n` (0-based), before the mandate re-check.
    pub before_action: Option<&'a dyn Fn(usize)>,
    /// Replaces the state save.
    // trace:TASK-1915 | ai:claude
    pub save: Option<&'a dyn Fn(&Path, &DriveState) -> Result<()>>,
}

/// What one tick did.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct TickReport {
    pub mandate_id: Option<String>,
    /// Why the tick did nothing at all, when it did nothing.
    pub idle: Option<String>,
    pub rows: Vec<ActionRow>,
}

struct Tick<'a, 'b> {
    inp: &'a TickInputs<'b>,
    hooks: &'a TickHooks<'a>,
    fx: &'a mut dyn DriveEffects,
    mandate_id: String,
    side_effects: usize,
    budget: usize,
    ended: Option<String>,
    report: TickReport,
}

impl Tick<'_, '_> {
    fn row(&mut self, row: ActionRow) {
        self.fx.ledger(&self.mandate_id, &row);
        self.report.rows.push(row);
    }

    /// A failed recheck before a substrate step: ledger the refusal and end
    /// the tick (TASK-1915 F3).
    // trace:TASK-1915 | ai:claude
    fn end(&mut self, lane: &str, subject: &str, action: &str, why: String) {
        self.row(ActionRow::new(
            lane,
            subject,
            action,
            Outcome::Refused,
            Some(why.clone()),
        ));
        self.ended = Some(why);
    }

    /// Ledger a skip row once per `(key, reason)`: a lane skipped for the
    /// same reason every tick is one row, not one per tick (TASK-1915 F7).
    // trace:TASK-1915 | ai:claude
    fn skip_once(&mut self, per: &mut PerMandate, key: &str, row: ActionRow) {
        let seen = format!("{key}|skip|{}", row.why.as_deref().unwrap_or_default());
        if per.emitted.insert(seen) {
            self.row(row);
        }
    }

    /// R3: re-read the mandate immediately before a side effect. `Err` ends
    /// the tick: this action is refused and the caller skips the rest.
    fn recheck(&mut self) -> std::result::Result<(), String> {
        if let Some(why) = &self.ended {
            return Err(why.clone());
        }
        if let Some(hook) = self.hooks.before_action {
            hook(self.side_effects);
        }
        self.side_effects += 1;
        let now = (self.inp.clock)();
        match crate::mandate::recheck(
            self.inp.mandate_dir,
            &self.mandate_id,
            self.inp.principal,
            now,
        ) {
            Ok(m) if m.is_drive() => Ok(()),
            Ok(_) => Err("mandate is no longer a drive mandate".into()),
            Err(MandateRefusal::Expired { .. }) => Err("mandate expired mid-tick".into()),
            Err(_) => Err("mandate stopped mid-tick".into()),
        }
    }
}

/// STORY-1706 (R4/R5): restart every crashed lane [`restart_verdict`]
/// allows. Before the exec: re-read the mandate, re-probe the lane and judge
/// it again from fresh reads, then append the lineage line (no line, no
/// exec). Returns, per lane key, `None` to leave the lane unrouted this tick
/// (restarted or backing off) or `Some(why)` to note on its advisor item.
// trace:STORY-1706 | ai:claude
fn restart_crashed(
    tick: &mut Tick<'_, '_>,
    per: &mut PerMandate,
    mandate: &Mandate,
    acted: &mut usize,
) -> BTreeMap<String, Option<String>> {
    const ACTION: &str = "relay-restart";
    let inp = tick.inp;
    let known = |id: &str| (inp.spec_signoff)(id).is_some();
    let verdict = |lane: &Lane| {
        restart_verdict(
            lane,
            mandate,
            inp.project_root,
            &inp.gates,
            inp.spec_always_human,
            &known,
            (inp.clock)(),
        )
    };
    let mut notes = BTreeMap::new();
    for lane in &inp.lanes {
        let Some(v) = verdict(lane) else {
            continue;
        };
        let key = format!("relay:{}", lane.run_id);
        let subject = format!("relay run {}", lane.run_id);
        let skip = |tick: &mut Tick<'_, '_>, per: &mut PerMandate, why: String| {
            let row = ActionRow::new(&key, &subject, ACTION, Outcome::Skipped, Some(why));
            tick.skip_once(per, &format!("{key}|{ACTION}|{}", crashed_head(lane)), row);
        };
        let plan = match v {
            RestartVerdict::Wait(why) => {
                skip(tick, per, why);
                notes.insert(key.clone(), None);
                continue;
            }
            // No row of its own: the reason rides on the lane's advisor
            // item, which is routed as before.
            RestartVerdict::No(why) => {
                notes.insert(key.clone(), Some(why));
                continue;
            }
            RestartVerdict::CrashLoop(sha) => {
                let why = format!(
                    "restarted {} times; it keeps crashing",
                    crate::relay_launch::MAX_RESTARTS
                );
                let loop_key = format!("{key}|crash-loop|{sha}");
                if !per.emitted.contains(&loop_key) && tick.ended.is_none() {
                    match tick.recheck() {
                        Err(why) => tick.end(&key, &subject, ACTION, why),
                        Ok(()) => {
                            let sent = tick.fx.notify_operator(
                                &format!("AIDA drive: {subject} is crash-looping"),
                                &format!(
                                    "{subject} crashed again after {} automatic restarts; the drive tick will not restart it again. Read {} and relaunch it yourself.",
                                    crate::relay_launch::MAX_RESTARTS,
                                    lane.run_dir.display()
                                ),
                            );
                            let (outcome, note) = match sent {
                                Ok(d) => {
                                    per.emitted.insert(loop_key);
                                    (Outcome::Routed, format!("{why}; operator notified ({d})"))
                                }
                                Err(e) => {
                                    (Outcome::Refused, format!("{why}; notify failed: {e:#}"))
                                }
                            };
                            tick.row(ActionRow::new(&key, &subject, ACTION, outcome, Some(note)));
                        }
                    }
                }
                notes.insert(key.clone(), Some(why));
                continue;
            }
            RestartVerdict::Restart(plan) => plan,
        };
        if let Some(why) = tick.ended.clone() {
            skip(tick, per, why.clone());
            notes.insert(key.clone(), Some(why));
            continue;
        }
        if *acted >= tick.budget {
            skip(
                tick,
                per,
                format!("budget of {} actions per tick reached", tick.budget),
            );
            notes.insert(key.clone(), None);
            continue;
        }
        let refuse = |tick: &mut Tick<'_, '_>, why: String| {
            tick.row(ActionRow::new(
                &key,
                &subject,
                ACTION,
                Outcome::Refused,
                Some(why),
            ));
        };
        if let Err(why) = tick.recheck() {
            refuse(tick, why.clone());
            tick.ended = Some(why.clone());
            notes.insert(key.clone(), Some(why));
            continue;
        }
        // Judge the lane again from fresh reads: the lock, the owner, the
        // status, the record, the lineage, the profile and the clock.
        let fresh = crate::fleet_lanes::probe_lane(lane.run_dir.clone());
        let again = fresh.as_ref().and_then(|l| verdict(l));
        match again {
            Some(RestartVerdict::Restart(p))
                if p.launch_sha == plan.launch_sha
                    && p.argv == plan.argv
                    && p.crashed_head == plan.crashed_head => {}
            _ => {
                let why = "the run changed before the restart; left for the next tick".to_string();
                refuse(tick, why);
                notes.insert(key.clone(), None);
                continue;
            }
        }
        let line = crate::relay_launch::RestartLine {
            generation: plan.generation,
            at: (inp.clock)(),
            launch: plan.launch_sha.clone(),
            mandate: tick.mandate_id.clone(),
            max_sessions: plan.max_sessions,
            used_before: plan.used_before,
        };
        if let Err(why) = crate::relay_launch::append_restart(&plan.run_dir, &line) {
            let why = format!("cannot record the restart, so it does not run: {why}");
            refuse(tick, why.clone());
            notes.insert(key.clone(), Some(why));
            continue;
        }
        *acted += 1;
        match tick.fx.restart_relay(&plan) {
            Ok(pid) => {
                per.actions += 1;
                tick.row(ActionRow::new(
                    &key,
                    &subject,
                    ACTION,
                    Outcome::Done,
                    Some(format!(
                        "restart {} of {} (pid {pid}, --max-sessions {})",
                        plan.generation,
                        crate::relay_launch::MAX_RESTARTS,
                        plan.max_sessions
                    )),
                ));
                notes.insert(key.clone(), None);
            }
            Err(e) => {
                // The lineage line stands: a failed spawn counts toward the
                // cap and backs off like a crash.
                refuse(tick, format!("restart {} failed: {e:#}", plan.generation));
                notes.insert(key.clone(), None);
            }
        }
    }
    notes
}

/// STORY-1700: one drive tick over `inp`. See the module docs.
// trace:STORY-1700 | ai:claude
pub(crate) fn run_tick(
    inp: &TickInputs<'_>,
    hooks: &TickHooks<'_>,
    fx: &mut dyn DriveEffects,
) -> TickReport {
    let now = (inp.clock)();
    let (valid, _) = crate::mandate::load_valid(inp.mandate_dir, inp.principal, now);
    let mandate = match crate::mandate::state_at(&valid, now) {
        MandateState::Active(m) if m.is_drive() => Some(m),
        _ => None,
    };

    // The dedicated lock, held for the whole tick.
    let lock = match try_drive_lock(inp.project_root) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            let id = mandate.as_ref().map(|m| m.id.clone()).unwrap_or_default();
            let row = ActionRow::new(
                "tick",
                "drive tick",
                "tick",
                Outcome::Skipped,
                Some("another drive tick holds the lock".into()),
            );
            fx.ledger(&id, &row);
            return TickReport {
                mandate_id: Some(id).filter(|i| !i.is_empty()),
                idle: None,
                rows: vec![row],
            };
        }
        Err(e) => {
            return TickReport {
                idle: Some(format!("cannot take the drive lock: {e:#}")),
                ..Default::default()
            }
        }
    };
    // An unreadable state fails closed: acting on a reset state would
    // re-route every deduplicated item and re-send every notice.
    // trace:TASK-1915 | ai:claude
    let save = |state: &DriveState| match hooks.save {
        Some(save) => save(inp.project_root, state),
        None => save_state(inp.project_root, state),
    };
    let mut state = match load_state(inp.project_root) {
        Ok(state) => {
            clear_state_refusal(inp.project_root);
            state
        }
        Err(e) => {
            let why = format!(
                "the drive state is unreadable ({e:#}); the tick refuses to act rather than reset its dedup, backoff and notices. Repair or remove the file"
            );
            let mut report = TickReport {
                mandate_id: mandate.as_ref().map(|m| m.id.clone()),
                idle: Some(why.clone()),
                rows: Vec::new(),
            };
            // One ledgered refusal per state fingerprint and one operator
            // notice, never every tick; and no row without an own active
            // mandate to ledger it under. trace:TASK-1915 | ai:claude
            if let Some(m) = &mandate {
                let fingerprint = state_fingerprint(inp.project_root, &e);
                let mut seen = read_state_refusal(inp.project_root)
                    .filter(|r| r.fingerprint == fingerprint)
                    .unwrap_or_else(|| {
                        let row = ActionRow::new(
                            "tick",
                            "drive tick",
                            "state-check",
                            Outcome::Refused,
                            Some(why.clone()),
                        );
                        fx.ledger(&m.id, &row);
                        report.rows.push(row);
                        StateRefusal {
                            fingerprint,
                            notified: false,
                        }
                    });
                if !seen.notified {
                    seen.notified = fx
                        .notify_operator(
                            "AIDA drive: the drive state is unreadable",
                            &format!(
                                "{why}: {}. The drive tick does nothing until it reads again.",
                                state_path(inp.project_root).display()
                            ),
                        )
                        .is_ok();
                }
                write_state_refusal(inp.project_root, &seen);
            }
            drop(lock);
            return report;
        }
    };

    let Some(mandate) = mandate else {
        // R4: a drive record for ANOTHER principal means the driver resolves
        // the wrong user; say so once per mandate and do nothing.
        let foreign: Vec<Mandate> = active_drive_records(inp.mandate_dir, now)
            .into_iter()
            .filter(|m| m.principal.trim() != inp.principal.trim())
            .collect();
        let mut report = TickReport {
            idle: Some("no active drive mandate".into()),
            ..Default::default()
        };
        for m in foreign {
            let per = state.mandates.entry(m.id.clone()).or_default();
            if per.mismatch_notified {
                continue;
            }
            let sent = fx.notify_operator(
                "AIDA drive mandate is not acting",
                &format!(
                    "Drive mandate {} belongs to `{}`, but the drive tick resolves `{}`. Set AIDA_USER={} in the schedule driver's environment.",
                    m.id, m.principal, inp.principal, m.principal
                ),
            );
            // Only a delivered notice marks the mismatch told: a failed one
            // retries next tick. trace:TASK-1915 | ai:claude
            let mut why = format!(
                "principal mismatch: record={} tick={}",
                m.principal, inp.principal
            );
            match &sent {
                Ok(_) => per.mismatch_notified = true,
                Err(e) => why.push_str(&format!("; notify failed: {e:#}")),
            }
            let row = ActionRow::new("tick", "drive tick", "tick", Outcome::Refused, Some(why));
            fx.ledger(&m.id, &row);
            report.mandate_id = Some(m.id.clone());
            report.rows.push(row);
        }
        // trace:TASK-1915 | ai:claude
        if let Err(e) = save(&state) {
            report.idle = Some(format!(
                "no active drive mandate; could not save the drive state: {e:#}"
            ));
        }
        drop(lock);
        return report;
    };

    let budget = inp
        .gates
        .as_ref()
        .ok()
        .and_then(|g| g.max_actions_per_tick)
        .unwrap_or(DEFAULT_MAX_ACTIONS);
    let mut tick = Tick {
        inp,
        hooks,
        fx,
        mandate_id: mandate.id.clone(),
        side_effects: 0,
        budget,
        ended: None,
        report: TickReport {
            mandate_id: Some(mandate.id.clone()),
            ..Default::default()
        },
    };
    let per = state.mandates.entry(mandate.id.clone()).or_default();

    if inp.inherited_session_env {
        tick.row(ActionRow::new(
            "tick",
            "drive tick",
            "session-env",
            Outcome::Skipped,
            Some("inherited session env ignored".into()),
        ));
    }

    // Substrate: the dead-session reap, every outcome ledgered.
    // trace:TASK-1915 | ai:claude
    match tick.recheck() {
        Err(why) => tick.end("leases", "session leases", "reap", why),
        Ok(()) => {
            for r in tick.fx.reap_dead_leases() {
                let row = ActionRow::new("leases", &r.subject, "reap", r.outcome, Some(r.why));
                match r.outcome {
                    Outcome::Skipped => {
                        tick.skip_once(per, &format!("leases|{}", r.subject), row);
                    }
                    Outcome::Done => {
                        per.actions += 1;
                        tick.row(row);
                    }
                    _ => tick.row(row),
                }
            }
        }
    }

    // No live advisor: one operator notice per absence episode; seat items
    // stay queued for the next advisor (R5).
    if inp.advisor_live {
        per.absence_episode = None;
    } else if per.absence_episode.is_none() && tick.ended.is_none() {
        match tick.recheck() {
            // trace:TASK-1915 | ai:claude
            Err(why) => tick.end("advisor", "advisor seat", "escalate", why),
            Ok(()) => {
                let sent = tick.fx.notify_operator(
                    "AIDA drive mandate: no advisor seat is live",
                    "The drive tick keeps doing substrate actions; ships, sketch signoffs and dispatches wait in `aida awaiting` for the next advisor seat.",
                );
                // Only a delivered notice opens the episode: a failed one
                // retries next tick.
                if sent.is_ok() {
                    per.absence_episode = Some(now);
                }
                per.actions += 1;
                tick.row(ActionRow::new(
                    "advisor",
                    "advisor seat",
                    "escalate",
                    if sent.is_ok() {
                        Outcome::Done
                    } else {
                        Outcome::Refused
                    },
                    Some(match sent {
                        Ok(d) => format!("no live advisor seat; operator notified ({d})"),
                        Err(e) => format!("no live advisor seat; notify failed: {e:#}"),
                    }),
                ));
            }
        }
    }

    // STORY-1706: restart crashed relays the human launch record allows.
    // A lane restarted or backing off is not routed this tick; any other
    // reason rides on the lane's advisor item.
    let mut acted = 0usize;
    let restart_notes = restart_crashed(&mut tick, per, &mandate, &mut acted);
    let mut decisions: Vec<LaneDecision> = inp
        .lanes
        .iter()
        .map(|l| plan_relay_lane(l, inp.spec_signoff))
        .filter_map(|d| match d {
            LaneDecision::Act(mut p) => match restart_notes.get(&p.lane) {
                Some(None) => None,
                Some(Some(why)) => {
                    p.detail.push_str(&format!("; not auto-restarted: {why}"));
                    Some(LaneDecision::Act(p))
                }
                None => Some(LaneDecision::Act(p)),
            },
            d => Some(d),
        })
        .collect();
    decisions.extend(inp.held_prs.iter().map(plan_held_pr));
    // Each unowned spec carries its dispatch epoch, so a dead fixer's spec
    // is routed again. trace:TASK-1915 | ai:claude
    decisions.extend(inp.unowned_specs.iter().map(|s| {
        let mut s = s.clone();
        s.epoch = per.dispatch_epoch(&s, now);
        plan_unowned_spec(&s)
    }));
    // An unreadable store: no operator-only or signoff check can be made,
    // so every gated seat action is withheld, with one ledgered refusal and
    // one operator notice per episode. trace:STORY-1700 | ai:claude
    const STORE_KEY: &str = "store|unreadable";
    if inp.store_readable {
        per.emitted.remove(STORE_KEY);
    } else {
        let gated =
            |d: &LaneDecision| matches!(d, LaneDecision::Act(p) if p.action.gate_op().is_some());
        let withheld = decisions.iter().filter(|d| gated(d)).count();
        decisions.retain(|d| !gated(d));
        if tick.ended.is_none() && !per.emitted.contains(STORE_KEY) {
            match tick.recheck() {
                // trace:TASK-1915 | ai:claude
                Err(why) => tick.end("store", "requirement store", "store-check", why),
                Ok(()) => {
                    let sent = tick.fx.notify_operator(
                        "AIDA drive: the requirement store is unreadable",
                        "The drive tick cannot check operator-only specs or signoffs, so it withholds every ship, signoff, answer and dispatch until the store reads again.",
                    );
                    if sent.is_ok() {
                        per.emitted.insert(STORE_KEY.into());
                    }
                    tick.row(ActionRow::new(
                        "store",
                        "requirement store",
                        "store-check",
                        Outcome::Refused,
                        Some(format!(
                            "store unreadable: {withheld} gated seat action(s) withheld; {}",
                            match sent {
                                Ok(d) => format!("operator notified ({d})"),
                                Err(e) => format!("notify failed: {e:#}"),
                            }
                        )),
                    ));
                }
            }
        }
    }
    for decision in decisions {
        let planned = match decision {
            LaneDecision::None => continue,
            LaneDecision::Skip {
                lane,
                subject,
                head,
                why,
            } => {
                // trace:TASK-1915 | ai:claude
                let row = ActionRow::new(&lane, &subject, "none", Outcome::Skipped, Some(why));
                tick.skip_once(per, &format!("{lane}|none|{head}"), row);
                continue;
            }
            LaneDecision::Act(p) => p,
        };
        let key = planned.key();
        if per.emitted.contains(&key) {
            continue;
        }
        let row = |outcome, why: String| {
            ActionRow::new(
                &planned.lane,
                &planned.subject,
                planned.action.as_str(),
                outcome,
                Some(why),
            )
        };
        // Skip rows are ledgered once per (item, reason), not every tick.
        // trace:TASK-1915 | ai:claude
        if let Some(why) = &tick.ended {
            let r = row(Outcome::Skipped, why.clone());
            tick.skip_once(per, &key, r);
            continue;
        }
        let now = (inp.clock)();
        if let Some(b) = per.backoff.get(&planned.backoff_key()) {
            if b.next_eligible_at.is_some_and(|at| now < at) {
                let r = row(Outcome::Skipped, "backoff".into());
                tick.skip_once(per, &key, r);
                continue;
            }
        }
        if acted >= tick.budget {
            let r = row(
                Outcome::Skipped,
                format!("budget of {} actions per tick reached", tick.budget),
            );
            tick.skip_once(per, &key, r);
            continue;
        }
        if let Some(scope) = planned.action.scope() {
            if !mandate.covers(scope) {
                let r = row(
                    Outcome::Skipped,
                    format!("not in the mandate's scope ({})", scope.as_str()),
                );
                tick.skip_once(per, &key, r);
                continue;
            }
        }
        let gate = planned
            .forced_gate
            .clone()
            .or_else(|| {
                planned
                    .action
                    .gate_op()
                    .and_then(|op| crate::mandate_gates::gate_reason(&inp.gates, op))
            })
            .or_else(|| {
                // Every spec the action touches takes the floor, not just
                // the first. trace:STORY-1700 | ai:claude
                planned
                    .spec
                    .iter()
                    .chain(planned.other_specs.iter())
                    .find_map(|s| (inp.spec_always_human)(s).map(|c| format!("{s} is {c}")))
            });
        if let Err(why) = tick.recheck() {
            let r = row(Outcome::Refused, why.clone());
            tick.row(r);
            tick.ended = Some(why);
            continue;
        }
        acted += 1;
        let (result, outcome) = match (&gate, planned.route) {
            (Some(why), _) => (
                tick.fx
                    .notify_operator(
                        &format!("AIDA drive: {} needs a human", planned.subject),
                        &format!(
                            "{}\nGated: {why}.\nNext: {}",
                            planned.detail, planned.command
                        ),
                    )
                    .map(|d| format!("gated ({why}); operator notified ({d})")),
                Outcome::Gated,
            ),
            (None, Route::Operator) => (
                tick.fx
                    .notify_operator(
                        &format!("AIDA drive: {} needs the operator", planned.subject),
                        &format!("{}\nNext: {}", planned.detail, planned.command),
                    )
                    .map(|d| format!("routed to the operator ({d})")),
                Outcome::Routed,
            ),
            (None, Route::Advisor) => (
                tick.fx
                    .emit_due(&tick.mandate_id, &planned)
                    .map(|()| "routed to the advisor seat".to_string()),
                Outcome::Routed,
            ),
        };
        match result {
            Ok(why) => {
                per.emitted.insert(key);
                per.backoff.remove(&planned.backoff_key());
                // A routed dispatch opens the spec's epoch. trace:TASK-1915 | ai:claude
                if let Some(spec) = planned.lane.strip_prefix("spec:") {
                    per.dispatch
                        .entry(spec.to_ascii_uppercase())
                        .or_default()
                        .routed_at = Some(now);
                }
                per.actions += 1;
                let r = row(outcome, why);
                tick.row(r);
            }
            Err(e) => {
                let b = per.backoff.entry(planned.backoff_key()).or_default();
                b.attempts += 1;
                b.last_at = Some(now);
                let ticks = BACKOFF_TICKS[(b.attempts as usize - 1).min(BACKOFF_TICKS.len() - 1)];
                b.next_eligible_at = Some(now + TICK_INTERVAL * ticks);
                let r = row(Outcome::Refused, format!("{e:#}"));
                tick.row(r);
            }
        }
    }
    if let Err(e) = save(&state) {
        tick.report.idle = Some(format!("could not save the drive state: {e:#}"));
    }
    drop(lock);
    tick.report
}

// ── Production wiring ───────────────────────────────────────────────────────

/// Remove the session variables from a child command and mark it a
/// schedule child (R2): the tick's children never inherit a seat.
// trace:STORY-1700 | ai:claude
pub(crate) fn scrub(cmd: &mut std::process::Command) -> &mut std::process::Command {
    for var in SESSION_ENV {
        cmd.env_remove(var);
    }
    cmd.env("AIDA_SCHEDULE_CHILD", "1")
}

/// A child command built the scrubbed way.
// trace:STORY-1700 | ai:claude
pub(crate) fn scrubbed_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    scrub(&mut cmd);
    cmd
}

/// Did this process inherit a session grant, role or purpose? Then remove
/// them from the process environment, so nothing the tick spawns (git, the
/// notify command) inherits them either.
fn scrub_own_env() -> bool {
    let mut inherited = false;
    for var in SESSION_ENV {
        if std::env::var_os(var).is_some_and(|v| !v.is_empty()) {
            inherited = true;
        }
        std::env::remove_var(var);
    }
    inherited
}

/// STORY-1706 (R6): one preview line per crashed lane, saying whether the
/// tick would restart it and, when not, why.
// trace:STORY-1706 | ai:claude
fn restart_preview(
    root: &Path,
    lanes: &[Lane],
    reqs: &[aida_core::Requirement],
    signoff: &dyn Fn(&str) -> Option<bool>,
    now: DateTime<Utc>,
) -> Vec<String> {
    let mandate = crate::mandate::mandate_dir().ok().and_then(|dir| {
        let principal = crate::current_user_id(None);
        let (valid, _) = crate::mandate::load_valid(&dir, &principal, now);
        match crate::mandate::state_at(&valid, now) {
            MandateState::Active(m) if m.is_drive() => Some(m),
            _ => None,
        }
    });
    let ledger = crate::mandate::mandate_dir()
        .ok()
        .and_then(|d| crate::mandate::read_exclusions(&d).ok());
    let always_human = spec_always_human_from(reqs, ledger.as_ref());
    let gates = crate::mandate_gates::load_human_gates(root).map_err(|e| format!("{e:#}"));
    let known = |id: &str| signoff(id).is_some();
    lanes
        .iter()
        .filter(|l| l.class == LaneClass::Crashed && l.evidence.trap_never_ran())
        .map(|lane| {
            let verdict = mandate
                .as_ref()
                .and_then(|m| restart_verdict(lane, m, root, &gates, &always_human, &known, now));
            let what = match verdict {
                None => "not: no active drive mandate".to_string(),
                Some(RestartVerdict::Restart(p)) => format!(
                    "eligible (restart {}, --max-sessions {})",
                    p.generation, p.max_sessions
                ),
                Some(RestartVerdict::Wait(why)) => format!("waiting: {why}"),
                Some(RestartVerdict::No(why)) => format!("not: {why}"),
                Some(RestartVerdict::CrashLoop(_)) => "not: crash-looping".to_string(),
            };
            format!("relay:{}  auto-restart: {what}", lane.run_id)
        })
        .collect()
}

/// STORY-1706 (R5): start a planned relay restart. The argv is the plan's
/// (record `arg=` lines plus `--max-sessions`); the environment is the
/// tick's, scrubbed of every session grant, role, purpose and agent name; no
/// terminal; its own process group, so it outlives the tick; output appended
/// to `<run dir>/relay.restart.log`.
// trace:STORY-1706 | ai:claude
pub(crate) fn spawn_restart(plan: &RestartPlan) -> Result<u32> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    let log_path = plan.run_dir.join(crate::relay_launch::RESTART_LOG);
    let log = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&log_path)
        .with_context(|| format!("cannot open {}", log_path.display()))?;
    let mut cmd = scrubbed_command(&plan.relay_sh);
    for var in RESTART_ENV_SCRUB {
        cmd.env_remove(var);
    }
    let child = cmd
        .args(&plan.argv)
        .current_dir(&plan.cwd)
        .env(crate::relay_launch::RESTART_ENV, &plan.launch_sha)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .with_context(|| format!("cannot start {}", plan.relay_sh.display()))?;
    Ok(child.id())
}

/// Beyond [`SESSION_ENV`]: an agent identity would make the restarted relay
/// look like an agent's launch, and a stale restart token is never passed on.
const RESTART_ENV_SCRUB: [&str; 3] = [
    "AIDA_AGENT_NAME",
    "AIDA_AGENT_TYPE",
    crate::relay_launch::RESTART_ENV,
];

struct LiveEffects<'a> {
    root: &'a Path,
}

impl DriveEffects for LiveEffects<'_> {
    fn emit_due(&mut self, mandate_id: &str, item: &Planned) -> Result<()> {
        emit_due_with(
            self.root,
            mandate_id,
            item,
            &crate::mailbox_store::write_message,
        )
    }

    fn notify_operator(&mut self, title: &str, body: &str) -> Result<String> {
        let outcome = crate::notify::send_direct(self.root, NOTIFY_RULE, title, body)?;
        Ok(match outcome.delivery() {
            crate::notify::DirectDelivery::NotConfigured => {
                "no [notify] command configured; recorded in the ledger only"
            }
            crate::notify::DirectDelivery::Sent => "sent",
            crate::notify::DirectDelivery::Deferred => "deferred to quiet-hours end",
            crate::notify::DirectDelivery::Suppressed => "suppressed by min_interval",
        }
        .to_string())
    }

    fn reap_dead_leases(&mut self) -> Vec<ReapRow> {
        reap_rows(crate::session_reap::reap_quiet_outcomes(self.root))
    }

    fn restart_relay(&mut self, plan: &RestartPlan) -> Result<u32> {
        spawn_restart(plan)
    }

    fn ledger(&mut self, mandate_id: &str, row: &ActionRow) {
        emit_action(self.root, mandate_id, "tick", row);
    }
}

/// Emit the advisor due item: the `MandateDriveDue` event, then the mail
/// nudge. Idempotent per key: a retry after a failed mail finds the event
/// already written and sends only the mail, so a mail failure never
/// duplicates the due item (TASK-1915 F5).
// trace:TASK-1915 | ai:claude
pub(crate) fn emit_due_with(
    root: &Path,
    mandate_id: &str,
    item: &Planned,
    write_mail: &dyn Fn(&Path, &aida_core::mailbox::Message) -> Result<()>,
) -> Result<()> {
    let key = item.key();
    let recorded = crate::events::read_all(root).iter().any(|ev| {
        matches!(&ev.kind, crate::events::EventKind::MandateDriveDue { mandate_id: m, key: k, .. }
        if m == mandate_id && *k == key)
    });
    if !recorded {
        let event = crate::events::Event::new(
            item.spec.clone(),
            "",
            crate::events::EventKind::MandateDriveDue {
                mandate_id: mandate_id.into(),
                key: key.clone(),
                lane: item.lane.clone(),
                action: item.action.as_str().into(),
                subject: item.subject.clone(),
                command: item.command.clone(),
                detail: item.detail.clone(),
            },
        );
        crate::events::emit_recorded(root, &event).map_err(anyhow::Error::msg)?;
    }
    // The mail nudge: the advisor inbox carries the exact command.
    use aida_core::mailbox::{Intent, Message, Recipient};
    let id = uuid::Uuid::new_v4().to_string();
    let msg = Message {
        subject: Some(format!("drive: {} {}", item.action.as_str(), item.subject)),
        id: id.clone(),
        thread_id: id,
        from: "aida-drive-tick".into(),
        to: Recipient::Agent(crate::mandate::MANDATE_SEAT.into()),
        timestamp: Utc::now().timestamp_millis(),
        in_reply_to: None,
        body: format!(
            "Drive mandate {mandate_id}: {}\n\nNext action:\n{}",
            item.detail, item.command
        ),
        urgent: false,
        intent: Intent::Request,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    };
    write_mail(root, &msg)
}

/// Ledger one `MandateDriveAction` row. Best-effort.
// trace:STORY-1700 | ai:claude
pub(crate) fn emit_action(root: &Path, mandate_id: &str, actor: &str, row: &ActionRow) {
    let mut event = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MandateDriveAction {
            mandate_id: mandate_id.into(),
            actor: actor.into(),
            lane: row.lane.clone(),
            subject: row.subject.clone(),
            action: row.action.clone(),
            outcome: row.outcome.as_str().into(),
            why: row.why.clone(),
        },
    );
    if actor == "session" {
        event.seat = crate::events::active_seat();
    }
    crate::events::emit(root, &event);
}

/// STORY-1700: the ship path's ledger row (`actor: session`) for a ship
/// attempted under a mandate, on success and on refusal.
// trace:STORY-1700 | ai:claude
pub(crate) fn emit_session_ship(
    root: &Path,
    mandate_id: &str,
    pr: u64,
    outcome: Outcome,
    why: Option<String>,
) {
    emit_action(
        root,
        mandate_id,
        "session",
        &ActionRow::new(
            &format!("pr:{pr}"),
            &format!("PR #{pr}"),
            "ship",
            outcome,
            why,
        ),
    );
}

/// STORY-1700: the ship path's `refused` row, under the mandate active now
/// (a refusal names no mandate id of its own).
// trace:STORY-1700 | ai:claude
pub(crate) fn ship_refused(root: &Path, pr: u64, message: &str) {
    let id = match crate::mandate::current_state() {
        MandateState::Active(m) | MandateState::Expired(m) | MandateState::Stopped(m) => m.id,
        MandateState::None => return,
    };
    emit_session_ship(root, &id, pr, Outcome::Refused, Some(message.to_string()));
}

/// Held PRs from the main checkout's merge-hold markers.
fn collect_held_prs(main_root: &Path) -> Vec<HeldPr> {
    use crate::merge_hold::HoldReasonKind as K;
    let hold_root = crate::pr_cmd::pr_ship_hold_root(main_root);
    crate::merge_hold::list_holds(&hold_root)
        .into_iter()
        .filter_map(|(pr, _)| {
            let record = crate::merge_hold::read_hold_record(&hold_root, pr)?;
            let releasable = !record.legacy
                && matches!(record.reason_kind, K::Supervision | K::Decision | K::Rework);
            let head = record.target_head_sha.clone();
            let approved_at_head = head.as_deref().is_some_and(|sha| {
                crate::review_verdict::verdicts_for_sha(main_root, sha)
                    .iter()
                    .any(|v| v.kind.approves() && !v.is_closed())
            });
            let changed_paths = match (&head, releasable && approved_at_head) {
                (Some(sha), true) => changed_paths(main_root, sha),
                _ => None,
            };
            Some(HeldPr {
                pr,
                releasable,
                head,
                spec: record.spec.clone(),
                approved_at_head,
                changed_paths,
            })
        })
        .collect()
}

/// Paths `head` changes against its merge base with `origin/main`, through a
/// scrubbed git child. `None` when git cannot say.
fn changed_paths(main_root: &Path, head: &str) -> Option<Vec<String>> {
    let mut cmd = aida_core::git_ops::host_git_command(main_root);
    scrub(&mut cmd);
    let out = cmd
        .args(["diff", "--name-only", "-z", "--no-renames"])
        .arg(format!("origin/main...{head}"))
        .arg("--")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        out.stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect(),
    )
}

/// Is an advisor seat live: a valid advisor grant whose per-turn notice ran
/// within 30 minutes?
// trace:STORY-1700 | ai:claude
pub(crate) fn advisor_live(project_root: &Path, now: DateTime<Utc>) -> bool {
    let (Ok(grants), Some(clock)) = (
        crate::seat_authority::grant_dir(),
        crate::presence::turn_clock_dir(),
    ) else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(grants) else {
        return false;
    };
    entries.flatten().any(|e| {
        let p = e.path();
        let Some(id) = p
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|_| p.extension().is_some_and(|x| x == "json"))
        else {
            return false;
        };
        crate::seat_authority::grant_authorized_at(
            project_root,
            id,
            crate::mandate::MANDATE_SEAT,
            now,
        ) && crate::presence::turn_clock_last_seen_in(&clock, &grant_turn_key(id))
            // A future-dated stamp (clock skew) is not evidence of a live
            // seat. trace:STORY-1700 | ai:claude
            .is_some_and(|seen| seen <= now && now - seen <= ADVISOR_LIVE_WITHIN)
    })
}

/// STORY-1700: the per-turn notice's stamp for the session's seat grant
/// ([`crate::seat_authority::GRANT_ENV`]), read back by [`advisor_live`].
// trace:STORY-1700 | ai:claude
pub(crate) fn stamp_grant_turn(grant: Option<&str>, now: DateTime<Utc>) {
    if let Some(g) = grant
        .map(str::trim)
        .filter(|g| uuid::Uuid::parse_str(g).is_ok())
    {
        crate::presence::stamp_turn_clock(Some(&grant_turn_key(g)), false, now);
    }
}

/// The turn-clock key the per-turn notice stamps for a seat grant.
pub(crate) fn grant_turn_key(grant_id: &str) -> String {
    format!("grant-{grant_id}")
}

fn find_spec<'a>(
    reqs: &'a [aida_core::Requirement],
    id: &str,
) -> Option<&'a aida_core::Requirement> {
    reqs.iter().find(|r| {
        r.spec_id
            .as_deref()
            .is_some_and(|x| x.eq_ignore_ascii_case(id))
            || r.agreed_id
                .as_deref()
                .is_some_and(|x| x.eq_ignore_ascii_case(id))
    })
}

/// Is `spec` always human (operator-only, decide mode), now or per the
/// sticky exclusion ledger? The ledger wins: once seen always-human, a spec
/// stays so even after its tag is removed. `ledger = None` (unreadable)
/// fails closed.
// trace:STORY-1700 | ai:claude
pub(crate) fn spec_always_human_from<'a>(
    reqs: &'a [aida_core::Requirement],
    ledger: Option<&'a BTreeMap<String, crate::mandate::Exclusion>>,
) -> impl Fn(&str) -> Option<String> + 'a {
    move |id: &str| {
        let Some(ledger) = ledger else {
            return Some("unknown (the exclusion ledger is unreadable)".into());
        };
        if let Some(e) = ledger
            .get(&id.to_ascii_uppercase())
            .filter(|e| e.always_human)
        {
            return Some(e.class.clone());
        }
        find_spec(reqs, id).and_then(crate::mandate::always_human_class)
    }
}

/// `None` when `id` is not a store spec, else whether it awaits a signoff
/// ([`awaits_signoff`]).
pub(crate) fn spec_signoff_from(
    reqs: &[aida_core::Requirement],
) -> impl Fn(&str) -> Option<bool> + '_ {
    move |id: &str| find_spec(reqs, id).map(awaits_signoff)
}

/// An approved spec and what already owns it (AC1's dispatch lane).
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnownedSpec {
    pub id: String,
    pub title: String,
    /// A live claim or spec-scoped session lease, a work-queue entry, or a
    /// live drain's spec set.
    // trace:TASK-1915 | ai:claude
    pub live_claim: bool,
    /// A local or remote branch named for the spec.
    pub branch: bool,
    /// An open PR for the spec; `None` = the forge could not say.
    pub open_pr: Option<bool>,
    /// The spec's dispatch epoch ([`DispatchEpoch`]); the tick sets it.
    pub epoch: u32,
}

impl UnownedSpec {
    fn owned(&self) -> bool {
        self.live_claim || self.branch || self.open_pr == Some(true)
    }
}

/// The routing for one approved spec: unowned (no live claim, no branch,
/// no open PR) is a dispatch for the advisor; an unknown PR state is
/// skipped once with the reason.
// trace:STORY-1700 | ai:claude
pub(crate) fn plan_unowned_spec(s: &UnownedSpec) -> LaneDecision {
    let lane = format!("spec:{}", s.id);
    if s.owned() {
        return LaneDecision::None;
    }
    // The head carries the dispatch epoch, so a re-route is a new item.
    // trace:TASK-1915 | ai:claude
    let head = match s.epoch {
        0 => "approved".to_string(),
        n => format!("approved#{n}"),
    };
    if s.open_pr.is_none() {
        return LaneDecision::Skip {
            lane,
            subject: s.id.clone(),
            head,
            why: "the forge could not say whether it has an open PR".into(),
        };
    }
    LaneDecision::Act(Planned {
        lane,
        subject: s.id.clone(),
        action: DriveAction::Redispatch,
        route: Route::Advisor,
        head,
        command: format!(
            "aida agent new claude --spec {} --bg (a separate session, never an in-session subagent)",
            s.id
        ),
        detail: format!(
            "{} is approved and unowned: no live claim, no branch, no open PR ({})",
            s.id, s.title
        ),
        spec: Some(s.id.clone()),
        forced_gate: None,
        other_specs: Vec::new(),
    })
}

/// Does a branch name carry `id` as a whole token (`claude/story-17` but
/// not `story-170`)?
fn branch_names_spec(branch: &str, id: &str) -> bool {
    let b = branch.to_ascii_lowercase();
    let id = id.to_ascii_lowercase();
    b.match_indices(&id).any(|(i, _)| {
        let before = b[..i].chars().last();
        let after = b[i + id.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_digit())
    })
}

/// Specs a work queue or the live drain already holds, upper-cased (spec
/// and agreed ids): a queue entry added within [`QUEUE_ENTRY_FRESH`] routes
/// the spec to someone, and a live drain's spec set is being worked, so
/// neither is unowned. An older queue entry is abandoned and owns nothing.
// trace:TASK-1915 | ai:claude
pub(crate) fn queue_and_drain_claims(
    root: &Path,
    reqs: &[aida_core::Requirement],
    now: DateTime<Utc>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(lock) = crate::drain_lock::read_pid_live_lock(root) {
        out.extend(lock.specs.iter().map(|s| s.to_ascii_uppercase()));
    }
    let Ok(entries) = std::fs::read_dir(root.join(".aida-store/registry/queues")) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("yaml") {
            continue;
        }
        let Some(queue) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_yaml::from_str::<Vec<aida_core::QueueEntry>>(&t).ok())
        else {
            continue;
        };
        for q in queue
            .into_iter()
            .filter(|q| now - q.added_at <= QUEUE_ENTRY_FRESH)
        {
            if let Some(r) = reqs.iter().find(|r| r.id == q.requirement_id) {
                out.extend(
                    [r.spec_id.as_deref(), r.agreed_id.as_deref()]
                        .into_iter()
                        .flatten()
                        .map(str::to_ascii_uppercase),
                );
            }
        }
    }
    out
}

/// Approved, non-epic specs from the store with their ownership facts.
fn collect_unowned_specs(root: &Path, reqs: &[aida_core::Requirement]) -> Vec<UnownedSpec> {
    let approved: Vec<&aida_core::Requirement> = reqs
        .iter()
        .filter(|r| matches!(r.status, aida_core::RequirementStatus::Approved))
        .filter(|r| !matches!(r.req_type, aida_core::RequirementType::Epic))
        .filter(|r| r.spec_id.is_some())
        .collect();
    if approved.is_empty() {
        return Vec::new();
    }
    let mut cmd = aida_core::git_ops::host_git_command(root);
    scrub(&mut cmd);
    let branches: Vec<String> = cmd
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let leases = crate::leases::list_leases(root);
    let live = crate::process_probe::probe_live_claude_sessions();
    let is_live = crate::leases::lease_is_live(&live, Utc::now());
    let ids: Vec<String> = approved.iter().filter_map(|r| r.spec_id.clone()).collect();
    let open = crate::db_reconcile::specs_with_open_prs(root, ids.clone());
    let held = queue_and_drain_claims(root, reqs, Utc::now());
    approved
        .iter()
        .filter_map(|r| {
            let id = r.spec_id.clone()?;
            let names: Vec<&str> = std::iter::once(id.as_str())
                .chain(r.agreed_id.as_deref())
                .collect();
            Some(UnownedSpec {
                live_claim: crate::leases::live_spec_claim_by_other(
                    &leases, None, &names, &is_live,
                )
                .is_some()
                    || names.iter().any(|n| held.contains(&n.to_ascii_uppercase())),
                epoch: 0,
                branch: branches
                    .iter()
                    .any(|b| names.iter().any(|n| branch_names_spec(b, n))),
                open_pr: open.as_ref().map(|m| {
                    m.keys()
                        .any(|k| names.iter().any(|n| k.eq_ignore_ascii_case(n)))
                }),
                title: r.title.clone(),
                id,
            })
        })
        .collect()
}

/// One live tick for the main checkout.
// trace:STORY-1700 | ai:claude
pub(crate) fn run_tick_live(main_root: &Path, inherited_session_env: bool) -> Result<TickReport> {
    let dir = crate::mandate::mandate_dir()?;
    let now = Utc::now();
    // The 15m job runs whether or not a mandate exists; with no drive
    // record it is a no-op that reads nothing else.
    if active_drive_records(&dir, now).is_empty() {
        return Ok(TickReport {
            idle: Some("no active drive mandate".into()),
            ..Default::default()
        });
    }
    let principal = crate::current_user_id(None);
    // Never default an unreadable store to "no specs": the tick withholds
    // every gated action instead (`store_readable`).
    let store = crate::load_store_for_lookup(main_root);
    let store_readable = store.is_some();
    let reqs: Vec<aida_core::Requirement> = store.map(|s| s.requirements).unwrap_or_default();
    let ledger = crate::mandate::read_exclusions(&dir).ok();
    let always_human = spec_always_human_from(&reqs, ledger.as_ref());
    let signoff = spec_signoff_from(&reqs);
    let inputs = TickInputs {
        project_root: main_root,
        mandate_dir: &dir,
        principal: &principal,
        clock: &Utc::now,
        gates: crate::mandate_gates::load_human_gates(main_root).map_err(|e| format!("{e:#}")),
        lanes: crate::fleet_lanes::classify_lanes(main_root, now),
        held_prs: collect_held_prs(main_root),
        advisor_live: advisor_live(main_root, now),
        inherited_session_env,
        spec_always_human: &always_human,
        spec_signoff: &signoff,
        store_readable,
        unowned_specs: collect_unowned_specs(main_root, &reqs),
    };
    Ok(run_tick(
        &inputs,
        &TickHooks::default(),
        &mut LiveEffects { root: main_root },
    ))
}

// ── Principal under the scheduler (R4) ──────────────────────────────────────

/// Which principal the installed schedule driver would resolve.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrincipalProbe {
    Resolved(String),
    NoDriver,
    Unknown(String),
}

/// An explicit `AIDA_USER=<name>` in a driver artifact (a systemd unit's
/// `Environment=` line, or the cron line).
pub(crate) fn aida_user_in(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .find_map(|tok| tok.strip_prefix("AIDA_USER="))
        .map(|v| v.trim_matches(|c| c == '"' || c == '\'').to_string())
        .filter(|v| !v.is_empty())
}

/// The passwd name of this uid: what cron and the systemd user manager put
/// in `USER`.
fn passwd_name() -> Option<String> {
    // SAFETY: getpwuid returns a pointer into static storage or null; it is
    // read immediately and never retained.
    unsafe {
        let pw = libc::getpwuid(libc::geteuid());
        if pw.is_null() || (*pw).pw_name.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*pw).pw_name)
            .to_str()
            .ok()
            .map(str::to_string)
    }
}

/// The principal the installed driver resolves, by the same order as
/// `current_user_id`: an explicit `AIDA_USER` in the driver, else the uid's
/// passwd name.
// trace:STORY-1700 | ai:claude
pub(crate) fn scheduler_principal(project_root: &Path) -> PrincipalProbe {
    let status = crate::schedule_driver::driver_status(project_root);
    if !status.any_installed() {
        return PrincipalProbe::NoDriver;
    }
    let marker = crate::maintenance_schedule::tick_cron_marker(project_root);
    let mut texts = Vec::new();
    if status.cron_installed() {
        if let Ok(Some(tab)) = crate::maintenance_schedule::read_crontab() {
            texts.extend(
                tab.lines()
                    .filter(|l| crate::maintenance_schedule::line_has_marker(l, &marker))
                    .map(str::to_string),
            );
        }
    }
    if status.systemd_installed() {
        let repo = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf())
            .display()
            .to_string();
        let dir = crate::schedule_driver::user_unit_dir_from(
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            crate::home_dir(),
        );
        if let Some(dir) = dir {
            let (service, _) = crate::schedule_driver::unit_names(&repo);
            if let Ok(text) = std::fs::read_to_string(dir.join(service)) {
                texts.push(text);
            }
        }
    }
    if let Some(user) = texts.iter().find_map(|t| aida_user_in(t)) {
        return PrincipalProbe::Resolved(user);
    }
    match passwd_name() {
        Some(name) => PrincipalProbe::Resolved(name),
        None => PrincipalProbe::Unknown("this uid has no passwd entry".into()),
    }
}

/// The warnings for a drive mandate minted by `principal`, given what the
/// driver resolves and whether a job runs the tick. Pure.
// trace:STORY-1700 | ai:claude
pub(crate) fn principal_warnings(
    probe: &PrincipalProbe,
    principal: &str,
    tick_job_enabled: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    match probe {
        PrincipalProbe::NoDriver => out.push(
            "no schedule driver is installed; the drive tick will never run (aida schedule install-systemd, or install-cron)".into(),
        ),
        PrincipalProbe::Resolved(p) if p.trim() != principal.trim() => out.push(format!(
            "the installed schedule driver resolves principal `{p}`, but this mandate belongs to `{principal}`; the drive tick will not act on it. Fix: set AIDA_USER={principal} in the driver environment"
        )),
        PrincipalProbe::Resolved(_) => {}
        PrincipalProbe::Unknown(why) => out.push(format!(
            "cannot tell which principal the schedule driver resolves ({why}); set AIDA_USER={principal} in the driver environment"
        )),
    }
    if !tick_job_enabled {
        out.push(format!(
            "no enabled schedule job runs `{JOB_COMMAND}`; add the `{JOB}` job (every = \"15m\", when = \"mandate.drive_active\")"
        ));
    }
    out
}

/// `aida mandate start --drive` warnings (R4).
// trace:STORY-1700 | ai:claude
pub(crate) fn start_warnings(project_root: &Path, principal: &str) -> Vec<String> {
    let enabled = crate::maintenance_schedule::jobs_running_command(project_root, JOB_COMMAND)
        .iter()
        .any(|(_, on)| *on);
    principal_warnings(&scheduler_principal(project_root), principal, enabled)
}

/// The `mandate-drive` doctor category: silent unless a drive mandate is
/// active and the tick would not act on it.
// trace:STORY-1700 | ai:claude
pub(crate) const DOCTOR_CATEGORY: &str = "mandate-drive";

pub(crate) fn doctor_scan(project_root: &Path) -> Vec<crate::DoctorFinding> {
    let MandateState::Active(m) = crate::mandate::current_state() else {
        return Vec::new();
    };
    if !m.is_drive() {
        return Vec::new();
    }
    start_warnings(project_root, &m.principal)
        .into_iter()
        .enumerate()
        .map(|(i, w)| crate::DoctorFinding {
            category: DOCTOR_CATEGORY.into(),
            id: format!("{DOCTOR_CATEGORY}:{}:{i}", m.id),
            summary: format!("drive mandate {} is active but {w}", m.id),
            action: "fix the schedule driver so the drive tick runs as the mandate's principal"
                .into(),
            safe_heal: false,
        })
        .collect()
}

// ── Ledger read-back ────────────────────────────────────────────────────────

/// Every `MandateDriveAction` row ledgered for `mandate_id`, oldest first:
/// `(ts, actor, row)`.
// trace:STORY-1700 | ai:claude
pub(crate) fn ledger_rows(root: &Path, mandate_id: &str) -> Vec<(String, String, ActionRow)> {
    crate::events::read_all_with_archive(root)
        .into_iter()
        .filter_map(|ev| match ev.kind {
            crate::events::EventKind::MandateDriveAction {
                mandate_id: id,
                actor,
                lane,
                subject,
                action,
                outcome,
                why,
            } if id == mandate_id => {
                let outcome = match outcome.as_str() {
                    "done" => Outcome::Done,
                    "routed" => Outcome::Routed,
                    "gated" => Outcome::Gated,
                    "refused" => Outcome::Refused,
                    _ => Outcome::Skipped,
                };
                Some((
                    ev.ts.to_rfc3339(),
                    actor,
                    ActionRow {
                        lane,
                        subject,
                        action,
                        outcome,
                        why,
                    },
                ))
            }
            _ => None,
        })
        .collect()
}

fn outcome_counts(rows: &[(String, String, ActionRow)]) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for (_, _, r) in rows {
        *counts.entry(r.outcome.as_str()).or_insert(0) += 1;
    }
    counts
}

/// `aida mandate status` text: counts by outcome and the last 10 actions.
// trace:STORY-1700 | ai:claude
pub(crate) fn ledger_text(root: &Path, mandate_id: &str) -> Option<String> {
    let rows = ledger_rows(root, mandate_id);
    if rows.is_empty() {
        return None;
    }
    let counts = outcome_counts(&rows)
        .into_iter()
        .map(|(k, v)| format!("{v} {k}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = format!(
        "  mandate-driven actions: {counts}\n  last {}:",
        rows.len().min(10)
    );
    for (ts, actor, r) in rows.iter().rev().take(10) {
        out.push_str(&format!(
            "\n    {ts} [{actor}] {} {} → {}{}",
            r.action,
            r.subject,
            r.outcome.as_str(),
            r.why
                .as_deref()
                .map(|w| format!(" ({w})"))
                .unwrap_or_default()
        ));
    }
    Some(out)
}

/// The JSON form of [`ledger_text`].
pub(crate) fn ledger_json(root: &Path, mandate_id: &str) -> serde_json::Value {
    let rows = ledger_rows(root, mandate_id);
    serde_json::json!({
        "counts": outcome_counts(&rows),
        "last": rows.iter().rev().take(10).map(|(ts, actor, r)| serde_json::json!({
            "ts": ts, "actor": actor, "lane": r.lane, "subject": r.subject,
            "action": r.action, "outcome": r.outcome.as_str(), "why": r.why,
        })).collect::<Vec<_>>(),
    })
}

/// The digest's "Mandate-driven" section: one line per mandate with
/// actions ledgered in `[since, until)`.
// trace:STORY-1700 | ai:claude
pub(crate) fn digest_lines(root: &Path, since: DateTime<Utc>, until: DateTime<Utc>) -> Vec<String> {
    let mut by_mandate: BTreeMap<String, (BTreeMap<String, usize>, BTreeMap<String, usize>)> =
        BTreeMap::new();
    for ev in crate::events::read_all_with_archive(root) {
        if ev.ts < since || ev.ts >= until {
            continue;
        }
        if let crate::events::EventKind::MandateDriveAction {
            mandate_id,
            action,
            outcome,
            ..
        } = ev.kind
        {
            let (outcomes, actions) = by_mandate.entry(mandate_id).or_default();
            *outcomes.entry(outcome).or_insert(0) += 1;
            *actions.entry(action).or_insert(0) += 1;
        }
    }
    by_mandate
        .into_iter()
        .map(|(id, (outcomes, actions))| {
            let join = |m: BTreeMap<String, usize>| {
                m.into_iter()
                    .map(|(k, v)| format!("{v} {k}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let short: String = id.chars().take(8).collect();
            format!(
                "mandate {short}: {} (by action: {})",
                join(outcomes),
                join(actions)
            )
        })
        .collect()
}

// ── CLI ─────────────────────────────────────────────────────────────────────

fn print_report(report: &TickReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    if let Some(idle) = &report.idle {
        println!("drive tick: {idle}");
    }
    if let Some(id) = &report.mandate_id {
        println!("drive mandate {id}: {} action(s)", report.rows.len());
    }
    for r in &report.rows {
        println!(
            "  {} {} → {}{}",
            r.action,
            r.subject,
            r.outcome.as_str(),
            r.why
                .as_deref()
                .map(|w| format!(" ({w})"))
                .unwrap_or_default()
        );
    }
    Ok(())
}

/// The read-only `aida fleet drive` preview. It mirrors the tick: with an
/// unreadable store it warns, and every gated seat action shows as
/// withheld rather than routed. Empty when no lane would act.
// trace:STORY-1700 | ai:claude
pub(crate) fn preview_lines(decisions: &[LaneDecision], store_readable: bool) -> Vec<String> {
    let mut out = Vec::new();
    for d in decisions {
        match d {
            LaneDecision::None => {}
            LaneDecision::Skip { subject, why, .. } => {
                out.push(format!("  {subject}: skipped ({why})"));
            }
            LaneDecision::Act(p) => {
                let route = p.route.as_str();
                let status = preview_hold(p, store_readable)
                    .map(|h| format!(" ({h})"))
                    .unwrap_or_default();
                out.push(format!(
                    "  {}: {} → {route}{status}\n    {}",
                    p.subject,
                    p.action.as_str(),
                    p.command.replace('\n', "\n    ")
                ));
            }
        }
    }
    if !store_readable {
        out.insert(
            0,
            "warning: the requirement store is unreadable; a tick would withhold every gated seat action (ship, signoff, answer, dispatch)".to_string(),
        );
    }
    out
}

/// Why the preview would not route a planned action as-is: withheld (an
/// unreadable store) or gated. `None` routes it.
// trace:STORY-1700 | ai:claude
fn preview_hold(p: &Planned, store_readable: bool) -> Option<String> {
    if !store_readable && p.action.gate_op().is_some() {
        Some("withheld: the requirement store is unreadable".to_string())
    } else {
        p.forced_gate.as_deref().map(|g| format!("gated: {g}"))
    }
}

/// The `--json` projection of the preview: the same lanes `preview_lines`
/// renders, as data. `lanes` is empty when no lane would act.
// trace:STORY-1700 | ai:claude
pub(crate) fn preview_json(decisions: &[LaneDecision], store_readable: bool) -> serde_json::Value {
    let lanes: Vec<serde_json::Value> = decisions
        .iter()
        .filter_map(|d| match d {
            LaneDecision::None => None,
            LaneDecision::Skip {
                lane, subject, why, ..
            } => Some(serde_json::json!({
                "lane": lane,
                "subject": subject,
                "skipped": why,
            })),
            LaneDecision::Act(p) => Some(serde_json::json!({
                "lane": p.lane,
                "subject": p.subject,
                "action": p.action.as_str(),
                "route": p.route.as_str(),
                "held": preview_hold(p, store_readable),
                "command": p.command,
            })),
        })
        .collect();
    serde_json::json!({ "store_readable": store_readable, "lanes": lanes })
}

/// `aida fleet drive [--tick] [--json]`.
// trace:STORY-1700 | ai:claude
pub(crate) fn handle_fleet(action: &crate::cli::FleetAction) -> Result<()> {
    match action {
        crate::cli::FleetAction::Drive { tick, json } => {
            // R2: the tick's authority never comes from the invoking session.
            let inherited = scrub_own_env();
            let root = crate::find_main_worktree_root()?;
            if *tick {
                let report = run_tick_live(&root, inherited)?;
                return print_report(&report, *json);
            }
            // Preview: what each lane would route, changing nothing.
            let now = Utc::now();
            let store = crate::load_store_for_lookup(&root);
            let store_readable = store.is_some();
            let reqs: Vec<aida_core::Requirement> =
                store.map(|s| s.requirements).unwrap_or_default();
            let signoff = spec_signoff_from(&reqs);
            let lanes = crate::fleet_lanes::classify_lanes(&root, now);
            let restart_lines = restart_preview(&root, &lanes, &reqs, &signoff, now);
            let mut decisions: Vec<LaneDecision> =
                lanes.iter().map(|l| plan_relay_lane(l, &signoff)).collect();
            decisions.extend(collect_held_prs(&root).iter().map(plan_held_pr));
            decisions.extend(
                collect_unowned_specs(&root, &reqs)
                    .iter()
                    .map(plan_unowned_spec),
            );
            if *json {
                let preview = preview_json(&decisions, store_readable);
                println!("{}", serde_json::to_string_pretty(&preview)?);
                return Ok(());
            }
            let mut lines = preview_lines(&decisions, store_readable);
            lines.extend(restart_lines);
            for line in &lines {
                println!("{line}");
            }
            if lines.is_empty() {
                println!("No stalled lane to drive.");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "tests/story_1700_drive_mandate_tests.rs"]
mod story_1700_drive_mandate_tests;

#[cfg(test)]
#[path = "tests/story_1706_relay_restart_tests.rs"]
mod story_1706_relay_restart_tests;

/// Fleet operations over relay runs and lanes.
///
/// `drive --tick` runs one drive-mandate tick: it classifies every relay
/// lane and routes each stalled one's next action to the advisor seat or
/// the operator, under an active drive mandate only. It holds no seat
/// grant and runs no model.
// trace:STORY-1700 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "fleet")]
pub(crate) struct FleetArgs {
    #[clap(subcommand)]
    pub action: crate::cli::FleetAction,
}

// STORY-1700: the drive tick reads the AIDA home and the run dirs and
// opens the store itself only for spec lookups. trace:STORY-1700 | ai:claude
fn run_fleet(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <FleetArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    handle_fleet(&args.action)
}

// trace:STORY-1728 | ai:claude
pub(crate) static FLEET: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "fleet",
        help_after: "mandate",
        command: <FleetArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_fleet),
    };

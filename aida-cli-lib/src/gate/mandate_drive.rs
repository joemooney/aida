//! STORY-1700 / STORY-1706 / STORY-1889: drive-mandate gate authority.
//!
//! Holds the drive tick's admission and routing decisions, scope and human-gate
//! mappings, always-human and sketch-signoff predicates, crashed-relay restart
//! eligibility ([`restart_verdict`]), held-PR and unowned-spec routing, and
//! inline drive-record and mid-tick recheck decisions. Scheduling loops,
//! process spawning, notifications, state persistence and CLI stay in
//! [`crate::mandate_drive`].
// trace:STORY-1700 | ai:claude
// trace:STORY-1706 | ai:claude
// trace:STORY-1889 | ai:agy
// mandate-gate

use crate::gate::fleet_lanes::{Lane, LaneClass, SeatKind};
use crate::gate::mandate::{Mandate, MandateRefusal, MandateState, Scope};
use crate::gate::mandate_gates::{GateOp, HumanGates};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_MAX_ACTIONS: usize = 10;
pub(crate) const TICK_INTERVAL: Duration = Duration::minutes(15);
/// Backoff after a failed attempt, in ticks: 1, 2, then 4.
pub(crate) const BACKOFF_TICKS: [i32; 3] = [1, 2, 4];

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

    pub(crate) fn backoff_key(&self) -> String {
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
    let ack =
        crate::gate::fleet_lanes::acknowledgement_marker(&lane.run_dir, &lane.evidence.status)
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
pub(crate) const RUN_SPEC_UNKNOWN: &str =
    "the spec this relay run was working could not be determined from its run record";

/// Every `PREFIX-123` token in `text`, in order.
pub(crate) fn spec_ids_in(text: &str) -> Vec<String> {
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

pub(crate) fn crashed_head(lane: &Lane) -> String {
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
    use crate::gate::relay_launch as rl;
    use RestartVerdict::{CrashLoop, No, Wait};
    if lane.class != LaneClass::Crashed || !lane.evidence.trap_never_ran() {
        return None;
    }
    let no = |why: String| Some(No(why));
    if !mandate.covers(Scope::RelayRestart) {
        return no("the mandate does not cover relay-restart (start a new drive mandate)".into());
    }
    if let Some(why) = crate::gate::mandate_gates::gate_reason(gates, GateOp::RelayRestart) {
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
// trace:STORY-1888 | ai:claude
pub(crate) fn plan_held_pr(
    pr: &HeldPr,
    protected: &crate::gate::mandate_gates::ProtectedPaths,
) -> LaneDecision {
    let (true, true, Some(head)) = (pr.releasable, pr.approved_at_head, pr.head.as_ref()) else {
        return LaneDecision::None;
    };
    let forced_gate = match &pr.changed_paths {
        None => Some("the files it changes could not be resolved".to_string()),
        Some(paths) => crate::gate::mandate_gates::touches_gates(paths, protected).map(|hit| {
            format!(
                "it changes {}, part of the mandate and its gates",
                hit.describe()
            )
        }),
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
            crate::gate::review_verdict::short_sha(head),
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

/// The dispatch lane's head for `epoch`: a re-route is a new item.
// trace:TASK-1955 | ai:claude (TASK-2080)
pub(crate) fn dispatch_head(epoch: u32) -> String {
    match epoch {
        0 => "approved".to_string(),
        n => format!("approved#{n}"),
    }
}

/// The [`Planned::key`] of `spec`'s dispatch under `head`.
// trace:TASK-1955 | ai:claude (TASK-2080)
pub(crate) fn dispatch_key(spec: &str, head: &str) -> String {
    format!("spec:{spec}|{}|{head}", DriveAction::Redispatch.as_str())
}

/// Is any unexpired, unstopped DRIVE mandate record present, whoever its
/// principal? The `mandate.drive_active` predicate; the tick decides
/// authority itself.
// trace:STORY-1700 | ai:claude
pub(crate) fn drive_record_active(now: DateTime<Utc>) -> bool {
    crate::gate::mandate::mandate_dir()
        .map(|dir| !active_drive_records(&dir, now).is_empty())
        .unwrap_or(false)
}

/// Every active drive record in `dir`, unfiltered by principal (the R4
/// mismatch probe). Never authority: only [`crate::gate::mandate::load_valid`] is.
pub(crate) fn active_drive_records(dir: &Path, now: DateTime<Utc>) -> Vec<Mandate> {
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

/// Resolve the active drive mandate for `principal` at `now`.
pub(crate) fn active_drive_mandate(
    mandate_dir: &Path,
    principal: &str,
    now: DateTime<Utc>,
) -> Option<Mandate> {
    let (valid, _) = crate::gate::mandate::load_valid(mandate_dir, principal, now);
    match crate::gate::mandate::state_at(&valid, now) {
        MandateState::Active(m) if m.is_drive() => Some(m),
        _ => None,
    }
}

/// R3: re-read the mandate immediately before a side effect and require it to
/// remain an active drive mandate.
pub(crate) fn recheck_drive_mandate(
    mandate_dir: &Path,
    mandate_id: &str,
    principal: &str,
    now: DateTime<Utc>,
) -> std::result::Result<(), String> {
    match crate::gate::mandate::recheck(mandate_dir, mandate_id, principal, now) {
        Ok(m) if m.is_drive() => Ok(()),
        Ok(_) => Err("mandate is no longer a drive mandate".into()),
        Err(MandateRefusal::Expired { .. }) => Err("mandate expired mid-tick".into()),
        Err(_) => Err("mandate stopped mid-tick".into()),
    }
}

/// Check whether `mandate` covers `planned`'s required scope.
/// Returns `Some(why)` when the action is outside the mandate's scope.
pub(crate) fn missing_scope_reason(mandate: &Mandate, planned: &Planned) -> Option<String> {
    let scope = planned.action.scope()?;
    (!mandate.covers(scope)).then(|| format!("not in the mandate's scope ({})", scope.as_str()))
}

/// Whether `decision` is a gated seat action that must be withheld when the
/// requirement store is unreadable.
pub(crate) fn is_store_dependent_decision(decision: &LaneDecision) -> bool {
    matches!(decision, LaneDecision::Act(p) if p.action.gate_op().is_some())
}

/// Verify that a freshly probed crashed lane produces the exact same restart
/// plan (`launch_sha`, `argv`, `crashed_head`) before appending lineage and
/// spawning.
pub(crate) fn verify_fresh_restart_plan(
    again: Option<RestartVerdict>,
    plan: &RestartPlan,
) -> std::result::Result<(), String> {
    match again {
        Some(RestartVerdict::Restart(p))
            if p.launch_sha == plan.launch_sha
                && p.argv == plan.argv
                && p.crashed_head == plan.crashed_head =>
        {
            Ok(())
        }
        _ => Err("the run changed before the restart; left for the next tick".to_string()),
    }
}

/// Held PRs from the main checkout's merge-hold markers.
pub(crate) fn collect_held_prs_with(
    main_root: &Path,
    changed_paths_fn: &dyn Fn(&Path, &str) -> Option<Vec<String>>,
) -> Vec<HeldPr> {
    use crate::gate::merge_hold_core::HoldReasonKind as K;
    let hold_root = crate::pr_cmd::pr_ship_hold_root(main_root);
    crate::gate::merge_hold_core::list_holds(&hold_root)
        .into_iter()
        .filter_map(|(pr, _)| {
            let record = crate::gate::merge_hold_core::read_hold_record(&hold_root, pr)?;
            let releasable = !record.legacy
                && matches!(record.reason_kind, K::Supervision | K::Decision | K::Rework);
            let head = record.target_head_sha.clone();
            let approved_at_head = head.as_deref().is_some_and(|sha| {
                crate::gate::review_verdict::verdicts_for_sha(main_root, sha)
                    .iter()
                    .any(|v| v.kind.approves() && !v.is_closed())
            });
            let changed_paths = match (&head, releasable && approved_at_head) {
                (Some(sha), true) => changed_paths_fn(main_root, sha),
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
    ledger: Option<&'a BTreeMap<String, crate::gate::mandate::Exclusion>>,
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
        find_spec(reqs, id).and_then(crate::gate::mandate::always_human_class)
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
    /// The spec's dispatch epoch ([`crate::mandate_drive::DispatchEpoch`]); the tick sets it.
    pub epoch: u32,
}

impl UnownedSpec {
    pub(crate) fn owned(&self) -> bool {
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
    let head = dispatch_head(s.epoch);
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

/// The human gate a planned action takes: its forced gate, the
/// `[mandate.human_gates]` op gate, then the operator-only/sticky floor of
/// every spec it touches. The tick and the preview both ask this, so the
/// preview never shows routed what the tick would gate.
// trace:STORY-1700 | ai:claude
// trace:TASK-1955 | ai:claude (TASK-1933)
pub(crate) fn planned_gate(
    planned: &Planned,
    gates: &std::result::Result<HumanGates, String>,
    spec_always_human: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    planned
        .forced_gate
        .clone()
        .or_else(|| {
            planned
                .action
                .gate_op()
                .and_then(|op| crate::gate::mandate_gates::gate_reason(gates, op))
        })
        .or_else(|| {
            // Every spec the action touches takes the floor, not just
            // the first. trace:STORY-1700 | ai:claude
            planned
                .spec
                .iter()
                .chain(planned.other_specs.iter())
                .find_map(|s| spec_always_human(s).map(|c| format!("{s} is {c}")))
        })
}

/// What the preview reads to mirror the tick's holds.
// trace:TASK-1955 | ai:claude (TASK-1933)
pub(crate) struct PreviewGates<'a> {
    pub store_readable: bool,
    pub gates: std::result::Result<HumanGates, String>,
    pub spec_always_human: &'a dyn Fn(&str) -> Option<String>,
}

/// The read-only `aida fleet drive` preview. It mirrors the tick: with an
/// unreadable store it warns, and every gated seat action shows as
/// withheld rather than routed. Empty when no lane would act.
// trace:STORY-1700 | ai:claude
pub(crate) fn preview_lines(decisions: &[LaneDecision], ctx: &PreviewGates) -> Vec<String> {
    let store_readable = ctx.store_readable;
    let mut out = Vec::new();
    for d in decisions {
        match d {
            LaneDecision::None => {}
            LaneDecision::Skip { subject, why, .. } => {
                out.push(format!("  {subject}: skipped ({why})"));
            }
            LaneDecision::Act(p) => {
                let route = p.route.as_str();
                let status = preview_hold(p, ctx)
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
/// unreadable store) or gated, exactly as the tick gates it. `None` routes it.
// trace:STORY-1700 | ai:claude
// trace:TASK-1955 | ai:claude (TASK-1933)
pub(crate) fn preview_hold(p: &Planned, ctx: &PreviewGates) -> Option<String> {
    if !ctx.store_readable && p.action.gate_op().is_some() {
        Some("withheld: the requirement store is unreadable".to_string())
    } else {
        planned_gate(p, &ctx.gates, ctx.spec_always_human).map(|g| format!("gated: {g}"))
    }
}

/// The `--json` projection of the preview: the same lanes `preview_lines`
/// renders, as data. `lanes` is empty when no lane would act.
// trace:STORY-1700 | ai:claude
pub(crate) fn preview_json(decisions: &[LaneDecision], ctx: &PreviewGates) -> serde_json::Value {
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
                "held": preview_hold(p, ctx),
                "command": p.command,
            })),
        })
        .collect();
    serde_json::json!({ "store_readable": ctx.store_readable, "lanes": lanes })
}

#[cfg(test)]
#[path = "tests/story_1700_drive_mandate_tests.rs"]
mod story_1700_drive_mandate_tests;

#[cfg(test)]
#[path = "tests/story_1706_relay_restart_tests.rs"]
mod story_1706_relay_restart_tests;

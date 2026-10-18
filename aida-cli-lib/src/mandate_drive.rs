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
//! The tick never mints, holds or forwards a grant, never ships, never
//! restarts a relay (STORY-1706) and never clears a hold. Every decision is
//! a `MandateDriveAction` row in `.aida/events.jsonl`.
// trace:STORY-1700 | ai:claude

use crate::fleet_lanes::{Lane, LaneClass, SeatKind};
use crate::mandate::{Mandate, MandateRefusal, MandateState, Scope};
use crate::mandate_gates::{GateOp, HumanGates};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
            Self::Answer | Self::FixSetup | Self::OperatorAttention => None,
        }
    }

    /// The mandate scope the action needs.
    pub(crate) fn scope(self) -> Option<Scope> {
        match self {
            Self::Ship => Some(Scope::ShipHeld),
            Self::SketchSignoff => Some(Scope::SketchSignoff),
            Self::Redispatch => Some(Scope::Dispatch),
            Self::Answer | Self::FixSetup | Self::OperatorAttention => None,
        }
    }
}

/// Who a planned action goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Advisor,
    Operator,
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
pub(crate) fn resolving_action(lane: &Lane) -> (DriveAction, String, String, Option<String>) {
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
    let spec = marker_line.as_deref().and_then(spec_id_in);
    let handoff_text = handoff
        .as_ref()
        .map(|h| h.display().to_string())
        .unwrap_or_else(|| "(no handoff recorded)".into());
    let (action, resolve) = match &spec {
        Some(spec) => (
            DriveAction::SketchSignoff,
            format!(
                "aida show {spec} --comments; then record the ruling: aida comment add {spec} \"ADVISOR SIGNOFF: ...\""
            ),
        ),
        None => (
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
    (action, command, detail, spec)
}

/// The first `PREFIX-123` spec id in `text`.
fn spec_id_in(text: &str) -> Option<String> {
    static ID: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = ID.get_or_init(|| regex::Regex::new(r"\b([A-Z][A-Z0-9]*-[0-9]+)\b").unwrap());
    re.captures(text)
        .map(|c| c[1].to_string())
        .filter(|id| !id.starts_with("BLOCKED"))
}

/// The routing for one relay lane, per the R7 classification table.
// trace:STORY-1700 | ai:claude
pub(crate) fn plan_relay_lane(lane: &Lane) -> LaneDecision {
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
    let act = |action, route, command: String, detail: String, spec| {
        LaneDecision::Act(Planned {
            lane: key_lane.clone(),
            subject: subject.clone(),
            action,
            route,
            head: head.clone(),
            command,
            detail,
            spec,
            forced_gate: None,
        })
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
            None,
        ),
        LaneClass::TerminalApi => act(
            DriveAction::OperatorAttention,
            Route::Operator,
            "restore API credits or quota, then relaunch the run".into(),
            format!(
                "relay run {} stopped on a terminal API error (exit 7); handoff: {handoff}",
                lane.run_id
            ),
            None,
        ),
        LaneClass::Blocked => {
            let (action, command, detail, spec) = resolving_action(lane);
            act(action, Route::Advisor, command, detail, spec)
        }
        LaneClass::Crashed => act(
            DriveAction::Redispatch,
            Route::Advisor,
            format!(
                "read {handoff}, then relaunch the run with bin/relay.sh against the same handoff (as a separate session, never an in-session subagent)"
            ),
            format!(
                "relay run {} crashed: its owner died without the exit trap running",
                lane.run_id
            ),
            None,
        ),
        LaneClass::SessionCap => act(
            DriveAction::Redispatch,
            Route::Advisor,
            format!(
                "dispatch a continuation: relaunch with bin/relay.sh against {handoff} (a separate session)"
            ),
            format!(
                "relay run {} reached its session cap with the task not complete",
                lane.run_id
            ),
            None,
        ),
        LaneClass::NoSlot => act(
            DriveAction::Redispatch,
            Route::Advisor,
            format!("relaunch with bin/relay.sh against {handoff} when a relay slot frees"),
            format!("relay run {} found no free relay slot (exit 8)", lane.run_id),
            None,
        ),
        LaneClass::SetupError | LaneClass::UsageBin => act(
            DriveAction::FixSetup,
            Route::Advisor,
            format!(
                "fix the relay's setup (exit {}), then relaunch against {handoff}",
                lane.evidence.field("exit").unwrap_or("?")
            ),
            format!(
                "relay run {} stopped before any session ({})",
                lane.run_id,
                lane.class.as_str()
            ),
            None,
        ),
    }
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
    fn new(lane: &str, subject: &str, action: &str, outcome: Outcome, why: Option<String>) -> Self {
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
/// notifications; tests record. None of these spawns a relay or a model.
// trace:STORY-1700 | ai:claude
pub(crate) trait DriveEffects {
    /// Emit the advisor due item (event + mail nudge).
    fn emit_due(&mut self, mandate_id: &str, item: &Planned) -> Result<()>;
    /// Tell the operator; `Ok` names the delivery.
    fn notify_operator(&mut self, title: &str, body: &str) -> Result<String>;
    /// Release session leases whose owner process is dead; the count.
    fn reap_dead_leases(&mut self) -> Result<usize>;
    /// Ledger one row.
    fn ledger(&mut self, mandate_id: &str, row: &ActionRow);
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

pub(crate) fn load_state(project_root: &Path) -> DriveState {
    std::fs::read(state_path(project_root))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
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
}

/// Test seams.
#[derive(Default)]
pub(crate) struct TickHooks<'a> {
    /// Runs before side effect `n` (0-based), before the mandate re-check.
    pub before_action: Option<&'a dyn Fn(usize)>,
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
    let mut state = load_state(inp.project_root);

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
            per.mismatch_notified = true;
            let why = format!(
                "principal mismatch: record={} tick={}",
                m.principal, inp.principal
            );
            let row = ActionRow::new(
                "tick",
                "drive tick",
                "tick",
                Outcome::Refused,
                Some(why.clone()),
            );
            fx.ledger(&m.id, &row);
            let _ = fx.notify_operator(
                "AIDA drive mandate is not acting",
                &format!(
                    "Drive mandate {} belongs to `{}`, but the drive tick resolves `{}`. Set AIDA_USER={} in the schedule driver's environment.",
                    m.id, m.principal, inp.principal, m.principal
                ),
            );
            report.mandate_id = Some(m.id.clone());
            report.rows.push(row);
        }
        let _ = save_state(inp.project_root, &state);
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

    // Substrate: dead-lease expiry.
    match tick.recheck() {
        Err(why) => tick.ended = Some(why),
        Ok(()) => match tick.fx.reap_dead_leases() {
            Ok(0) => {}
            Ok(n) => {
                per.actions += 1;
                tick.row(ActionRow::new(
                    "leases",
                    "session leases",
                    "lease-expiry",
                    Outcome::Done,
                    Some(format!("released {n} lease(s) whose owner process is dead")),
                ));
            }
            Err(e) => tick.row(ActionRow::new(
                "leases",
                "session leases",
                "lease-expiry",
                Outcome::Refused,
                Some(format!("{e:#}")),
            )),
        },
    }

    // No live advisor: one operator notice per absence episode; seat items
    // stay queued for the next advisor (R5).
    if inp.advisor_live {
        per.absence_episode = None;
    } else if per.absence_episode.is_none() && tick.ended.is_none() {
        match tick.recheck() {
            Err(why) => tick.ended = Some(why),
            Ok(()) => {
                let sent = tick.fx.notify_operator(
                    "AIDA drive mandate: no advisor seat is live",
                    "The drive tick keeps doing substrate actions; ships, sketch signoffs and dispatches wait in `aida awaiting` for the next advisor seat.",
                );
                per.absence_episode = Some(now);
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

    let mut decisions: Vec<LaneDecision> = inp.lanes.iter().map(plan_relay_lane).collect();
    decisions.extend(inp.held_prs.iter().map(plan_held_pr));
    let mut acted = 0usize;
    for decision in decisions {
        let planned = match decision {
            LaneDecision::None => continue,
            LaneDecision::Skip {
                lane,
                subject,
                head,
                why,
            } => {
                let key = format!("{lane}|skip|{head}");
                if per.emitted.insert(key) {
                    tick.row(ActionRow::new(
                        &lane,
                        &subject,
                        "none",
                        Outcome::Skipped,
                        Some(why),
                    ));
                }
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
        if let Some(why) = &tick.ended {
            let r = row(Outcome::Skipped, why.clone());
            tick.row(r);
            continue;
        }
        let now = (inp.clock)();
        if let Some(b) = per.backoff.get(&planned.backoff_key()) {
            if b.next_eligible_at.is_some_and(|at| now < at) {
                let r = row(Outcome::Skipped, "backoff".into());
                tick.row(r);
                continue;
            }
        }
        if acted >= tick.budget {
            let r = row(
                Outcome::Skipped,
                format!("budget of {} actions per tick reached", tick.budget),
            );
            tick.row(r);
            continue;
        }
        if let Some(scope) = planned.action.scope() {
            if !mandate.covers(scope) {
                let r = row(
                    Outcome::Skipped,
                    format!("not in the mandate's scope ({})", scope.as_str()),
                );
                tick.row(r);
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
                planned
                    .spec
                    .as_deref()
                    .and_then(|s| (inp.spec_always_human)(s).map(|c| format!("{s} is {c}")))
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
    if let Err(e) = save_state(inp.project_root, &state) {
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

struct LiveEffects<'a> {
    root: &'a Path,
}

impl DriveEffects for LiveEffects<'_> {
    fn emit_due(&mut self, mandate_id: &str, item: &Planned) -> Result<()> {
        let event = crate::events::Event::new(
            item.spec.clone(),
            "",
            crate::events::EventKind::MandateDriveDue {
                mandate_id: mandate_id.into(),
                key: item.key(),
                lane: item.lane.clone(),
                action: item.action.as_str().into(),
                subject: item.subject.clone(),
                command: item.command.clone(),
                detail: item.detail.clone(),
            },
        );
        crate::events::emit_recorded(self.root, &event).map_err(anyhow::Error::msg)?;
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
        crate::mailbox_store::write_message(self.root, &msg)
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

    fn reap_dead_leases(&mut self) -> Result<usize> {
        Ok(crate::session_reap::reap_quiet(self.root))
    }

    fn ledger(&mut self, mandate_id: &str, row: &ActionRow) {
        emit_action(self.root, mandate_id, "tick", row);
    }
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
            .is_some_and(|seen| now - seen <= ADVISOR_LIVE_WITHIN)
    })
}

/// The turn-clock key the per-turn notice stamps for a seat grant.
pub(crate) fn grant_turn_key(grant_id: &str) -> String {
    format!("grant-{grant_id}")
}

/// Is `spec` always human (operator-only, decide mode), now or per the
/// sticky exclusion ledger?
fn spec_always_human_live(root: &Path) -> impl Fn(&str) -> Option<String> {
    let store = crate::load_store_for_lookup(root);
    let ledger = crate::mandate::mandate_dir()
        .ok()
        .and_then(|d| crate::mandate::read_exclusions(&d).ok());
    move |id: &str| {
        if let Some(e) = ledger
            .as_ref()
            .and_then(|l| l.get(&id.to_ascii_uppercase()))
            .filter(|e| e.always_human)
        {
            return Some(e.class.clone());
        }
        store.as_ref().and_then(|s| {
            s.requirements
                .iter()
                .find(|r| {
                    r.spec_id
                        .as_deref()
                        .is_some_and(|x| x.eq_ignore_ascii_case(id))
                })
                .and_then(crate::mandate::always_human_class)
        })
    }
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
    let always_human = spec_always_human_live(main_root);
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
            let mut decisions: Vec<LaneDecision> = crate::fleet_lanes::classify_lanes(&root, now)
                .iter()
                .map(plan_relay_lane)
                .collect();
            decisions.extend(collect_held_prs(&root).iter().map(plan_held_pr));
            let mut shown = 0;
            for d in &decisions {
                match d {
                    LaneDecision::None => {}
                    LaneDecision::Skip { subject, why, .. } => {
                        shown += 1;
                        println!("  {subject}: skipped ({why})");
                    }
                    LaneDecision::Act(p) => {
                        shown += 1;
                        println!(
                            "  {}: {} → {}{}\n    {}",
                            p.subject,
                            p.action.as_str(),
                            match p.route {
                                Route::Advisor => "advisor",
                                Route::Operator => "operator",
                            },
                            p.forced_gate
                                .as_deref()
                                .map(|g| format!(" (gated: {g})"))
                                .unwrap_or_default(),
                            p.command.replace('\n', "\n    ")
                        );
                    }
                }
            }
            if shown == 0 {
                println!("No stalled lane to drive.");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "tests/story_1700_drive_mandate_tests.rs"]
mod story_1700_drive_mandate_tests;

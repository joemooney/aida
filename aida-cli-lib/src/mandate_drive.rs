//! STORY-1700 (ADR-77): the drive-mandate tick orchestration.
//!
//! Gate authority (routing decisions, scope/gate mappings, always-human and
//! sketch-signoff predicates, crashed-relay restart eligibility, held-PR and
//! unowned-spec routing, and inline drive-record/recheck checks) lives in
//! [`crate::gate::mandate_drive`].
// trace:STORY-1700 | ai:claude
// trace:STORY-1706 | ai:claude
// trace:STORY-1889 | ai:agy

use crate::gate::fleet_lanes::{Lane, LaneClass};
use crate::gate::mandate::{Mandate, MandateState};
use crate::gate::mandate_drive::{
    active_drive_mandate, active_drive_records, collect_held_prs, crashed_head, dispatch_head,
    dispatch_key, is_store_dependent_decision, missing_scope_reason, plan_held_pr, plan_relay_lane,
    plan_unowned_spec, planned_gate, preview_json, preview_lines, recheck_drive_mandate,
    record_fresh_restart, restart_verdict, spec_always_human_from, spec_signoff_from,
    tick_restart_verdict, with_live_tick_inputs, DriveAction, HeldPr, LaneDecision, Planned,
    PreviewGates, RestartPlan, RestartRefused, RestartVerdict, Route, TickInputs, UnownedSpec,
    BACKOFF_TICKS, DEFAULT_MAX_ACTIONS, TICK_INTERVAL,
};
use crate::gate::mandate_gates::HumanGates;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// trace:STORY-1728 | ai:claude
#[path = "mandate_drive_args.rs"]
pub(crate) mod mandate_drive_args;
pub(crate) use mandate_drive_args::*;

// ── Ledger rows ─────────────────────────────────────────────────────────────

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

/// The scheduled job's name (and its run ledger at `schedule/<job>.yaml`).
pub(crate) const JOB: &str = "mandate-drive";
/// The scheduled job's command.
pub(crate) const JOB_COMMAND: &str = "fleet drive --tick";
const LOCK_FILE: &str = "mandate-drive.lock";
const STATE_FILE: &str = "mandate-drive-state.json";
const ADVISOR_LIVE_WITHIN: Duration = Duration::minutes(30);
/// A routed dispatch that no owner has taken up within this window is
/// re-routed: the fixer died before it claimed (TASK-1915 F8).
const DISPATCH_STALE: Duration = Duration::hours(2);
/// How many times a dispatch nobody took up is re-routed before the drive
/// stops re-routing it (until an owner shows up): the window doubles each
/// time (2h, 4h, 8h), so an approved spec the advisor leaves unqueued on
/// purpose costs three re-routes, not one every 2h for the mandate's life.
// trace:TASK-1955 | ai:claude (TASK-2078) (TASK-1696)
const MAX_STALE_REROUTES: u32 = 3;
/// A work-queue entry older than this is abandoned, not an owner: a spec
/// left in a stale queue is dispatched again (TASK-1915 F8).
const QUEUE_ENTRY_FRESH: Duration = Duration::days(7);
/// The session variables a tick never acts on and scrubs from its children.
pub(crate) const SESSION_ENV: [&str; 3] = [
    crate::gate::seat_authority::GRANT_ENV,
    "AIDA_SESSION_ROLE",
    "AIDA_SESSION_PURPOSE",
];
const NOTIFY_RULE: &str = "mandate-drive";

// ── Effects ─────────────────────────────────────────────────────────────────

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
    /// Emit the advisor due item (event + mail nudge). `event_recorded`
    /// says the event is already written (send only the mail); the effect
    /// sets it once the event is written, whatever the mail does.
    // trace:TASK-1955 | ai:claude (TASK-2084) (TASK-1693)
    fn emit_due(
        &mut self,
        mandate_id: &str,
        item: &Planned,
        event_recorded: &mut bool,
    ) -> Result<()>;
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
    /// Due keys whose `MandateDriveDue` event is written but whose mail
    /// failed: the retry sends only the mail, without scanning the event
    /// log for the event (TASK-1915 F5).
    // trace:TASK-1955 | ai:claude (TASK-2084) (TASK-1693)
    #[serde(default)]
    pub due_recorded: BTreeSet<String>,
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
    /// Re-routes since an owner last took the spec up, each closed by
    /// [`DISPATCH_STALE`] doubling, at most [`MAX_STALE_REROUTES`].
    // trace:TASK-1955 | ai:claude (TASK-2078) (TASK-1696)
    #[serde(default)]
    pub stale_reroutes: u32,
}

impl PerMandate {
    /// `s`'s dispatch epoch now, advancing it per [`DispatchEpoch`]. An
    /// unknown PR state neither opens nor closes an epoch.
    // trace:TASK-1915 | ai:claude
    fn dispatch_epoch(&mut self, s: &UnownedSpec, now: DateTime<Utc>) -> u32 {
        let d = self.dispatch.entry(s.id.to_ascii_uppercase()).or_default();
        // A dispatch routed before epochs existed left only its emitted key:
        // date it from now, so its epoch can still close.
        // trace:TASK-1955 | ai:claude (TASK-2080) (TASK-1694)
        if d.routed_at.is_none()
            && self
                .emitted
                .contains(&dispatch_key(&s.id, &dispatch_head(d.epoch)))
        {
            d.routed_at = Some(now);
        }
        if s.owned() {
            if d.routed_at.is_some() {
                d.owned_since_route = true;
            }
        } else if s.open_pr.is_some() {
            if let Some(at) = d.routed_at {
                // An owner that came and went closes the epoch at once; a
                // dispatch nobody took up closes after a doubling window, a
                // bounded number of times. trace:TASK-1955 | ai:claude (TASK-2078) (TASK-1696)
                let stale = d.stale_reroutes < MAX_STALE_REROUTES
                    && now - at >= DISPATCH_STALE * (1i32 << d.stale_reroutes);
                if d.owned_since_route || stale {
                    d.stale_reroutes = if d.owned_since_route {
                        0
                    } else {
                        d.stale_reroutes + 1
                    };
                    d.epoch += 1;
                    d.routed_at = None;
                    d.owned_since_route = false;
                }
            }
        }
        d.epoch
    }

    /// Drop what no longer names a live item, so the state stays bounded:
    /// dispatch epochs and spec keys of specs no longer approved (or of an
    /// epoch the spec has left), and lease keys of sessions this tick's reap
    /// no longer reported. `approved` is `None` when the store could not be
    /// read, `reaped` when the reap did not run; either then prunes nothing
    /// of its kind. Relay and PR keys stay: a lane the forge or the relay
    /// dir transiently fails to list would otherwise be routed again.
    // trace:TASK-1955 | ai:claude (TASK-2080) (TASK-1694)
    fn prune(
        &mut self,
        approved: Option<&BTreeMap<String, u32>>,
        reaped: Option<&BTreeSet<String>>,
    ) {
        let keep = |key: &str| -> bool {
            let mut parts = key.split('|');
            let lane = parts.next().unwrap_or_default();
            if let (Some(spec), Some(approved)) = (lane.strip_prefix("spec:"), approved) {
                let _action = parts.next();
                let head = parts.next().unwrap_or_default();
                return approved
                    .get(&spec.to_ascii_uppercase())
                    .is_some_and(|&epoch| head == dispatch_head(epoch));
            }
            if let (Some(reaped), "leases") = (reaped, lane) {
                return parts.next().is_some_and(|subject| reaped.contains(subject));
            }
            true
        };
        self.emitted.retain(|k| keep(k));
        self.due_recorded.retain(|k| keep(k));
        if let Some(approved) = approved {
            self.dispatch.retain(|id, _| approved.contains_key(id));
        }
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
    /// Foreign-principal drive mandates already told about (R4) while the
    /// state could not record it.
    // trace:TASK-1955 | ai:claude
    #[serde(default)]
    mismatch_notified: BTreeSet<String>,
}

pub(crate) fn state_refusal_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".aida")
        .join("mandate-drive-state.refused.json")
}

fn read_state_refusal(project_root: &Path) -> Option<StateRefusal> {
    serde_json::from_slice(&std::fs::read(state_refusal_path(project_root)).ok()?).ok()
}

/// Record `refusal`; an error names the sidecar (TASK-1955, PR-2804 r2).
// trace:TASK-1955 | ai:claude
fn write_state_refusal(project_root: &Path, refusal: &StateRefusal) -> Result<()> {
    let path = state_refusal_path(project_root);
    aida_core::fs_atomic::write_atomic(&path, &serde_json::to_vec(refusal)?)
        .with_context(|| format!("cannot write {}", path.display()))
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

// ── Tick execution ──────────────────────────────────────────────────────────

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
        self.once(per, key, "skip", row);
    }

    /// Ledger `row` once per `(key, kind, reason)`: a persistent refusal
    /// (a reap that cannot remove a worktree, a failing registry GC) is one
    /// row until its reason changes, not one per 15m tick.
    // trace:TASK-1955 | ai:claude (TASK-1697)
    fn once(&mut self, per: &mut PerMandate, key: &str, kind: &str, row: ActionRow) {
        let seen = format!("{key}|{kind}|{}", row.why.as_deref().unwrap_or_default());
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
        let now = self.inp.now();
        recheck_drive_mandate(
            self.inp.mandate_dir(),
            &self.mandate_id,
            self.inp.principal(),
            now,
        )
    }
}

/// R4: a drive record for ANOTHER principal means the driver resolves the
/// wrong user. Notify (and ledger under that mandate) each one `told` does
/// not already cover; returns `(mandate id, notice delivered, row)`. Only a
/// delivered notice marks the mismatch told: a failed one retries next tick.
// trace:TASK-1915 | ai:claude
fn principal_mismatches(
    inp: &TickInputs<'_>,
    now: DateTime<Utc>,
    fx: &mut dyn DriveEffects,
    told: &dyn Fn(&str) -> bool,
) -> Vec<(String, bool, ActionRow)> {
    let mut out = Vec::new();
    for m in active_drive_records(inp.mandate_dir(), now) {
        if m.principal.trim() == inp.principal().trim() || told(&m.id) {
            continue;
        }
        let sent = fx.notify_operator(
            "AIDA drive mandate is not acting",
            &format!(
                "Drive mandate {} belongs to `{}`, but the drive tick resolves `{}`. Set AIDA_USER={} in the schedule driver's environment.",
                m.id, m.principal, inp.principal(), m.principal
            ),
        );
        let mut why = format!(
            "principal mismatch: record={} tick={}",
            m.principal,
            inp.principal()
        );
        if let Err(e) = &sent {
            why.push_str(&format!("; notify failed: {e:#}"));
        }
        let row = ActionRow::new("tick", "drive tick", "tick", Outcome::Refused, Some(why));
        fx.ledger(&m.id, &row);
        out.push((m.id.clone(), sent.is_ok(), row));
    }
    out
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
    let mut notes = BTreeMap::new();
    for lane in inp.lanes() {
        let Some(v) = tick_restart_verdict(inp, lane, mandate) else {
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
            // The lane's advisor item carries the reason, but it is routed
            // once; a restart candidate (it has a launch record) also gets
            // its own row per (lane, crashed head, reason). A relay launched
            // without a record was never restartable, so it adds no row.
            // trace:TASK-1955 | ai:claude (TASK-2207)
            RestartVerdict::No(why) => {
                let record = lane.run_dir.join(crate::gate::relay_launch::RECORD_FILE);
                if record.symlink_metadata().is_ok() {
                    let row = ActionRow::new(
                        &key,
                        &subject,
                        ACTION,
                        Outcome::Skipped,
                        Some(format!("not auto-restarted: {why}")),
                    );
                    tick.skip_once(
                        per,
                        &format!("{key}|{ACTION}|{}|no", crashed_head(lane)),
                        row,
                    );
                }
                notes.insert(key.clone(), Some(why));
                continue;
            }
            RestartVerdict::CrashLoop(sha) => {
                let why = format!(
                    "restarted {} times; it keeps crashing",
                    crate::gate::relay_launch::MAX_RESTARTS
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
                                    crate::gate::relay_launch::MAX_RESTARTS,
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
        // The gate re-judges the lane from fresh reads, then appends the
        // lineage line: no line, no exec.
        match record_fresh_restart(inp, lane, mandate, &plan) {
            Ok(()) => {}
            Err(RestartRefused::Changed(why)) => {
                refuse(tick, why);
                notes.insert(key.clone(), None);
                continue;
            }
            Err(RestartRefused::Unrecorded(why)) => {
                let why = format!("cannot record the restart, so it does not run: {why}");
                refuse(tick, why.clone());
                notes.insert(key.clone(), Some(why));
                continue;
            }
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
                        crate::gate::relay_launch::MAX_RESTARTS,
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
    let now = inp.now();
    let mandate = active_drive_mandate(inp.mandate_dir(), inp.principal(), now);

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
            let fingerprint = state_fingerprint(inp.project_root, &e);
            let known =
                read_state_refusal(inp.project_root).filter(|r| r.fingerprint == fingerprint);
            let fresh = known.is_none();
            let mut seen = known.unwrap_or(StateRefusal {
                fingerprint,
                notified: false,
                mismatch_notified: BTreeSet::new(),
            });
            // The sidecar is the only dedup while the state is unreadable:
            // when it cannot be written, every tick would ledger and notify
            // again, so the tick says why on its report and stays quiet.
            // trace:TASK-1955 | ai:claude
            if fresh {
                if let Err(werr) = write_state_refusal(inp.project_root, &seen) {
                    report.idle = Some(format!(
                        "{why}; the refusal is not ledgered or notified, because {werr:#}"
                    ));
                    drop(lock);
                    return report;
                }
            }
            if let Some(m) = &mandate {
                if fresh {
                    let row = ActionRow::new(
                        "tick",
                        "drive tick",
                        "state-check",
                        Outcome::Refused,
                        Some(why.clone()),
                    );
                    fx.ledger(&m.id, &row);
                    report.rows.push(row);
                }
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
            } else {
                // R4 still runs: the mismatch is why nothing acts, and the
                // sidecar records who was told. trace:TASK-1955 | ai:claude
                let told = seen.mismatch_notified.clone();
                for (id, delivered, row) in
                    principal_mismatches(inp, now, fx, &|id| told.contains(id))
                {
                    if delivered {
                        seen.mismatch_notified.insert(id.clone());
                    }
                    report.mandate_id = Some(id);
                    report.rows.push(row);
                }
            }
            if let Err(werr) = write_state_refusal(inp.project_root, &seen) {
                report.idle = Some(format!("{why}; {werr:#}"));
            }
            drop(lock);
            return report;
        }
    };

    let Some(mandate) = mandate else {
        let mut report = TickReport {
            idle: Some("no active drive mandate".into()),
            ..Default::default()
        };
        let told: BTreeSet<String> = state
            .mandates
            .iter()
            .filter(|(_, per)| per.mismatch_notified)
            .map(|(id, _)| id.clone())
            .collect();
        for (id, delivered, row) in principal_mismatches(inp, now, fx, &|id| told.contains(id)) {
            if delivered {
                state
                    .mandates
                    .entry(id.clone())
                    .or_default()
                    .mismatch_notified = true;
            }
            report.mandate_id = Some(id);
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
        .gates()
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
    let mut reaped: Option<BTreeSet<String>> = None;
    match tick.recheck() {
        Err(why) => tick.end("leases", "session leases", "reap", why),
        Ok(()) => {
            let seen = reaped.insert(BTreeSet::new());
            for r in tick.fx.reap_dead_leases() {
                seen.insert(r.subject.clone());
                let key = format!("leases|{}", r.subject);
                let row = ActionRow::new("leases", &r.subject, "reap", r.outcome, Some(r.why));
                match r.outcome {
                    Outcome::Skipped => tick.skip_once(per, &key, row),
                    Outcome::Done => {
                        per.actions += 1;
                        tick.row(row);
                    }
                    // trace:TASK-1955 | ai:claude (TASK-1697)
                    _ => tick.once(per, &key, "refused", row),
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
        .lanes()
        .iter()
        .map(|l| plan_relay_lane(l, inp.spec_signoff()))
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
    let protected = crate::gate::mandate_gates::ProtectedPaths::of(inp.gates());
    decisions.extend(inp.held_prs().iter().map(|pr| plan_held_pr(pr, &protected)));
    // Each unowned spec carries its dispatch epoch, so a dead fixer's spec
    // is routed again. trace:TASK-1915 | ai:claude
    let mut epochs: BTreeMap<String, u32> = BTreeMap::new();
    decisions.extend(inp.unowned_specs.iter().map(|s| {
        let mut s = s.clone();
        s.epoch = per.dispatch_epoch(&s, now);
        epochs.insert(s.id.to_ascii_uppercase(), s.epoch);
        plan_unowned_spec(&s)
    }));
    // trace:TASK-1955 | ai:claude (TASK-2080) (TASK-1694)
    per.prune(inp.store_readable().then_some(&epochs), reaped.as_ref());
    // An unreadable store: no operator-only or signoff check can be made,
    // so every gated seat action is withheld, with one ledgered refusal and
    // one operator notice per episode. trace:STORY-1700 | ai:claude
    const STORE_KEY: &str = "store|unreadable";
    if inp.store_readable() {
        per.emitted.remove(STORE_KEY);
    } else {
        let withheld = decisions
            .iter()
            .filter(|d| is_store_dependent_decision(d))
            .count();
        decisions.retain(|d| !is_store_dependent_decision(d));
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
        let now = inp.now();
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
        if let Some(why) = missing_scope_reason(&mandate, &planned) {
            let r = row(Outcome::Skipped, why);
            tick.skip_once(per, &key, r);
            continue;
        }
        let gate = planned_gate(&planned, inp.gates(), inp.spec_always_human());
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
            (None, Route::Advisor) => {
                // trace:TASK-1955 | ai:claude (TASK-2084) (TASK-1693)
                let mut recorded = per.due_recorded.contains(&key);
                let sent = tick.fx.emit_due(&tick.mandate_id, &planned, &mut recorded);
                if recorded && sent.is_err() {
                    per.due_recorded.insert(key.clone());
                } else {
                    per.due_recorded.remove(&key);
                }
                (
                    sent.map(|()| "routed to the advisor seat".to_string()),
                    Outcome::Routed,
                )
            }
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
pub(crate) fn restart_preview(
    root: &Path,
    lanes: &[Lane],
    reqs: &[aida_core::Requirement],
    signoff: &dyn Fn(&str) -> Option<bool>,
    now: DateTime<Utc>,
) -> Vec<String> {
    let mandate = crate::gate::mandate::mandate_dir().ok().and_then(|dir| {
        let principal = crate::current_user_id(None);
        active_drive_mandate(&dir, &principal, now)
    });
    let ledger = crate::gate::mandate::mandate_dir()
        .ok()
        .and_then(|d| crate::gate::mandate::read_exclusions(&d).ok());
    let always_human = spec_always_human_from(reqs, ledger.as_ref());
    let gates = crate::gate::mandate_gates::load_human_gates(root).map_err(|e| format!("{e:#}"));
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
    let log_path = plan.run_dir.join(crate::gate::relay_launch::RESTART_LOG);
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
        .env(crate::gate::relay_launch::RESTART_ENV, &plan.launch_sha)
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
/// A restarted relay is a long-lived worker, not a schedule child, so the
/// marker `scrubbed_command` sets is removed again (TASK-2206).
// trace:TASK-1955 | ai:claude (TASK-2206)
const RESTART_ENV_SCRUB: [&str; 4] = [
    "AIDA_AGENT_NAME",
    "AIDA_AGENT_TYPE",
    "AIDA_SCHEDULE_CHILD",
    crate::gate::relay_launch::RESTART_ENV,
];

struct LiveEffects<'a> {
    root: &'a Path,
}

impl DriveEffects for LiveEffects<'_> {
    fn emit_due(
        &mut self,
        mandate_id: &str,
        item: &Planned,
        event_recorded: &mut bool,
    ) -> Result<()> {
        emit_due_with(
            self.root,
            mandate_id,
            item,
            event_recorded,
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
/// nudge. Idempotent per key: a retry after a failed mail passes
/// `event_recorded` (the tick keeps it in its state) and sends only the
/// mail, so a mail failure never duplicates the due item (TASK-1915 F5).
/// Nothing scans the event log to find out (TASK-2084).
// trace:TASK-1915 | ai:claude
// trace:TASK-1955 | ai:claude (TASK-2084) (TASK-1693)
pub(crate) fn emit_due_with(
    root: &Path,
    mandate_id: &str,
    item: &Planned,
    event_recorded: &mut bool,
    write_mail: &dyn Fn(&Path, &aida_core::mailbox::Message) -> Result<()>,
) -> Result<()> {
    let key = item.key();
    if !*event_recorded {
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
        *event_recorded = true;
    }
    // The mail nudge: the advisor inbox carries the exact command.
    use aida_core::mailbox::{Intent, Message, Recipient};
    let id = uuid::Uuid::new_v4().to_string();
    let msg = Message {
        subject: Some(format!("drive: {} {}", item.action.as_str(), item.subject)),
        id: id.clone(),
        thread_id: id,
        from: "aida-drive-tick".into(),
        to: Recipient::Agent(crate::gate::mandate::MANDATE_SEAT.into()),
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
    let id = match crate::gate::mandate::current_state() {
        MandateState::Active(m) | MandateState::Expired(m) | MandateState::Stopped(m) => m.id,
        MandateState::None => return,
    };
    emit_session_ship(root, &id, pr, Outcome::Refused, Some(message.to_string()));
}

/// Is an advisor seat live: a valid advisor grant whose per-turn notice ran
/// within 30 minutes? BUG-1934: from a process the grant is bound to, as the
/// stamp recorded.
// trace:STORY-1700 | ai:claude
// trace:BUG-1934 | ai:claude
pub(crate) fn advisor_live(project_root: &Path, now: DateTime<Utc>) -> bool {
    let (Ok(grants), Some(clock)) = (
        crate::gate::seat_authority::grant_dir(),
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
        let key = grant_turn_key(id);
        crate::gate::seat_authority::grant_authorized_at(
            project_root,
            id,
            crate::gate::mandate::MANDATE_SEAT,
            now,
            &crate::presence::turn_clock_binding_in(&clock, &key),
        ) && crate::presence::turn_clock_last_seen_in(&clock, &key)
            // A future-dated stamp (clock skew) is not evidence of a live
            // seat. trace:STORY-1700 | ai:claude
            .is_some_and(|seen| seen <= now && now - seen <= ADVISOR_LIVE_WITHIN)
    })
}

/// STORY-1700: the per-turn notice's stamp for the session's seat grant
/// ([`crate::gate::seat_authority::GRANT_ENV`]), read back by [`advisor_live`].
/// BUG-1934: the stamp carries this process's tokens that satisfy the
/// grant's binding; none (an unbound grant, a foreign presenter) stamps
/// nothing, so it can neither make the seat live nor end a live one.
// trace:STORY-1700 | ai:claude
// trace:BUG-1934 | ai:claude
pub(crate) fn stamp_grant_turn(grant: Option<&str>, now: DateTime<Utc>) {
    if let Some(g) = grant
        .map(str::trim)
        .filter(|g| uuid::Uuid::parse_str(g).is_ok())
    {
        let binding = crate::gate::seat_authority::read_grant(g)
            .map(|record| {
                crate::gate::seat_authority::matched_binding(
                    &record,
                    &crate::gate::seat_authority::presenter_tokens_for(&record),
                )
            })
            .unwrap_or_default();
        // TASK-2210: a presenter the grant is not bound to (a daemon lane
        // that inherited a leaked grant id) matches nothing. Its stamp would
        // overwrite the bound session's with an empty binding and flap
        // advisor liveness, so it stamps nothing.
        // trace:TASK-1955 | ai:claude (TASK-2210)
        if binding.is_empty() {
            return;
        }
        crate::presence::stamp_bound_turn_clock(&grant_turn_key(g), binding, now);
    }
}

/// The turn-clock key the per-turn notice stamps for a seat grant.
pub(crate) fn grant_turn_key(grant_id: &str) -> String {
    format!("grant-{grant_id}")
}

/// Does a branch name carry `id` as a whole token (`claude/story-17` but
/// not `story-170`)?
pub(crate) fn branch_names_spec(branch: &str, id: &str) -> bool {
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

/// One live tick for the main checkout. The gate assembles its authority
/// inputs ([`with_live_tick_inputs`]).
// trace:STORY-1700 | ai:claude
// trace:STORY-1889 | ai:claude
pub(crate) fn run_tick_live(main_root: &Path, inherited_session_env: bool) -> Result<TickReport> {
    // The 15m job runs whether or not a mandate exists; with no drive
    // record it is a no-op that reads nothing else.
    let report = with_live_tick_inputs(
        main_root,
        inherited_session_env,
        |now| advisor_live(main_root, now),
        |reqs| collect_unowned_specs(main_root, reqs),
        |inputs| {
            run_tick(
                inputs,
                &TickHooks::default(),
                &mut LiveEffects { root: main_root },
            )
        },
    )?;
    Ok(report.unwrap_or_else(|| TickReport {
        idle: Some("no active drive mandate".into()),
        ..Default::default()
    }))
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
    let MandateState::Active(m) = crate::gate::mandate::current_state() else {
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
            let store = crate::load_store_for_lookup(&root);
            let store_readable = store.is_some();
            let reqs: Vec<aida_core::Requirement> =
                store.map(|s| s.requirements).unwrap_or_default();
            let signoff = spec_signoff_from(&reqs);
            // The tick's own gate inputs, so the preview holds what it holds.
            // trace:TASK-1955 | ai:claude (TASK-1933)
            let ledger = crate::gate::mandate::mandate_dir()
                .ok()
                .and_then(|dir| crate::gate::mandate::read_exclusions(&dir).ok());
            let always_human = spec_always_human_from(&reqs, ledger.as_ref());
            let ctx = PreviewGates {
                store_readable,
                gates: crate::gate::mandate_gates::load_human_gates(&root)
                    .map_err(|e| format!("{e:#}")),
                spec_always_human: &always_human,
            };
            // Keep restart eligibility alongside the preview's current gate inputs.
            // trace:STORY-1706 | ai:codex
            let lanes = crate::gate::fleet_lanes::classify_lanes(&root, now);
            let restart_lines = restart_preview(&root, &lanes, &reqs, &signoff, now);
            let mut decisions: Vec<LaneDecision> =
                lanes.iter().map(|l| plan_relay_lane(l, &signoff)).collect();
            let protected = crate::gate::mandate_gates::ProtectedPaths::of(&ctx.gates);
            decisions.extend(
                collect_held_prs(&root)
                    .iter()
                    .map(|pr| plan_held_pr(pr, &protected)),
            );
            decisions.extend(
                collect_unowned_specs(&root, &reqs)
                    .iter()
                    .map(plan_unowned_spec),
            );
            if *json {
                let preview = preview_json(&decisions, &ctx);
                println!("{}", serde_json::to_string_pretty(&preview)?);
                return Ok(());
            }
            let mut lines = preview_lines(&decisions, &ctx);
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

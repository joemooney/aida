//! Single-occupancy seats (STORY-1485 slice A, TASK-1607).
//!
//! One live session at a time may drive the orchestrator loop in a repo. The
//! seat's state lives in one record per seat under the main worktree's
//! `.aida/seat-occupancy/` (see [`store`]); this module is the pure state
//! machine over that record, so every race and refusal is testable with an
//! injected clock and process probe.
//!
//! Decisions this module implements:
//! - ADR-67: the holder is a validated grant session plus an *anchor* process
//!   (nearest claude/codex harness ancestor, else the invoking parent shell or
//!   loop process) plus that process's start identity.
//! - ADR-68: a caller whose validated grant is for a single-occupancy seat
//!   claims it implicitly on its first protected op (an amendment to the
//!   signed-off sketch, which required an explicit claim). A former holder
//!   never claims implicitly (advisor condition C4).
//! - ADR-69: Unknown process evidence refuses; nothing falls back to env
//!   labels.
//! - ADR-70: protected ops commit while the caller holds the seat lock; only
//!   committed child spawns and the takeover request outlive the lock.
//! - C1 (advisor condition, recorded on TASK-1607): a drain-spawned child is
//!   classified by lineage. Its own-scope ops pass; it cannot start a loop,
//!   route other specs or spawn peers on its parent's occupancy.
//!
//! `--kill` (hard takeover) is NOT delivered here: every kill request refuses
//! as scaffolding until STORY-1485 slice B (TASK-1608).
//!
//! trace:TASK-1607 trace:ADR-67 trace:ADR-68 trace:ADR-69 trace:ADR-70 | ai:claude

pub(crate) mod cmd;
pub(crate) mod config;
pub(crate) mod runtime;
pub(crate) mod store;

use aida_core::process_probe::{Probe, ProcFacts, ProcessIdentity};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const ORCHESTRATOR: &str = "orchestrator";

/// Refusal text for any `--kill` request until slice B ships.
pub(crate) const KILL_SCAFFOLDING: &str = "hard takeover (--kill) is not available yet: this build only supports graceful takeover. Ask the holder to release the seat, or wait for it to exit";

// ---------------------------------------------------------------------------
// Record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct SeatRecord {
    pub schema: u32,
    pub seat: String,
    /// Increases on every acquisition, release and transfer.
    pub generation: u64,
    pub holder: Option<Holder>,
    pub request: Option<TakeoverRequest>,
    /// Children spawned under the seat and still running. Listed as in-flight
    /// work at takeover; never killed or re-dispatched by a transfer.
    #[serde(default)]
    pub children: Vec<ChildSpawn>,
    /// Former holders' processes that are still alive. They stay fenced from
    /// protected ops, including implicit claims, until they die.
    #[serde(default)]
    pub tombstones: Vec<Tombstone>,
    /// Transition events not yet projected into history.
    #[serde(default)]
    pub outbox: Vec<SeatEvent>,
}

impl SeatRecord {
    pub fn empty(seat: &str) -> Self {
        SeatRecord {
            schema: SCHEMA_VERSION,
            seat: seat.to_string(),
            generation: 0,
            holder: None,
            request: None,
            children: Vec::new(),
            tombstones: Vec::new(),
            outbox: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Holder {
    /// `SeatGrant.session_id` of the validated grant that claimed the seat.
    pub grant_session: String,
    pub grant_id: String,
    /// The holder's anchor process (ADR-67).
    pub anchor: ProcessIdentity,
    /// Human label for the anchor (`claude`, `codex`, `bash`, ...).
    pub anchor_command: String,
    /// Loop processes started by the holder. They act as the holder and keep
    /// the seat alive if the anchor exits first (a detached drain).
    #[serde(default)]
    pub bound: Vec<ProcessIdentity>,
    pub since: DateTime<Utc>,
    pub how: ClaimKind,
}

impl Holder {
    fn processes(&self) -> impl Iterator<Item = &ProcessIdentity> {
        std::iter::once(&self.anchor).chain(self.bound.iter())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ClaimKind {
    Explicit,
    Implicit,
    Takeover,
    DeadReplacement,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct TakeoverRequest {
    pub id: String,
    pub requester_session: String,
    pub requester_grant_id: String,
    pub requester_anchor: ProcessIdentity,
    pub requester_command: String,
    pub expected_holder_session: String,
    pub expected_generation: u64,
    pub created_at: DateTime<Utc>,
    /// Absolute; resuming a request never extends it.
    pub deadline: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ChildSpawn {
    pub identity: ProcessIdentity,
    pub spec: Option<String>,
    pub generation: u64,
    pub what: String,
    pub spawned_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Tombstone {
    pub process: ProcessIdentity,
    pub grant_session: String,
    pub generation: u64,
    pub demoted_at: DateTime<Utc>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct SeatEvent {
    /// Idempotency key for history projection.
    pub op_id: String,
    pub at: DateTime<Utc>,
    pub seat: String,
    pub kind: SeatEventKind,
    pub from_session: Option<String>,
    pub to_session: Option<String>,
    pub generation_from: u64,
    pub generation_to: u64,
    pub request_id: Option<String>,
    /// In-flight scopes / child processes at the time of the event.
    #[serde(default)]
    pub in_flight: Vec<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SeatEventKind {
    Claimed,
    Released,
    TakeoverRequested,
    TakeoverAcknowledged,
    TakeoverTimedOut,
    TakeoverCancelled,
    Transferred,
    DeadHolderReplaced,
}

impl SeatEventKind {
    pub fn label(self) -> &'static str {
        match self {
            SeatEventKind::Claimed => "claimed",
            SeatEventKind::Released => "released",
            SeatEventKind::TakeoverRequested => "takeover requested",
            SeatEventKind::TakeoverAcknowledged => "takeover acknowledged",
            SeatEventKind::TakeoverTimedOut => "takeover timed out",
            SeatEventKind::TakeoverCancelled => "takeover cancelled",
            SeatEventKind::Transferred => "transferred",
            SeatEventKind::DeadHolderReplaced => "dead holder replaced",
        }
    }
}

// ---------------------------------------------------------------------------
// Caller
// ---------------------------------------------------------------------------

/// The validated grant the caller carries, if any (STORY-1473 resolver).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GrantRef {
    pub id: String,
    pub session_id: String,
    pub seat: String,
    /// Issued by `issue_child` (a delegated worker), not at a human TTY.
    pub is_child: bool,
}

/// Everything the state machine knows about whoever is calling.
#[derive(Debug, Clone)]
pub(crate) struct Caller {
    /// The calling process and its ancestors, nearest first, or why the walk
    /// failed.
    pub chain: Result<Vec<ProcFacts>, String>,
    pub grant: Option<GrantRef>,
    /// The caller's registered session-lease scope (C1 fallback for a child
    /// with no recorded spawn).
    pub lease_scope: Option<String>,
}

/// How the caller relates to the seat (C1 classification, recorded on
/// TASK-1607). Precedence is the variant order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Relation {
    /// The caller's own lineage (self up to its anchor) is the holder's.
    Holder,
    /// The caller's own lineage is a former holder that is still alive.
    Demoted(Tombstone),
    /// A descendant of holder/former-holder lineage with its own anchor.
    Child {
        own_spec: Option<String>,
    },
    /// Carries a validated, non-child grant for this seat.
    SeatGrant,
    Ordinary,
}

/// The caller's anchor: nearest harness above the caller, else its parent.
fn anchor_index(chain: &[ProcFacts]) -> Option<usize> {
    chain
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, f)| f.harness().is_some())
        .map(|(i, _)| i)
        .or(if chain.len() >= 2 { Some(1) } else { None })
}

pub(crate) struct ResolvedCaller<'a> {
    pub chain: &'a [ProcFacts],
    pub anchor: &'a ProcFacts,
    anchor_idx: usize,
}

impl<'a> ResolvedCaller<'a> {
    pub fn resolve(caller: &'a Caller) -> Result<Self, String> {
        let chain = caller
            .chain
            .as_ref()
            .map_err(|why| format!("seat occupancy cannot identify the calling process: {why}"))?;
        let anchor_idx = anchor_index(chain).ok_or_else(|| {
            "seat occupancy cannot identify the calling process: it has no parent process"
                .to_string()
        })?;
        Ok(ResolvedCaller {
            chain,
            anchor: &chain[anchor_idx],
            anchor_idx,
        })
    }

    /// Self up to and including the anchor.
    fn own(&self) -> &'a [ProcFacts] {
        &self.chain[..=self.anchor_idx]
    }

    /// Everything above the anchor.
    fn above(&self) -> &'a [ProcFacts] {
        &self.chain[self.anchor_idx + 1..]
    }

    pub fn self_process(&self) -> &'a ProcessIdentity {
        &self.chain[0].identity
    }
}

fn contains(facts: &[ProcFacts], id: &ProcessIdentity) -> bool {
    facts.iter().any(|f| &f.identity == id)
}

pub(crate) fn classify(rec: &SeatRecord, caller: &Caller, rc: &ResolvedCaller) -> Relation {
    if let Some(h) = &rec.holder {
        if h.processes().any(|p| contains(rc.own(), p)) {
            return Relation::Holder;
        }
    }
    if let Some(t) = rec
        .tombstones
        .iter()
        .find(|t| contains(rc.own(), &t.process))
    {
        return Relation::Demoted(t.clone());
    }
    let parent_lineage = rec
        .holder
        .iter()
        .flat_map(|h| h.processes())
        .chain(rec.tombstones.iter().map(|t| &t.process))
        .any(|p| contains(rc.above(), p));
    if parent_lineage {
        let own_spec = rec
            .children
            .iter()
            .find(|c| contains(rc.chain, &c.identity))
            .and_then(|c| c.spec.clone())
            .or_else(|| caller.lease_scope.clone());
        return Relation::Child { own_spec };
    }
    match &caller.grant {
        Some(g) if !g.is_child && g.seat == rec.seat => Relation::SeatGrant,
        _ => Relation::Ordinary,
    }
}

// ---------------------------------------------------------------------------
// Protected operations
// ---------------------------------------------------------------------------

/// A mutation the seat may fence.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SeatOp {
    /// Start an orchestrator loop (burndown run, queue work drive, ...).
    LoopStart { what: String },
    /// Spawn an agent child for `spec`.
    Spawn { spec: Option<String>, what: String },
    /// A queue/routing mutation touching `spec` (None = several or unknown).
    Route { spec: Option<String>, what: String },
    /// `aida session handoff --seat <seat> --write`.
    HandoffWrite,
}

impl SeatOp {
    pub fn describe(&self) -> String {
        match self {
            SeatOp::LoopStart { what }
            | SeatOp::Spawn { what, .. }
            | SeatOp::Route { what, .. } => what.clone(),
            SeatOp::HandoffWrite => "write the seat handoff".to_string(),
        }
    }

    fn spec(&self) -> Option<&str> {
        match self {
            SeatOp::Spawn { spec, .. } | SeatOp::Route { spec, .. } => spec.as_deref(),
            _ => None,
        }
    }
}

/// Facts the state machine needs from outside: the time and a process probe.
pub(crate) struct Ctx<'a> {
    pub now: DateTime<Utc>,
    pub probe: &'a dyn Fn(&ProcessIdentity) -> Probe,
}

/// Holder liveness: Alive if any holder process is alive, Dead only if every
/// one is provably dead, otherwise Unknown.
pub(crate) fn holder_liveness(h: &Holder, ctx: &Ctx) -> Probe {
    let mut unknown = None;
    let mut dead = None;
    for p in h.processes() {
        match (ctx.probe)(p) {
            Probe::Alive => return Probe::Alive,
            Probe::Unknown(r) => unknown = unknown.or(Some(r)),
            Probe::Dead(r) => dead = dead.or(Some(r)),
        }
    }
    match unknown {
        Some(r) => Probe::Unknown(r),
        None => Probe::Dead(dead.unwrap_or_else(|| "no holder process".into())),
    }
}

/// Drop tombstones and child records whose processes are provably dead.
/// Unknown entries stay: they might still be running.
pub(crate) fn prune(rec: &mut SeatRecord, ctx: &Ctx) {
    rec.tombstones
        .retain(|t| !matches!((ctx.probe)(&t.process), Probe::Dead(_)));
    rec.children
        .retain(|c| !matches!((ctx.probe)(&c.identity), Probe::Dead(_)));
}

/// What a refusal shows (advisor condition C3): who holds the seat and how to
/// release or take it over.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Refusal {
    pub reason: String,
    pub holder: Option<HolderView>,
    pub hints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct HolderView {
    pub seat: String,
    pub session: String,
    pub anchor_pid: u32,
    pub anchor_command: String,
    pub since: DateTime<Utc>,
    pub generation: u64,
    pub liveness: String,
    pub liveness_reason: Option<String>,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)?;
        if let Some(h) = &self.holder {
            write!(
                f,
                "\n  holder: session {} ({} pid {}), since {}, generation {}, {}",
                h.session,
                h.anchor_command,
                h.anchor_pid,
                h.since.format("%Y-%m-%d %H:%M:%SZ"),
                h.generation,
                h.liveness
            )?;
            if let Some(r) = &h.liveness_reason {
                write!(f, " ({r})")?;
            }
        }
        for hint in &self.hints {
            write!(f, "\n  {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Refusal {}

pub(crate) fn holder_view(rec: &SeatRecord, ctx: &Ctx) -> Option<HolderView> {
    rec.holder.as_ref().map(|h| {
        let live = holder_liveness(h, ctx);
        HolderView {
            seat: rec.seat.clone(),
            session: h.grant_session.clone(),
            anchor_pid: h.anchor.pid,
            anchor_command: h.anchor_command.clone(),
            since: h.since,
            generation: rec.generation,
            liveness: live.label().to_string(),
            liveness_reason: live.reason().map(str::to_string),
        }
    })
}

fn release_hint(rec: &SeatRecord) -> String {
    format!(
        "the holder releases with: aida session seat release --seat {} --generation {}",
        rec.seat, rec.generation
    )
}

fn takeover_hint(rec: &SeatRecord) -> String {
    format!(
        "or request a graceful takeover: aida session seat takeover --seat {} --force",
        rec.seat
    )
}

fn refuse(rec: &SeatRecord, ctx: &Ctx, reason: impl Into<String>, takeover: bool) -> Refusal {
    let mut hints = Vec::new();
    if rec.holder.is_some() {
        hints.push(release_hint(rec));
        if takeover {
            hints.push(takeover_hint(rec));
        }
    }
    Refusal {
        reason: reason.into(),
        holder: holder_view(rec, ctx),
        hints,
    }
}

/// Printed whenever a claim happens implicitly (advisor condition C2).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ClaimNotice {
    pub seat: String,
    pub generation: u64,
    pub anchor_pid: u32,
    pub anchor_command: String,
    pub how: ClaimKind,
}

impl std::fmt::Display for ClaimNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let how = match self.how {
            ClaimKind::DeadReplacement => "claimed (previous holder had exited)",
            _ => "claimed",
        };
        write!(
            f,
            "seat {} {how}: generation {}, held by {} pid {}. Release with: aida session seat release --seat {} --generation {}",
            self.seat,
            self.generation,
            self.anchor_command,
            self.anchor_pid,
            self.seat,
            self.generation
        )
    }
}

/// The result of gating one protected op.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Gate {
    /// Not fenced (ordinary work, or a child's own scope).
    Pass,
    /// The caller is (now) the holder; `claimed` is set when this call claimed.
    Holder { claimed: Option<ClaimNotice> },
}

fn new_event(
    rec: &SeatRecord,
    ctx: &Ctx,
    kind: SeatEventKind,
    from: Option<&str>,
    to: Option<&str>,
    generation_from: u64,
) -> SeatEvent {
    SeatEvent {
        op_id: Uuid::new_v4().to_string(),
        at: ctx.now,
        seat: rec.seat.clone(),
        kind,
        from_session: from.map(str::to_string),
        to_session: to.map(str::to_string),
        generation_from,
        generation_to: rec.generation,
        request_id: rec.request.as_ref().map(|r| r.id.clone()),
        in_flight: in_flight(rec),
        detail: None,
    }
}

pub(crate) fn in_flight(rec: &SeatRecord) -> Vec<String> {
    rec.children
        .iter()
        .map(|c| match &c.spec {
            Some(spec) => format!("{spec} ({} pid {})", c.what, c.identity.pid),
            None => format!("{} pid {}", c.what, c.identity.pid),
        })
        .collect()
}

fn grant_for_claim<'g>(
    caller: &'g Caller,
    op: Option<&SeatOp>,
    seat: &str,
) -> Result<&'g GrantRef, String> {
    let grant = caller.grant.as_ref().ok_or_else(|| {
        format!("claiming the {seat} seat needs a validated role grant: run `aida role enter {seat}` from your terminal")
    })?;
    if grant.is_child {
        return Err(format!(
            "a delegated (child) grant cannot claim the {seat} seat: delegated workers cannot start loops or route work on their own authority"
        ));
    }
    // Any validated direct grant may start a loop (the sketch: every loop
    // start requires occupancy regardless of label); other protected ops
    // claim only with a grant for this seat (ADR-68).
    let loop_start = matches!(op, Some(SeatOp::LoopStart { .. }));
    if !loop_start && grant.seat != seat {
        return Err(format!(
            "the active `{}` grant cannot claim the {seat} seat: run `aida role enter {seat}` from your terminal",
            grant.seat
        ));
    }
    Ok(grant)
}

/// Install `caller` as the holder. Precondition: the seat is empty.
fn install(
    rec: &mut SeatRecord,
    ctx: &Ctx,
    grant: &GrantRef,
    rc: &ResolvedCaller,
    how: ClaimKind,
    from: Option<String>,
) -> ClaimNotice {
    let generation_from = rec.generation;
    rec.generation += 1;
    rec.holder = Some(Holder {
        grant_session: grant.session_id.clone(),
        grant_id: grant.id.clone(),
        anchor: rc.anchor.identity.clone(),
        anchor_command: rc.anchor.command_label(),
        bound: Vec::new(),
        since: ctx.now,
        how,
    });
    // A tombstoned process that explicitly reclaims is no longer demoted.
    let own: Vec<ProcessIdentity> = rc.own().iter().map(|f| f.identity.clone()).collect();
    rec.tombstones.retain(|t| !own.contains(&t.process));
    let kind = if how == ClaimKind::DeadReplacement {
        SeatEventKind::DeadHolderReplaced
    } else {
        SeatEventKind::Claimed
    };
    let mut ev = new_event(
        rec,
        ctx,
        kind,
        from.as_deref(),
        Some(&grant.session_id),
        generation_from,
    );
    ev.detail = Some(format!("{how:?}").to_lowercase());
    rec.outbox.push(ev);
    ClaimNotice {
        seat: rec.seat.clone(),
        generation: rec.generation,
        anchor_pid: rc.anchor.identity.pid,
        anchor_command: rc.anchor.command_label(),
        how,
    }
}

/// Move the current holder's processes to tombstones (they stay fenced while
/// alive) and empty the seat.
fn demote_holder(rec: &mut SeatRecord, ctx: &Ctx, reason: &str) -> Option<Holder> {
    let old = rec.holder.take()?;
    for p in old.processes() {
        if !matches!((ctx.probe)(p), Probe::Dead(_)) {
            rec.tombstones.push(Tombstone {
                process: p.clone(),
                grant_session: old.grant_session.clone(),
                generation: rec.generation,
                demoted_at: ctx.now,
                reason: reason.to_string(),
            });
        }
    }
    Some(old)
}

/// Try to make the caller the holder of an empty or provably dead seat.
fn try_claim(
    rec: &mut SeatRecord,
    ctx: &Ctx,
    grant: &GrantRef,
    rc: &ResolvedCaller,
    how: ClaimKind,
) -> Result<ClaimNotice, Refusal> {
    match rec.holder.as_ref().map(|h| holder_liveness(h, ctx)) {
        None => Ok(install(rec, ctx, grant, rc, how, None)),
        Some(Probe::Dead(_)) => {
            let old = demote_holder(rec, ctx, "holder process exited");
            // A request against a dead holder can never be acknowledged.
            rec.request = None;
            Ok(install(
                rec,
                ctx,
                grant,
                rc,
                ClaimKind::DeadReplacement,
                old.map(|h| h.grant_session),
            ))
        }
        Some(Probe::Alive) => Err(refuse(
            rec,
            ctx,
            format!("the {} seat is held by another live session", rec.seat),
            true,
        )),
        Some(Probe::Unknown(why)) => Err(refuse(
            rec,
            ctx,
            format!(
                "the {} seat's holder cannot be verified alive or dead ({why}); refusing rather than guessing",
                rec.seat
            ),
            false,
        )),
    }
}

/// Gate one protected op. Called under the seat lock; on `Ok`, the caller
/// performs the op before releasing the lock (ADR-70).
pub(crate) fn gate(
    rec: &mut SeatRecord,
    ctx: &Ctx,
    caller: &Caller,
    op: &SeatOp,
) -> Result<Gate, Refusal> {
    prune(rec, ctx);
    let rc = ResolvedCaller::resolve(caller).map_err(|reason| Refusal {
        reason,
        holder: holder_view(rec, ctx),
        hints: Vec::new(),
    })?;
    match classify(rec, caller, &rc) {
        Relation::Holder => {
            let h = rec.holder.as_ref().expect("holder relation implies a holder");
            match &caller.grant {
                Some(g) if g.session_id == h.grant_session => {}
                _ => {
                    return Err(refuse(
                        rec,
                        ctx,
                        format!(
                            "this process belongs to the {} seat holder but does not carry the holder's validated grant",
                            rec.seat
                        ),
                        false,
                    ))
                }
            }
            if let Some(req) = &rec.request {
                if *op != SeatOp::HandoffWrite {
                    let mut r = refuse(
                        rec,
                        ctx,
                        format!(
                            "a takeover of the {} seat is pending (request {}): new dispatch is stopped",
                            rec.seat, req.id
                        ),
                        false,
                    );
                    r.hints = vec![format!(
                        "write your handoff, then acknowledge: aida session seat ack --seat {} --request {} --generation {} --safe-to-stop",
                        rec.seat, req.id, rec.generation
                    )];
                    return Err(r);
                }
            }
            if matches!(op, SeatOp::LoopStart { .. }) {
                bind_loop(rec, rc.self_process());
            }
            Ok(Gate::Holder { claimed: None })
        }
        Relation::Demoted(t) => Err(refuse(
            rec,
            ctx,
            format!(
                "this session no longer holds the {} seat (it was demoted at generation {}: {}); it cannot {} or claim the seat again implicitly",
                rec.seat,
                t.generation,
                t.reason,
                op.describe()
            ),
            false,
        )),
        Relation::Child { own_spec } => {
            let own_scope = matches!(op, SeatOp::Route { .. })
                && op.spec().is_some()
                && op.spec() == own_spec.as_deref();
            if own_scope {
                return Ok(Gate::Pass);
            }
            Err(refuse(
                rec,
                ctx,
                format!(
                    "this process was dispatched under the {} seat{}; a delegated worker can only act on its own spec and cannot {}",
                    rec.seat,
                    own_spec
                        .as_deref()
                        .map(|s| format!(" for {s}"))
                        .unwrap_or_default(),
                    op.describe()
                ),
                false,
            ))
        }
        rel @ (Relation::SeatGrant | Relation::Ordinary) => {
            let needs_seat = match op {
                SeatOp::LoopStart { .. } | SeatOp::HandoffWrite => true,
                SeatOp::Spawn { .. } | SeatOp::Route { .. } => rel == Relation::SeatGrant,
            };
            if !needs_seat {
                return Ok(Gate::Pass);
            }
            let grant = grant_for_claim(caller, Some(op), &rec.seat).map_err(|reason| Refusal {
                reason,
                holder: holder_view(rec, ctx),
                hints: Vec::new(),
            })?;
            let grant = grant.clone();
            let notice = try_claim(rec, ctx, &grant, &rc, ClaimKind::Implicit)?;
            if matches!(op, SeatOp::LoopStart { .. }) {
                bind_loop(rec, rc.self_process());
            }
            Ok(Gate::Holder {
                claimed: Some(notice),
            })
        }
    }
}

fn bind_loop(rec: &mut SeatRecord, process: &ProcessIdentity) {
    if let Some(h) = rec.holder.as_mut() {
        if h.anchor != *process && !h.bound.contains(process) {
            h.bound.push(process.clone());
        }
    }
}

/// Record a child spawned under the seat (called under the lock right after
/// `spawn()` returned, ADR-70).
pub(crate) fn record_child(
    rec: &mut SeatRecord,
    ctx: &Ctx,
    identity: ProcessIdentity,
    spec: Option<String>,
    what: &str,
) {
    rec.children.retain(|c| c.identity != identity);
    rec.children.push(ChildSpawn {
        identity,
        spec,
        generation: rec.generation,
        what: what.to_string(),
        spawned_at: ctx.now,
    });
}

// ---------------------------------------------------------------------------
// Explicit lifecycle: claim / release / ack / takeover
// ---------------------------------------------------------------------------

/// `aida session seat claim`. Idempotent for the current holder; clears the
/// caller's own tombstone (an explicit reclaim is how a former holder comes
/// back, never an implicit one).
pub(crate) fn claim(
    rec: &mut SeatRecord,
    ctx: &Ctx,
    caller: &Caller,
) -> Result<Option<ClaimNotice>, Refusal> {
    prune(rec, ctx);
    let rc = ResolvedCaller::resolve(caller).map_err(|reason| Refusal {
        reason,
        holder: holder_view(rec, ctx),
        hints: Vec::new(),
    })?;
    match classify(rec, caller, &rc) {
        Relation::Holder => {
            let h = rec.holder.as_ref().expect("holder");
            if caller.grant.as_ref().map(|g| &g.session_id) != Some(&h.grant_session) {
                return Err(refuse(
                    rec,
                    ctx,
                    "this process belongs to the seat holder but does not carry the holder's validated grant",
                    false,
                ));
            }
            Ok(None)
        }
        Relation::Child { .. } => Err(refuse(
            rec,
            ctx,
            format!(
                "a worker dispatched under the {} seat cannot claim it",
                rec.seat
            ),
            false,
        )),
        Relation::Demoted(_) | Relation::SeatGrant | Relation::Ordinary => {
            let grant = grant_for_claim(caller, None, &rec.seat)
                .map_err(|reason| Refusal {
                    reason,
                    holder: holder_view(rec, ctx),
                    hints: Vec::new(),
                })?
                .clone();
            try_claim(rec, ctx, &grant, &rc, ClaimKind::Explicit).map(Some)
        }
    }
}

fn require_holder<'r>(
    rec: &'r SeatRecord,
    ctx: &Ctx,
    caller: &Caller,
    generation: u64,
) -> Result<&'r Holder, Refusal> {
    let rc = ResolvedCaller::resolve(caller).map_err(|reason| Refusal {
        reason,
        holder: holder_view(rec, ctx),
        hints: Vec::new(),
    })?;
    let not_holder = |why: &str| Refusal {
        reason: format!(
            "only the current {} seat holder can do this: {why}",
            rec.seat
        ),
        holder: holder_view(rec, ctx),
        hints: Vec::new(),
    };
    let Some(h) = rec.holder.as_ref() else {
        return Err(not_holder("the seat is empty"));
    };
    if classify(rec, caller, &rc) != Relation::Holder {
        return Err(not_holder("this process is not the holder"));
    }
    // The authoritative comparison: the validated grant's session, never a
    // caller-supplied field (sketch section 3).
    if caller.grant.as_ref().map(|g| &g.session_id) != Some(&h.grant_session) {
        return Err(not_holder("the validated grant is not the holder's"));
    }
    if generation != rec.generation {
        return Err(not_holder(&format!(
            "generation {generation} is stale (current: {})",
            rec.generation
        )));
    }
    Ok(h)
}

/// Outcome of release/ack.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Handover {
    /// The seat is now empty.
    Released { generation: u64 },
    /// Ownership moved to the requester.
    Transferred { to_session: String, generation: u64 },
}

/// `aida session seat release`. With a pending request, the matching request
/// ID is required and the seat transfers to a still-valid requester in the
/// same transition; an invalid requester's request is cancelled.
pub(crate) fn release(
    rec: &mut SeatRecord,
    ctx: &Ctx,
    caller: &Caller,
    generation: u64,
    request: Option<&str>,
) -> Result<Handover, Refusal> {
    prune(rec, ctx);
    require_holder(rec, ctx, caller, generation)?;
    match (&rec.request, request) {
        (Some(pending), None) => {
            return Err(Refusal {
                reason: format!(
                    "a takeover of the {} seat is pending; release it to the requester with --request {}",
                    rec.seat, pending.id
                ),
                holder: holder_view(rec, ctx),
                hints: Vec::new(),
            })
        }
        (Some(pending), Some(id)) if pending.id != id => {
            return Err(Refusal {
                reason: format!(
                    "request {id} is not the pending takeover request ({})",
                    pending.id
                ),
                holder: holder_view(rec, ctx),
                hints: Vec::new(),
            })
        }
        (None, Some(id)) => {
            return Err(Refusal {
                reason: format!("request {id} is not pending on the {} seat", rec.seat),
                holder: holder_view(rec, ctx),
                hints: Vec::new(),
            })
        }
        _ => {}
    }
    if rec.request.is_some() {
        return hand_over(rec, ctx, "released to the takeover requester");
    }
    let from = rec.holder.as_ref().map(|h| h.grant_session.clone());
    let generation_from = rec.generation;
    demote_holder(rec, ctx, "released the seat");
    rec.generation += 1;
    let ev = new_event(
        rec,
        ctx,
        SeatEventKind::Released,
        from.as_deref(),
        None,
        generation_from,
    );
    rec.outbox.push(ev);
    Ok(Handover::Released {
        generation: rec.generation,
    })
}

/// `aida session seat ack --safe-to-stop`: transfer to the requester named by
/// the pending request. Mail is only a notification; this is the only
/// acknowledgment that moves the seat (sketch section 3).
pub(crate) fn ack(
    rec: &mut SeatRecord,
    ctx: &Ctx,
    caller: &Caller,
    request: &str,
    generation: u64,
) -> Result<Handover, Refusal> {
    prune(rec, ctx);
    require_holder(rec, ctx, caller, generation)?;
    match &rec.request {
        Some(r) if r.id == request && r.expected_generation == generation => {}
        _ => {
            return Err(Refusal {
                reason: format!(
                "request {request} is not pending against generation {generation} of the {} seat",
                rec.seat
            ),
                holder: holder_view(rec, ctx),
                hints: Vec::new(),
            })
        }
    }
    hand_over(rec, ctx, "acknowledged a takeover")
}

/// Transfer to the pending requester if it is still alive, else cancel the
/// request. Precondition: holder validated, request pending.
fn hand_over(rec: &mut SeatRecord, ctx: &Ctx, reason: &str) -> Result<Handover, Refusal> {
    let req = rec.request.clone().expect("pending request");
    let generation_from = rec.generation;
    let from = rec.holder.as_ref().map(|h| h.grant_session.clone());
    let ack_ev = new_event(
        rec,
        ctx,
        SeatEventKind::TakeoverAcknowledged,
        from.as_deref(),
        Some(&req.requester_session),
        generation_from,
    );
    rec.outbox.push(ack_ev);
    match (ctx.probe)(&req.requester_anchor) {
        Probe::Alive => {
            demote_holder(rec, ctx, reason);
            rec.generation += 1;
            rec.holder = Some(Holder {
                grant_session: req.requester_session.clone(),
                grant_id: req.requester_grant_id.clone(),
                anchor: req.requester_anchor.clone(),
                anchor_command: req.requester_command.clone(),
                bound: Vec::new(),
                since: ctx.now,
                how: ClaimKind::Takeover,
            });
            let ev = new_event(
                rec,
                ctx,
                SeatEventKind::Transferred,
                from.as_deref(),
                Some(&req.requester_session),
                generation_from,
            );
            rec.outbox.push(ev);
            rec.request = None;
            Ok(Handover::Transferred {
                to_session: req.requester_session,
                generation: rec.generation,
            })
        }
        other => {
            let mut ev = new_event(
                rec,
                ctx,
                SeatEventKind::TakeoverCancelled,
                from.as_deref(),
                Some(&req.requester_session),
                generation_from,
            );
            ev.detail = Some(format!(
                "requester is {}: {}",
                other.label(),
                other.reason().unwrap_or("")
            ));
            rec.outbox.push(ev);
            rec.request = None;
            // Releasing with an invalid requester leaves the seat empty;
            // an ack with one keeps the holder in place.
            if reason.starts_with("released") {
                demote_holder(rec, ctx, "released the seat");
                rec.generation += 1;
                let ev = new_event(
                    rec,
                    ctx,
                    SeatEventKind::Released,
                    from.as_deref(),
                    None,
                    generation_from,
                );
                rec.outbox.push(ev);
                Ok(Handover::Released {
                    generation: rec.generation,
                })
            } else {
                Err(Refusal {
                    reason: format!(
                        "the takeover requester is no longer running ({}); the request was cancelled and you still hold the seat",
                        other.reason().unwrap_or(other.label())
                    ),
                    holder: holder_view(rec, ctx),
                    hints: Vec::new(),
                })
            }
        }
    }
}

/// What `takeover --force` should do next.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TakeoverStep {
    /// The seat was empty or its holder provably dead: claimed now.
    Claimed(ClaimNotice),
    /// The caller already holds the seat.
    AlreadyHolder,
    /// A request is pending (new or resumed); notify the holder and wait
    /// until `deadline`.
    Pending {
        request: TakeoverRequest,
        created: bool,
    },
}

/// `aida session seat takeover --force [--request UUID]`: start or resume a
/// graceful takeover. `--kill` never reaches here (refused as scaffolding).
pub(crate) fn takeover(
    rec: &mut SeatRecord,
    ctx: &Ctx,
    caller: &Caller,
    grace: Duration,
    resume: Option<&str>,
) -> Result<TakeoverStep, Refusal> {
    prune(rec, ctx);
    let plain = |reason: String| Refusal {
        reason,
        holder: holder_view(rec, ctx),
        hints: Vec::new(),
    };
    let rc = ResolvedCaller::resolve(caller).map_err(plain)?;
    let relation = classify(rec, caller, &rc);
    match relation {
        Relation::Holder => return Ok(TakeoverStep::AlreadyHolder),
        Relation::Child { .. } => {
            return Err(plain(format!(
                "a worker dispatched under the {} seat cannot take it over",
                rec.seat
            )))
        }
        Relation::Demoted(_) | Relation::SeatGrant | Relation::Ordinary => {}
    }
    let grant = grant_for_claim(caller, None, &rec.seat)
        .map_err(plain)?
        .clone();

    if let Some(pending) = rec.request.clone() {
        let mine = pending.requester_session == grant.session_id
            && pending.requester_anchor == rc.anchor.identity;
        if !mine {
            return Err(Refusal {
                reason: format!(
                    "another takeover of the {} seat is already pending (request {}, by session {}, until {})",
                    rec.seat,
                    pending.id,
                    pending.requester_session,
                    pending.deadline.format("%H:%M:%SZ")
                ),
                holder: holder_view(rec, ctx),
                hints: Vec::new(),
            });
        }
        if let Some(id) = resume {
            if id != pending.id {
                return Err(plain(format!(
                    "request {id} is not your pending request ({})",
                    pending.id
                )));
            }
        }
        return Ok(TakeoverStep::Pending {
            request: pending,
            created: false,
        });
    }
    if let Some(id) = resume {
        return Err(plain(format!(
            "request {id} is no longer pending on the {} seat (it was transferred, timed out or cancelled); check `aida session seat status --seat {}`",
            rec.seat, rec.seat
        )));
    }

    let holder_session = match rec.holder.as_ref().map(|h| (h.grant_session.clone(), holder_liveness(h, ctx))) {
        None | Some((_, Probe::Dead(_))) => {
            // Empty or provably dead: no request needed (operator acceptance 6).
            return try_claim(rec, ctx, &grant, &rc, ClaimKind::Takeover).map(TakeoverStep::Claimed);
        }
        Some((_, Probe::Unknown(why))) => {
            return Err(refuse(
                rec,
                ctx,
                format!(
                    "the {} seat's holder cannot be verified alive or dead ({why}); a takeover cannot proceed",
                    rec.seat
                ),
                false,
            ))
        }
        Some((session, Probe::Alive)) => session,
    };
    let request = TakeoverRequest {
        id: Uuid::new_v4().to_string(),
        requester_session: grant.session_id.clone(),
        requester_grant_id: grant.id.clone(),
        requester_anchor: rc.anchor.identity.clone(),
        requester_command: rc.anchor.command_label(),
        expected_holder_session: holder_session.clone(),
        expected_generation: rec.generation,
        created_at: ctx.now,
        deadline: ctx.now + grace,
    };
    rec.request = Some(request.clone());
    let ev = new_event(
        rec,
        ctx,
        SeatEventKind::TakeoverRequested,
        Some(&holder_session),
        Some(&grant.session_id),
        rec.generation,
    );
    rec.outbox.push(ev);
    Ok(TakeoverStep::Pending {
        request,
        created: true,
    })
}

/// Where a waiting requester stands, re-read under the lock.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WaitState {
    /// The seat now belongs to the requester.
    Granted { generation: u64 },
    /// Still pending, before the deadline.
    Waiting,
    /// The request ended without a transfer (cancelled, or someone else
    /// holds the seat).
    Ended(String),
    /// The deadline passed; the request was marked timed out (no kill).
    TimedOut,
}

/// Poll step for a waiting requester. Times out only the matching request;
/// the old holder keeps its generation.
pub(crate) fn poll_takeover(
    rec: &mut SeatRecord,
    ctx: &Ctx,
    request_id: &str,
    requester_session: &str,
) -> WaitState {
    match &rec.request {
        Some(r) if r.id == request_id => {
            if ctx.now < r.deadline {
                return WaitState::Waiting;
            }
            let ev = new_event(
                rec,
                ctx,
                SeatEventKind::TakeoverTimedOut,
                Some(&r.expected_holder_session),
                Some(&r.requester_session),
                rec.generation,
            );
            rec.outbox.push(ev);
            rec.request = None;
            WaitState::TimedOut
        }
        _ => match &rec.holder {
            Some(h) if h.grant_session == requester_session => WaitState::Granted {
                generation: rec.generation,
            },
            _ => WaitState::Ended(format!(
                "request {request_id} is no longer pending and the seat was not transferred to you"
            )),
        },
    }
}

#[cfg(test)]
mod tests;

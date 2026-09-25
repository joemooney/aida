//! STORY-1051: the re-drive supervisor — the EPIC-62 keystone that makes an
//! unattended drain self-recover. It scans `NeedsAttention` parks, re-drives
//! only the TRANSIENT ones (typed causes from STORY-974) on a capped,
//! backed-off loop, and leaves genuine needs-human parks for escalation.
//!
//! Design (ADR-26, decided in a guided session 2026-09-12):
//! - **Execution model (fork A):** a substrate-reading command (`aida
//!   supervise`), not a post-drain-only step and not a daemon. It reads parks
//!   from the git-canonical store on each run, so it recovers a park regardless
//!   of which drain parked it or whether that drain crashed, and it needs no
//!   process kept alive.
//! - **Attempt state (fork B):** the per-spec re-drive count and last-attempt
//!   time come from `SpecReDriven` events in `.aida/events.jsonl` — append-only,
//!   never drifts, shows the recovery trail in `aida history`. No state file, no
//!   counter mutated onto the spec.
//! - **Default posture (fork C):** the AUTO-invocation of the supervisor
//!   (`[drain] supervise`) ships default-OFF; the command always works when run
//!   by hand.
//
// trace:STORY-1051 | ai:claude

use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use aida_core::{Comment, DatabaseBackend, Requirement, RequirementStatus, RequirementType};

use crate::{events, findings};

const NEEDS_HUMAN_TAG: &str = "needs-human";

/// Supervised re-drive cap (fork C default): three attempts BEYOND the in-phase
/// STORY-975 retry budget before reclassifying to needs-human.
pub(crate) const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Exponential backoff before the Nth supervised re-drive: 2m, 8m, 30m. The last
/// entry is reused for any attempt past its length.
pub(crate) const DEFAULT_BACKOFF: [Duration; 3] = [
    Duration::from_secs(2 * 60),
    Duration::from_secs(8 * 60),
    Duration::from_secs(30 * 60),
];

#[derive(Debug, Clone)]
pub(crate) struct SuperviseOpts {
    /// Actually mutate + re-drive; `false` is a dry-run (report only).
    pub(crate) execute: bool,
    /// Re-drive cap before reclassifying to needs-human.
    pub(crate) max_attempts: u32,
    /// Backoff schedule, indexed by prior attempt count.
    pub(crate) backoff: Vec<Duration>,
    /// Cap on how many specs to re-drive in a single run (`None` = no cap).
    pub(crate) max: Option<usize>,
    pub(crate) json: bool,
    /// Extra floors an unattended caller (the night shift) applies before
    /// anything else. `None` = the manual verb's behaviour.
    pub(crate) floors: Option<RedriveFloors>,
}

impl Default for SuperviseOpts {
    fn default() -> Self {
        SuperviseOpts {
            execute: false,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff: DEFAULT_BACKOFF.to_vec(),
            max: None,
            json: false,
            floors: None,
        }
    }
}

/// The night shift's re-drive floors: it touches only parks it could also
/// launch. A park outside them is left exactly as it is (no re-drive, no cap
/// reclassification) for a human or the manual verb.
// trace:TASK-1492 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RedriveFloors {
    /// Only an EXPLICIT `execution_mode = drain` park.
    pub(crate) drain_mode_only: bool,
    /// Never a keystone-class spec (the one keystone classifier).
    pub(crate) exclude_keystone: bool,
    /// Upper-cased spec ids that carry a live merge hold.
    pub(crate) held: std::collections::BTreeSet<String>,
}

/// Where a re-driven spec is put back in the queue.
// trace:TASK-1492 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueTarget {
    /// Queue file owner (the user the wave drains as).
    pub(crate) user: String,
    /// `for_role` routing (the wave's role).
    pub(crate) role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum ParkClass {
    Transient,
    NeedsHuman,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SuperviseDecision {
    pub(crate) spec: String,
    pub(crate) class: ParkClass,
    /// The failure kind (transient) or the human-reason category.
    pub(crate) reason: String,
    /// Supervised re-drives already spent (from the event log).
    pub(crate) attempts: u32,
    /// What the supervisor did / would do: re-drive, backoff-wait,
    /// reclassify-needs-human, leave-for-human, skip-max-this-run (and, under
    /// the night shift's floors, leave-not-drain-mode / leave-keystone /
    /// leave-merge-held).
    pub(crate) action: String,
    /// The spec's store id, for the targeted write that applies the decision.
    #[serde(skip)]
    pub(crate) id: Option<uuid::Uuid>,
}

/// The `aida supervise` entry point. Plans over the store + event log with
/// [`plan_redrives`], and (when `execute`) applies each decision with a
/// targeted write and launches the re-drives in the foreground.
// trace:STORY-1051 trace:TASK-1492 | ai:claude
pub(crate) fn handle_supervise_command<B: DatabaseBackend>(
    backend: &B,
    project_root: &std::path::Path,
    opts: SuperviseOpts,
) -> Result<()> {
    // STORY-1429: `store` is a read-only snapshot for classification. Every
    // write below is a targeted single-spec write that re-reads the status
    // just before writing, instead of one whole-store save of this snapshot
    // (which could revert a concurrent triage or drop a concurrently added
    // spec).
    // trace:STORY-1429 | ai:claude
    let store = backend.load()?;
    // Attempts come from the event log, archive included, so a rotation of
    // `events.jsonl` never resets the ADR-26 count (fork B).
    let history = events::RedriveHistory::from_events(&events::read_all_with_archive(project_root));
    // `--max` counts APPLIED re-drives (the manual verb's behaviour before
    // the plan/apply split): a spec that moved meanwhile does not use up a
    // slot. A dry run can only count planned ones.
    // trace:TASK-1492 | ai:claude
    let plan_opts = if opts.execute {
        SuperviseOpts {
            max: None,
            ..opts.clone()
        }
    } else {
        opts.clone()
    };
    let mut decisions = plan_redrives(
        &store.requirements,
        &history,
        &plan_opts,
        chrono::Utc::now(),
    );
    let launches = if opts.execute {
        apply_decisions(backend, project_root, &mut decisions, &opts)?
    } else {
        Vec::new()
    };

    render_decisions(&decisions, opts.json)?;

    for spec in launches {
        launch_redrive(project_root, &spec)?;
    }
    Ok(())
}

/// The manual verb's apply pass over a plan made WITHOUT `--max`: the cap
/// branch, then re-drives in plan order until `opts.max` of them have
/// actually been applied (a spec that moved meanwhile does not use up a
/// slot). After an attempt that could not be recorded nothing further is
/// re-driven (ADR-26 fail closed). Returns the specs to launch.
// trace:TASK-1492 | ai:claude
fn apply_decisions<B: DatabaseBackend>(
    backend: &B,
    project_root: &std::path::Path,
    decisions: &mut [SuperviseDecision],
    opts: &SuperviseOpts,
) -> Result<Vec<String>> {
    let mut launches: Vec<String> = Vec::new();
    let mut unrecorded: Option<String> = None;
    for decision in decisions.iter_mut() {
        match decision.action.as_str() {
            "reclassify-needs-human" => {
                if !apply_cap(backend, project_root, decision, opts.max_attempts)? {
                    decision.action = "skip-status-moved".to_string();
                }
            }
            "would-re-drive" => {
                let skip = if unrecorded.is_some() {
                    Some(HELD_UNRECORDED)
                } else if opts.max.is_some_and(|m| launches.len() >= m) {
                    Some("skip-max-this-run")
                } else {
                    None
                };
                if let Some(action) = skip {
                    decision.action = action.to_string();
                    decision.attempts = decision.attempts.saturating_sub(1);
                    continue;
                }
                // The supervisor is never a human at a terminal, so it
                // never clears an escalation (STORY-1429).
                let outcome = apply_requeue(
                    backend,
                    project_root,
                    std::slice::from_ref(decision),
                    opts.max_attempts,
                    None,
                )?;
                if let Some(reason) = outcome.unrecorded {
                    decision.action = HELD_UNRECORDED.to_string();
                    decision.attempts = decision.attempts.saturating_sub(1);
                    unrecorded = Some(reason);
                } else if outcome.applied.is_empty() {
                    decision.action = "skip-status-moved".to_string();
                    decision.attempts = decision.attempts.saturating_sub(1);
                } else {
                    decision.action = "re-drive".to_string();
                    launches.push(decision.spec.clone());
                }
            }
            _ => {}
        }
    }
    if let Some(reason) = &unrecorded {
        eprintln!("supervisor: re-drive held for the rest of this run: {reason}");
    }
    Ok(launches)
}

/// Plan one supervisor pass over the parked specs, oldest-parked first, so
/// the longest-stuck spec is recovered first. Pure over its inputs: the
/// attempt count and last re-drive time come from `history` (ADR-26 fork B).
/// Actions: `would-re-drive` (attempts already advanced to the attempt it
/// would be), `reclassify-needs-human` (cap reached), `backoff-wait`,
/// `skip-max-this-run`, `leave-for-human`, and the floor refusals.
// trace:TASK-1492 | ai:claude
pub(crate) fn plan_redrives(
    requirements: &[Requirement],
    history: &events::RedriveHistory,
    opts: &SuperviseOpts,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<SuperviseDecision> {
    let mut parked: Vec<&Requirement> = requirements
        .iter()
        .filter(|r| matches!(r.status, RequirementStatus::NeedsAttention))
        .collect();
    parked.sort_by_key(|r| r.modified_at);

    let mut decisions = Vec::new();
    let mut planned = 0usize;
    for req in parked {
        let spec = req.display_id();
        let (attempts, last_redrive) = history.get(&spec);
        let mut decision = classify_requirement(req, attempts);

        if let Some(floors) = &opts.floors {
            if let Some(action) = floor_refusal(req, &spec, floors) {
                decision.action = action.to_string();
                decisions.push(decision);
                continue;
            }
        }
        if decision.class == ParkClass::NeedsHuman {
            decisions.push(decision);
            continue;
        }
        // Transient. Cap reached → reclassify to needs-human and stop.
        if attempts >= opts.max_attempts {
            decision.action = "reclassify-needs-human".to_string();
            decisions.push(decision);
            continue;
        }
        // Backoff: wait `backoff[attempts]` since the last re-drive (or since
        // the spec was parked, for the first re-drive) before trying again.
        let since = last_redrive.unwrap_or(req.modified_at);
        let wait = backoff_for(&opts.backoff, attempts);
        let elapsed = (now - since).to_std().unwrap_or(Duration::ZERO);
        if elapsed < wait {
            decision.action = "backoff-wait".to_string();
            decisions.push(decision);
            continue;
        }
        // Per-run cap on how many we re-drive.
        if opts.max.is_some_and(|m| planned >= m) {
            decision.action = "skip-max-this-run".to_string();
            decisions.push(decision);
            continue;
        }
        planned += 1;
        decision.attempts = attempts + 1;
        decision.action = "would-re-drive".to_string();
        decisions.push(decision);
    }
    decisions
}

/// The night shift's floors, checked before any other decision.
// trace:TASK-1492 | ai:claude
fn floor_refusal(req: &Requirement, spec: &str, floors: &RedriveFloors) -> Option<&'static str> {
    if floors.drain_mode_only && req.execution_mode != Some(aida_core::ExecutionMode::Drain) {
        return Some("leave-not-drain-mode");
    }
    if floors.exclude_keystone
        && crate::presence::is_keystone_class(
            &req.req_type.to_string(),
            req.tags.iter().map(|t| t.as_str()),
        )
    {
        return Some("leave-keystone");
    }
    if floors.held.contains(&spec.to_ascii_uppercase()) {
        return Some("leave-merge-held");
    }
    None
}

/// The action a `would-re-drive` decision gets when its attempt could not be
/// recorded (or an earlier one in the same run could not): nothing re-queued.
// trace:TASK-1492 | ai:claude
pub(crate) const HELD_UNRECORDED: &str = "held-attempt-unrecorded";

/// What [`apply_requeue`] did.
// trace:TASK-1492 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RequeueOutcome {
    /// Specs re-queued, in decision order.
    pub(crate) applied: Vec<String>,
    /// Set when a re-drive attempt could not be recorded: that spec and every
    /// later decision were left parked (ADR-26 fail closed).
    pub(crate) unrecorded: Option<String>,
    /// The specs left parked because of it (the failing one and every later
    /// `would-re-drive` decision).
    pub(crate) held: Vec<String>,
}

/// Apply the `would-re-drive` decisions: each spec goes back to Approved
/// through the one requeue owner, after its attempt record (`SpecReDriven`)
/// is written. With a `queue` target, every re-queued spec is then (re-)added
/// at the HEAD of that queue in the decisions' order (oldest-parked first):
/// the implementer dequeues at pickup, so a parked spec is usually not queued
/// any more, and a status change alone would never reach a wave. A spec that
/// moved meanwhile is skipped. The first attempt that cannot be recorded
/// stops the pass: that spec and the rest stay parked, and the reason is
/// returned in [`RequeueOutcome::unrecorded`].
// trace:TASK-1492 | ai:claude
pub(crate) fn apply_requeue<B: DatabaseBackend>(
    backend: &B,
    project_root: &std::path::Path,
    decisions: &[SuperviseDecision],
    max_attempts: u32,
    queue: Option<&QueueTarget>,
) -> Result<RequeueOutcome> {
    apply_requeue_with(
        backend,
        decisions,
        max_attempts,
        queue,
        &mut |ev| events::emit_recorded(project_root, ev),
        &mut |spec, ctx, outcome| crate::requeue::emit_requeued(project_root, spec, ctx, outcome),
    )
}

/// [`apply_requeue`] with the attempt recorder and the requeue-trail emitter
/// injected (the test seam for a write that fails once and then works). A
/// store error part-way through is returned only AFTER the specs already
/// moved to Approved got their queue entry, so none is left Approved but
/// unqueued (A7).
// trace:TASK-1492 | ai:claude
fn apply_requeue_with<B: DatabaseBackend>(
    backend: &B,
    decisions: &[SuperviseDecision],
    max_attempts: u32,
    queue: Option<&QueueTarget>,
    record: &mut dyn FnMut(&events::Event) -> std::result::Result<(), String>,
    trail: &mut dyn FnMut(&str, &crate::requeue::ReturnCtx, &crate::requeue::ReturnOutcome),
) -> Result<RequeueOutcome> {
    let mut applied: Vec<(String, uuid::Uuid)> = Vec::new();
    let mut unrecorded = None;
    let mut held = Vec::new();
    let mut failed: Option<anyhow::Error> = None;
    for d in decisions.iter().filter(|d| d.action == "would-re-drive") {
        if unrecorded.is_some() {
            held.push(d.spec.clone());
            continue;
        }
        let Some(id) = d.id else { continue };
        let result = supervisor_requeue_with(
            backend,
            id,
            &d.spec,
            &d.reason,
            d.attempts,
            max_attempts,
            record,
            trail,
        );
        match result {
            Ok(RedriveApply::Applied) => applied.push((d.spec.clone(), id)),
            Ok(RedriveApply::Moved) => {}
            Ok(RedriveApply::Unrecorded(reason)) => {
                unrecorded = Some(format!(
                    "cannot record the re-drive attempt for {}: {reason}",
                    d.spec
                ));
                held.push(d.spec.clone());
            }
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
    }
    if let (Some(q), false) = (queue, applied.is_empty()) {
        let ids: Vec<uuid::Uuid> = applied.iter().map(|(_, id)| *id).collect();
        let head = backend
            .queue_list(&q.user, false)?
            .iter()
            .filter(|e| !ids.contains(&e.requirement_id) && e.position != i64::MAX)
            .map(|e| e.position)
            .min()
            .unwrap_or(0);
        let n = ids.len() as i64;
        for (i, id) in ids.iter().enumerate() {
            backend.queue_add(aida_core::QueueEntry {
                user_id: q.user.clone(),
                requirement_id: *id,
                position: head.saturating_sub((n - i as i64).saturating_mul(1000)),
                added_by: "night-shift".to_string(),
                note: Some("re-queued after a transient park".to_string()),
                added_at: chrono::Utc::now(),
                for_role: Some(q.role.clone()),
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })?;
        }
    }
    if let Some(e) = failed {
        return Err(e);
    }
    Ok(RequeueOutcome {
        applied: applied.into_iter().map(|(spec, _)| spec).collect(),
        unrecorded,
        held,
    })
}

/// Apply a `reclassify-needs-human` decision (the ADR-26 cap branch):
/// tag needs-human with one targeted write while the spec is still parked,
/// emit `ReclassifiedNeedsHuman`, and file the cap finding. Returns false
/// when the spec had moved, in which case nothing was written or emitted.
// trace:TASK-1492 | ai:claude
pub(crate) fn apply_cap<B: DatabaseBackend>(
    backend: &B,
    project_root: &std::path::Path,
    decision: &SuperviseDecision,
    max_attempts: u32,
) -> Result<bool> {
    let Some(id) = decision.id else {
        return Ok(false);
    };
    if !reclassify_needs_human_atomically(backend, id, max_attempts)? {
        return Ok(false);
    }
    events::emit(
        project_root,
        &events::Event::new(
            Some(decision.spec.clone()),
            "",
            events::EventKind::ReclassifiedNeedsHuman {
                kind: decision.reason.clone(),
                attempts: decision.attempts,
            },
        ),
    );
    backend.add_requirement(cap_finding(
        &decision.spec,
        &decision.reason,
        decision.attempts,
    ))?;
    Ok(true)
}

/// The result of one supervised re-drive.
// trace:TASK-1492 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RedriveApply {
    /// Attempt recorded and the spec is back in Approved.
    Applied,
    /// The spec was no longer parked; nothing moved.
    Moved,
    /// The attempt record could not be written, so the spec was NOT moved.
    Unrecorded(String),
}

/// One supervised re-drive. ADR-26 fail closed: the attempt record
/// (`SpecReDriven`, which the cap counts) is written with a fallible append
/// BEFORE the status change, and a failed append leaves the spec parked. A
/// spec that is already out of `NeedsAttention` is left alone without a
/// record. If the record lands but the spec moves before the atomic
/// status-checked write, the attempt stays counted: an over-count only
/// reaches the cap sooner, never later. On success also emits
/// `SpecRequeued` (the requeue trail, which the cap does not count).
// trace:STORY-1429 trace:TASK-1492 | ai:claude
pub(crate) fn supervisor_requeue<B: DatabaseBackend>(
    backend: &B,
    project_root: &std::path::Path,
    id: uuid::Uuid,
    spec: &str,
    cause: &str,
    attempt: u32,
    max_attempts: u32,
) -> Result<RedriveApply> {
    supervisor_requeue_with(
        backend,
        id,
        spec,
        cause,
        attempt,
        max_attempts,
        &mut |ev| events::emit_recorded(project_root, ev),
        &mut |spec, ctx, outcome| crate::requeue::emit_requeued(project_root, spec, ctx, outcome),
    )
}

// trace:TASK-1492 | ai:claude
#[allow(clippy::too_many_arguments)]
fn supervisor_requeue_with<B: DatabaseBackend>(
    backend: &B,
    id: uuid::Uuid,
    spec: &str,
    cause: &str,
    attempt: u32,
    max_attempts: u32,
    record: &mut dyn FnMut(&events::Event) -> std::result::Result<(), String>,
    trail: &mut dyn FnMut(&str, &crate::requeue::ReturnCtx, &crate::requeue::ReturnOutcome),
) -> Result<RedriveApply> {
    let parked = backend
        .get_requirement(&id)?
        .is_some_and(|r| r.status == RequirementStatus::NeedsAttention);
    if !parked {
        return Ok(RedriveApply::Moved);
    }
    if let Err(reason) = record(&events::Event::new(
        Some(spec.to_string()),
        "",
        events::EventKind::SpecReDriven {
            cause: cause.to_string(),
            attempt,
            max: max_attempts,
        },
    )) {
        return Ok(RedriveApply::Unrecorded(reason));
    }
    let ctx = crate::requeue::ReturnCtx {
        via: "the re-drive supervisor".to_string(),
        via_slug: "supervisor",
        author: "supervisor".to_string(),
        clear_escalation: false,
        reason: Some(format!(
            "supervised re-drive {attempt}/{max_attempts} after transient `{cause}`"
        )),
    };
    let (outcome, _) = crate::requeue::return_to_flight_in_backend(
        backend,
        id,
        &RequirementStatus::NeedsAttention,
        &RequirementStatus::Approved,
        &ctx,
    )?;
    if !outcome.applied() {
        return Ok(RedriveApply::Moved);
    }
    trail(spec, &ctx, &outcome);
    Ok(RedriveApply::Applied)
}

/// Reclassify a capped transient park to needs-human with one targeted write,
/// only while it is still parked. The status check and the reclassification
/// run on the copy read under the store write lock (`update_spec_atomically`),
/// so a spec that moved between the listing and the write is left alone and a
/// concurrent change to it is never overwritten. Returns false when it moved.
// trace:STORY-1429 trace:TASK-1506 | ai:claude
fn reclassify_needs_human_atomically<B: DatabaseBackend>(
    backend: &B,
    id: uuid::Uuid,
    max_attempts: u32,
) -> Result<bool> {
    let Some(located) = backend.get_requirement(&id)? else {
        return Ok(false);
    };
    let mut applied = false;
    backend.update_spec_atomically(&located, |r| {
        if r.status == RequirementStatus::NeedsAttention {
            reclassify_needs_human(r, max_attempts);
            applied = true;
        }
    })?;
    Ok(applied)
}

/// Classify one parked spec. Transient iff it carries a typed transient
/// failure kind and no design-fork/needs-human marker.
// trace:STORY-1051 | ai:claude
pub(crate) fn classify_requirement(req: &Requirement, attempts: u32) -> SuperviseDecision {
    let spec = req.display_id();
    let id = Some(req.id);
    let human = |reason: &str| SuperviseDecision {
        spec: spec.clone(),
        class: ParkClass::NeedsHuman,
        reason: reason.to_string(),
        attempts,
        action: "leave-for-human".to_string(),
        id,
    };
    if req.tags.contains(NEEDS_HUMAN_TAG) {
        return human(NEEDS_HUMAN_TAG);
    }
    if let Some(attention) = &req.attention_reason {
        return human(&attention.category.to_string());
    }
    let Some(fr) = &req.failure_reason else {
        return human("missing-failure-reason");
    };
    if is_transient_failure_kind(&fr.kind) {
        SuperviseDecision {
            spec,
            class: ParkClass::Transient,
            reason: fr.kind.clone(),
            attempts,
            action: "would-re-drive".to_string(),
            id,
        }
    } else {
        // ci-red on a real test failure, request-changes on substance,
        // environmental — the integrity floor. NEVER auto-retried.
        human(&fr.kind)
    }
}

/// The transient (auto-re-drivable) failure kinds — a tooling hiccup that a
/// re-run clears. Everything else (ci-red, request-changes, environmental,
/// internal) is needs-human and never touched by the supervisor.
// trace:STORY-1051 | ai:claude
pub(crate) fn is_transient_failure_kind(kind: &str) -> bool {
    matches!(
        normalize_kind(kind).as_str(),
        "watchdog"
            | "no-verdict"
            | "no-pr"
            | "tool-exit"
            | "lease-conflict"
            | "cache-locked"
            | "headless-wait"
            | "spawn"
            | "inconclusive"
    )
}

fn normalize_kind(kind: &str) -> String {
    kind.trim()
        .chars()
        .map(|c| match c {
            '_' | ' ' => '-',
            c if c.is_ascii_uppercase() => c.to_ascii_lowercase(),
            c => c,
        })
        .collect()
}

/// Backoff for the re-drive after `prior_attempts` already-spent attempts.
/// Clamps to the last schedule entry for attempts past its length.
fn backoff_for(schedule: &[Duration], prior_attempts: u32) -> Duration {
    if schedule.is_empty() {
        return Duration::ZERO;
    }
    let i = (prior_attempts as usize).min(schedule.len() - 1);
    schedule[i]
}

fn reclassify_needs_human(req: &mut Requirement, max_attempts: u32) {
    req.tags.insert(NEEDS_HUMAN_TAG.to_string());
    req.add_comment(Comment::new(
        "supervisor".to_string(),
        format!(
            "Supervisor retry cap exhausted after {max_attempts} transient re-drive attempt(s). \
             Leaving this parked for human triage."
        ),
    ));
    req.modified_at = chrono::Utc::now();
}

/// The cap finding, added with a single targeted add (no whole-store save).
// trace:STORY-1429 | ai:claude
fn cap_finding(spec: &str, kind: &str, attempts: u32) -> Requirement {
    let mut finding = Requirement::new(
        format!("Supervisor retry cap exhausted for {spec}"),
        format!(
            "{spec} remained parked after {attempts} supervised re-drive attempt(s) \
             for transient failure `{kind}`. Inspect the latest run and decide whether \
             the failure is environmental, a real product/code issue, or needs a new spec."
        ),
    );
    finding.req_type = RequirementType::Task;
    finding.status = RequirementStatus::Draft;
    finding.owner = "supervisor".to_string();
    finding
        .tags
        .insert(format!("{}{}", findings::FROM_ADVISOR_PREFIX, spec));
    finding.tags.insert("kind:supervisor-cap".to_string());
    finding.tags.insert("severity:major".to_string());
    finding.tags.insert(format!("linked:{spec}"));
    finding
}

fn render_decisions(decisions: &[SuperviseDecision], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(decisions)?);
        return Ok(());
    }
    if decisions.is_empty() {
        println!("No parked specs to supervise.");
        return Ok(());
    }
    println!("Supervisor decisions");
    for d in decisions {
        println!(
            "  {:<14} {:<10} {:<22} attempts={} action={}",
            d.spec,
            match d.class {
                ParkClass::Transient => "transient",
                ParkClass::NeedsHuman => "human",
            },
            d.reason,
            d.attempts,
            d.action
        );
    }
    Ok(())
}

/// A supervised re-drive is a NORMAL drive through the existing path
/// (`queue work --from-pr`/`--force-claim --auto-complete --no-human=both`),
/// not a new orchestration path (ADR-26).
fn launch_redrive(project_root: &std::path::Path, spec: &str) -> Result<()> {
    let status = std::process::Command::new(crate::aida_exe_path())
        .current_dir(project_root)
        .args([
            "queue",
            "work",
            spec,
            "--auto-complete",
            "--force-claim",
            "--no-human",
            "both",
        ])
        .status()?;
    if !status.success() {
        anyhow::bail!(
            "supervisor re-drive for {spec} exited {}",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "with a signal".to_string())
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::{FailureReason, PuntCategory};

    fn req_with_failure(kind: &str) -> Requirement {
        let mut req = Requirement::new("parked".to_string(), "parked".to_string());
        req.spec_id = Some("STORY-1".to_string());
        req.status = RequirementStatus::NeedsAttention;
        req.failure_reason = Some(FailureReason {
            phase: "implementer".to_string(),
            phase_index: 1,
            kind: kind.to_string(),
            detail: "failed".to_string(),
            recovery_hint: None,
            shelved_by: None,
            shelved_at: chrono::Utc::now(),
        });
        req
    }

    #[test]
    fn classifies_transient_failure_kinds() {
        for kind in [
            "watchdog",
            "no-verdict",
            "no-pr",
            "tool-exit",
            "lease-conflict",
            "cache-locked",
            "headless-wait",
        ] {
            assert!(is_transient_failure_kind(kind), "{kind}");
        }
        // The integrity floor is never transient.
        assert!(!is_transient_failure_kind("ci-red"));
        assert!(!is_transient_failure_kind("request-changes"));
        assert!(!is_transient_failure_kind("environmental"));
    }

    #[test]
    fn watchdog_park_is_transient() {
        let d = classify_requirement(&req_with_failure("watchdog"), 0);
        assert_eq!(d.class, ParkClass::Transient);
        assert_eq!(d.attempts, 0);
    }

    #[test]
    fn ci_red_park_is_needs_human_not_retried() {
        let d = classify_requirement(&req_with_failure("ci-red"), 0);
        assert_eq!(d.class, ParkClass::NeedsHuman);
    }

    #[test]
    fn design_fork_attention_stays_human() {
        let mut req = req_with_failure("watchdog");
        req.attention_reason = Some(aida_core::AttentionReason {
            category: PuntCategory::DesignFork,
            detail: "pick an architecture".to_string(),
            lean: None,
            raised_by: None,
            raised_at: chrono::Utc::now(),
        });
        let d = classify_requirement(&req, 0);
        assert_eq!(d.class, ParkClass::NeedsHuman);
        assert_eq!(d.reason, "design-fork");
    }

    #[test]
    fn needs_human_tag_prevents_duplicate_reclassification() {
        let mut req = req_with_failure("watchdog");
        req.tags.insert(NEEDS_HUMAN_TAG.to_string());
        let d = classify_requirement(&req, 2);
        assert_eq!(d.class, ParkClass::NeedsHuman);
        assert_eq!(d.reason, NEEDS_HUMAN_TAG);
    }

    #[test]
    fn attempts_are_carried_from_the_event_derived_count() {
        // The caller passes the event-derived count; classify reflects it.
        let d = classify_requirement(&req_with_failure("watchdog"), 3);
        assert_eq!(d.attempts, 3);
    }

    #[test]
    fn backoff_grows_then_clamps() {
        let s = DEFAULT_BACKOFF.to_vec();
        assert_eq!(backoff_for(&s, 0), Duration::from_secs(120));
        assert_eq!(backoff_for(&s, 1), Duration::from_secs(480));
        assert_eq!(backoff_for(&s, 2), Duration::from_secs(1800));
        // past the schedule → clamp to the last entry.
        assert_eq!(backoff_for(&s, 9), Duration::from_secs(1800));
        assert_eq!(backoff_for(&[], 0), Duration::ZERO);
    }

    // STORY-1429: the supervisor's re-drive runs through the one owner, on the
    // one spec with a targeted write. It clears the attention and
    // failure markers, records one audit note, and emits both the attempt
    // record (SpecReDriven) and the requeue trail (SpecRequeued). A second
    // call finds the spec no longer parked and changes nothing.
    // trace:STORY-1429 | ai:claude
    #[test]
    fn supervisor_redrive_routes_through_return_to_flight_and_clears_attention_reason() {
        let _env = crate::test_env::EnvVarsGuard::apply(&[(events::EVENTS_DISABLE_ENV, None)]);
        let tmp = tempfile::tempdir().unwrap();
        let backend = aida_core::GitBackend::new(&tmp.path().join(".aida-store")).unwrap();
        let mut req = req_with_failure("watchdog");
        req.attention_reason = Some(aida_core::AttentionReason {
            category: PuntCategory::DesignFork,
            detail: "stale".into(),
            lean: None,
            raised_by: None,
            raised_at: chrono::Utc::now(),
        });
        let id = req.id;
        let mut store = aida_core::RequirementsStore::default();
        store.requirements.push(req);
        backend.save(&store).unwrap();

        assert_eq!(
            supervisor_requeue(&backend, tmp.path(), id, "STORY-1", "watchdog", 1, 3).unwrap(),
            RedriveApply::Applied
        );
        let after = backend.get_requirement(&id).unwrap().unwrap();
        assert_eq!(after.status, RequirementStatus::Approved);
        assert!(after.attention_reason.is_none());
        assert!(after.failure_reason.is_none());
        assert_eq!(after.comments.len(), 1);
        assert!(after.comments[0]
            .content
            .contains("via the re-drive supervisor"));

        let evs = events::read_all(tmp.path());
        let kinds: Vec<&str> = evs.iter().map(|e| e.kind.name()).collect();
        assert_eq!(kinds, vec!["SpecReDriven", "SpecRequeued"]);
        assert_eq!(events::supervisor_redrive_state(tmp.path(), "STORY-1").0, 1);

        // Under-the-write check: no longer parked, so nothing happens.
        assert_eq!(
            supervisor_requeue(&backend, tmp.path(), id, "STORY-1", "watchdog", 2, 3).unwrap(),
            RedriveApply::Moved
        );
        assert_eq!(events::read_all(tmp.path()).len(), 2);
        assert_eq!(
            backend
                .get_requirement(&id)
                .unwrap()
                .unwrap()
                .comments
                .len(),
            1
        );
    }

    // STORY-1429 (A2): a human requeue neither advances nor resets the
    // supervisor's attempt count. trace:STORY-1429 | ai:claude
    #[test]
    fn human_requeue_neither_advances_nor_resets_supervisor_attempts() {
        let _env = crate::test_env::EnvVarsGuard::apply(&[(events::EVENTS_DISABLE_ENV, None)]);
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        events::emit(
            root,
            &events::Event::new(
                Some("STORY-7".into()),
                "",
                events::EventKind::SpecReDriven {
                    cause: "watchdog".into(),
                    attempt: 1,
                    max: 3,
                },
            ),
        );
        let before = events::supervisor_redrive_state(root, "STORY-7");
        assert_eq!(before.0, 1);
        events::emit(
            root,
            &events::Event::new(
                Some("STORY-7".into()),
                "",
                events::EventKind::SpecRequeued {
                    via: "queue-rework".into(),
                    actor: Some("human".into()),
                    from: "Needs Attention".into(),
                    to: "Approved".into(),
                    cleared_tags: vec!["needs-human".into()],
                    kept_tags: Vec::new(),
                },
            ),
        );
        assert_eq!(events::supervisor_redrive_state(root, "STORY-7"), before);
    }

    // The parked check and the reclassification run on the copy read under
    // the store lock: a spec that moved is left alone, and a concurrent change
    // to a still-parked spec is kept. trace:TASK-1506 | ai:claude
    #[test]
    fn reclassify_runs_on_the_copy_under_the_lock() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join(".aida-store");
        std::fs::create_dir_all(&root).unwrap();
        let backend = aida_core::GitBackend::new(&root).unwrap();
        let parked = backend
            .add_requirement(req_with_failure("watchdog"))
            .unwrap();

        // Moved out of NeedsAttention after the supervisor listed it.
        let other = aida_core::GitBackend::new(&root).unwrap();
        let mut moved = other.get_requirement(&parked.id).unwrap().unwrap();
        moved.status = RequirementStatus::InProgress;
        other.update_requirement(&moved).unwrap();
        assert!(!reclassify_needs_human_atomically(&backend, parked.id, 3).unwrap());
        let after = backend.get_requirement(&parked.id).unwrap().unwrap();
        assert!(!after.tags.contains(NEEDS_HUMAN_TAG));
        assert!(after.comments.is_empty());

        // Still parked, with a concurrent owner change: both land.
        let mut theirs = after.clone();
        theirs.status = RequirementStatus::NeedsAttention;
        theirs.owner = "someone".to_string();
        other.update_requirement(&theirs).unwrap();
        assert!(reclassify_needs_human_atomically(&backend, parked.id, 3).unwrap());
        let done = backend.get_requirement(&parked.id).unwrap().unwrap();
        assert!(done.tags.contains(NEEDS_HUMAN_TAG));
        assert_eq!(done.owner, "someone");
        assert_eq!(done.comments.len(), 1);
    }

    /// A parked spec stored in a fresh temp store, parked `mins_ago`.
    // trace:TASK-1492 | ai:claude
    fn stored_park(backend: &aida_core::GitBackend, spec: &str, mins_ago: i64) -> Requirement {
        let mut req = req_with_failure("watchdog");
        req.spec_id = Some(spec.to_string());
        req.modified_at = chrono::Utc::now() - chrono::Duration::minutes(mins_ago);
        backend.add_requirement(req).unwrap()
    }

    // B1: the attempt record is written BEFORE the status change. When it
    // cannot be written the spec stays parked and no requeue trail appears;
    // the ADR-26 cap can never be bypassed by a failed append.
    // trace:TASK-1492 | ai:claude
    #[test]
    fn supervisor_requeue_unrecordable_attempt_leaves_spec_parked() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = aida_core::GitBackend::new(&tmp.path().join(".aida-store")).unwrap();
        let parked = stored_park(&backend, "STORY-1", 60);
        let still_parked = |backend: &aida_core::GitBackend| {
            let r = backend.get_requirement(&parked.id).unwrap().unwrap();
            r.status == RequirementStatus::NeedsAttention && r.comments.is_empty()
        };
        {
            // Events disabled in this process: nothing would be recorded.
            let _env =
                crate::test_env::EnvVarsGuard::apply(&[(events::EVENTS_DISABLE_ENV, Some("1"))]);
            match supervisor_requeue(&backend, tmp.path(), parked.id, "STORY-1", "watchdog", 1, 3)
                .unwrap()
            {
                RedriveApply::Unrecorded(r) => {
                    assert!(r.contains(events::EVENTS_DISABLE_ENV), "{r}")
                }
                other => panic!("expected Unrecorded, got {other:?}"),
            }
            assert!(still_parked(&backend));
        }
        let _env = crate::test_env::EnvVarsGuard::apply(&[(events::EVENTS_DISABLE_ENV, None)]);
        let aida = tmp.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        // A read-only events file (skipped where permissions do not bind,
        // e.g. when the tests run as root).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = aida.join("events.jsonl");
            std::fs::write(&file, "").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
            let binds = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .is_err();
            if binds {
                assert!(matches!(
                    supervisor_requeue(
                        &backend,
                        tmp.path(),
                        parked.id,
                        "STORY-1",
                        "watchdog",
                        1,
                        3
                    )
                    .unwrap(),
                    RedriveApply::Unrecorded(_)
                ));
                assert!(still_parked(&backend));
            }
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
            std::fs::remove_file(&file).unwrap();
        }
        // An events path that cannot be appended to at all (replaced by a
        // directory): fails for every user.
        std::fs::create_dir(aida.join("events.jsonl")).unwrap();
        match supervisor_requeue(&backend, tmp.path(), parked.id, "STORY-1", "watchdog", 1, 3)
            .unwrap()
        {
            RedriveApply::Unrecorded(r) => assert!(r.contains("cannot append"), "{r}"),
            other => panic!("expected Unrecorded, got {other:?}"),
        }
        assert!(still_parked(&backend));
        std::fs::remove_dir(aida.join("events.jsonl")).unwrap();

        // Writable again: the attempt is recorded first, then the move.
        assert_eq!(
            supervisor_requeue(&backend, tmp.path(), parked.id, "STORY-1", "watchdog", 1, 3)
                .unwrap(),
            RedriveApply::Applied
        );
        let kinds: Vec<&str> = events::read_all(tmp.path())
            .iter()
            .map(|e| e.kind.name())
            .collect();
        assert_eq!(kinds, vec!["SpecReDriven", "SpecRequeued"]);
    }

    // B1 hold: once one attempt cannot be recorded, apply_requeue re-queues
    // nothing further in the pass, and the manual verb holds the rest of
    // its run the same way.
    // trace:TASK-1492 | ai:claude
    #[test]
    fn unrecorded_attempt_holds_the_rest_of_the_pass() {
        let _env = crate::test_env::EnvVarsGuard::apply(&[(events::EVENTS_DISABLE_ENV, None)]);
        let tmp = tempfile::tempdir().unwrap();
        let backend = aida_core::GitBackend::new(&tmp.path().join(".aida-store")).unwrap();
        stored_park(&backend, "STORY-1", 90);
        stored_park(&backend, "STORY-2", 60);
        std::fs::create_dir_all(tmp.path().join(".aida").join("events.jsonl")).unwrap();
        let store = backend.load().unwrap();
        let opts = SuperviseOpts {
            execute: true,
            ..Default::default()
        };
        let plan = plan_redrives(
            &store.requirements,
            &events::RedriveHistory::default(),
            &opts,
            chrono::Utc::now(),
        );
        let out = apply_requeue(&backend, tmp.path(), &plan, 3, None).unwrap();
        assert!(out.applied.is_empty());
        assert!(out.unrecorded.as_deref().unwrap().contains("STORY-1"));
        assert_eq!(out.held, vec!["STORY-1", "STORY-2"]);

        let mut decisions = plan.clone();
        let launches = apply_decisions(&backend, tmp.path(), &mut decisions, &opts).unwrap();
        assert!(launches.is_empty());
        assert!(decisions
            .iter()
            .all(|d| d.action == HELD_UNRECORDED && d.attempts == 0));
        for r in backend.load().unwrap().requirements {
            assert_eq!(r.status, RequirementStatus::NeedsAttention);
        }
    }

    // M3: manual `aida supervise --max N` counts APPLIED re-drives: a spec
    // that moved meanwhile does not use up a slot.
    // trace:TASK-1492 | ai:claude
    #[test]
    fn manual_max_counts_applied_redrives_not_planned_ones() {
        let _env = crate::test_env::EnvVarsGuard::apply(&[(events::EVENTS_DISABLE_ENV, None)]);
        let tmp = tempfile::tempdir().unwrap();
        let backend = aida_core::GitBackend::new(&tmp.path().join(".aida-store")).unwrap();
        let moved = stored_park(&backend, "STORY-1", 120);
        stored_park(&backend, "STORY-2", 90);
        stored_park(&backend, "STORY-3", 60);
        // The plan's snapshot still sees STORY-1 parked...
        let snapshot = backend.load().unwrap();
        // ...but it moved before the apply.
        let mut m = backend.get_requirement(&moved.id).unwrap().unwrap();
        m.status = RequirementStatus::InProgress;
        backend.update_requirement(&m).unwrap();

        let opts = SuperviseOpts {
            execute: true,
            max: Some(1),
            ..Default::default()
        };
        let mut decisions = plan_redrives(
            &snapshot.requirements,
            &events::RedriveHistory::default(),
            &SuperviseOpts {
                max: None,
                ..opts.clone()
            },
            chrono::Utc::now(),
        );
        let launches = apply_decisions(&backend, tmp.path(), &mut decisions, &opts).unwrap();
        assert_eq!(launches, vec!["STORY-2"]);
        let action = |spec: &str| {
            decisions
                .iter()
                .find(|d| d.spec == spec)
                .map(|d| (d.action.clone(), d.attempts))
                .unwrap()
        };
        assert_eq!(action("STORY-1"), ("skip-status-moved".to_string(), 0));
        assert_eq!(action("STORY-2"), ("re-drive".to_string(), 1));
        assert_eq!(action("STORY-3"), ("skip-max-this-run".to_string(), 0));
    }

    // B1 direct: the first attempt record fails, the next one would work.
    // The pass still holds: the later specs are never tried, stay parked,
    // and nothing is queued. A failure on the second spec keeps the first
    // re-queued and holds the rest.
    // trace:TASK-1492 | ai:claude
    #[test]
    fn a_failed_record_holds_the_pass_even_when_the_next_write_would_succeed() {
        for fail_at in [0usize, 1] {
            let tmp = tempfile::tempdir().unwrap();
            let backend = aida_core::GitBackend::new(&tmp.path().join(".aida-store")).unwrap();
            stored_park(&backend, "STORY-1", 120);
            stored_park(&backend, "STORY-2", 90);
            stored_park(&backend, "STORY-3", 60);
            let store = backend.load().unwrap();
            let plan = plan_redrives(
                &store.requirements,
                &events::RedriveHistory::default(),
                &SuperviseOpts::default(),
                chrono::Utc::now(),
            );
            let queue = QueueTarget {
                user: "joe".to_string(),
                role: "implementer".to_string(),
            };
            let mut calls = 0usize;
            let mut recorded: Vec<String> = Vec::new();
            let mut record = |ev: &events::Event| {
                let n = calls;
                calls += 1;
                if n == fail_at {
                    Err("transient write error".to_string())
                } else {
                    recorded.push(ev.spec.clone().unwrap_or_default());
                    Ok(())
                }
            };
            let out = apply_requeue_with(
                &backend,
                &plan,
                3,
                Some(&queue),
                &mut record,
                &mut |_, _, _| {},
            )
            .unwrap();
            let expect_applied: Vec<&str> = ["STORY-1", "STORY-2", "STORY-3"][..fail_at].to_vec();
            let expect_held: Vec<&str> = ["STORY-1", "STORY-2", "STORY-3"][fail_at..].to_vec();
            assert_eq!(out.applied, expect_applied, "fail_at {fail_at}");
            assert_eq!(out.held, expect_held, "fail_at {fail_at}");
            assert!(out.unrecorded.as_deref().unwrap().contains(expect_held[0]));
            assert_eq!(calls, fail_at + 1, "no record is tried after the failure");
            assert_eq!(recorded, expect_applied);
            let queued = backend.queue_list("joe", false).unwrap();
            assert_eq!(queued.len(), fail_at);
            for r in backend.load().unwrap().requirements {
                let want = if expect_applied.contains(&r.display_id().as_str()) {
                    RequirementStatus::Approved
                } else {
                    RequirementStatus::NeedsAttention
                };
                assert_eq!(r.status, want, "{} fail_at {fail_at}", r.display_id());
            }
        }
    }
}

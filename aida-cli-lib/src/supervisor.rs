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
    let mut decisions = plan_redrives(&store.requirements, &history, &opts, chrono::Utc::now());
    let mut launches: Vec<String> = Vec::new();

    if opts.execute {
        for decision in decisions.iter_mut() {
            match decision.action.as_str() {
                "reclassify-needs-human" => {
                    if !apply_cap(backend, project_root, decision, opts.max_attempts)? {
                        decision.action = "skip-status-moved".to_string();
                    }
                }
                "would-re-drive" => {
                    // The supervisor is never a human at a terminal, so it
                    // never clears an escalation (STORY-1429).
                    let applied = apply_requeue(
                        backend,
                        project_root,
                        std::slice::from_ref(decision),
                        opts.max_attempts,
                        None,
                    )?;
                    if applied.is_empty() {
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
    }

    render_decisions(&decisions, opts.json)?;

    if opts.execute {
        for spec in launches {
            launch_redrive(project_root, &spec)?;
        }
    }
    Ok(())
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

/// Apply the `would-re-drive` decisions: each spec goes back to Approved
/// through the one requeue owner (emitting `SpecReDriven`, the attempt
/// record). With a `queue` target, every re-queued spec is then (re-)added at
/// the HEAD of that queue in the decisions' order (oldest-parked first): the
/// implementer dequeues at pickup, so a parked spec is usually not queued any
/// more, and a status change alone would never reach a wave. Returns the
/// specs actually re-queued; a spec that moved meanwhile is skipped.
// trace:TASK-1492 | ai:claude
pub(crate) fn apply_requeue<B: DatabaseBackend>(
    backend: &B,
    project_root: &std::path::Path,
    decisions: &[SuperviseDecision],
    max_attempts: u32,
    queue: Option<&QueueTarget>,
) -> Result<Vec<String>> {
    let mut applied: Vec<(String, uuid::Uuid)> = Vec::new();
    for d in decisions.iter().filter(|d| d.action == "would-re-drive") {
        let Some(id) = d.id else { continue };
        if supervisor_requeue(
            backend,
            project_root,
            id,
            &d.spec,
            &d.reason,
            d.attempts,
            max_attempts,
        )? {
            applied.push((d.spec.clone(), id));
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
    Ok(applied.into_iter().map(|(spec, _)| spec).collect())
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

/// One supervised re-drive's store transition: the owner runs on the copy
/// read inside the backend's atomic write. On success emits `SpecReDriven`
/// (the attempt record `supervisor_redrive_state` counts) and `SpecRequeued`
/// (the requeue trail, which it does not count). Returns false when the spec
/// was no longer parked, in which case nothing was written or emitted.
// trace:STORY-1429 | ai:claude
pub(crate) fn supervisor_requeue<B: DatabaseBackend>(
    backend: &B,
    project_root: &std::path::Path,
    id: uuid::Uuid,
    spec: &str,
    cause: &str,
    attempt: u32,
    max_attempts: u32,
) -> Result<bool> {
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
        return Ok(false);
    }
    events::emit(
        project_root,
        &events::Event::new(
            Some(spec.to_string()),
            "",
            events::EventKind::SpecReDriven {
                cause: cause.to_string(),
                attempt,
                max: max_attempts,
            },
        ),
    );
    crate::requeue::emit_requeued(project_root, spec, &ctx, &outcome);
    Ok(true)
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

        assert!(supervisor_requeue(&backend, tmp.path(), id, "STORY-1", "watchdog", 1, 3).unwrap());
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
        assert!(
            !supervisor_requeue(&backend, tmp.path(), id, "STORY-1", "watchdog", 2, 3).unwrap()
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
}

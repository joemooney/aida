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
}

impl Default for SuperviseOpts {
    fn default() -> Self {
        SuperviseOpts {
            execute: false,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff: DEFAULT_BACKOFF.to_vec(),
            max: None,
            json: false,
        }
    }
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
    /// reclassify-needs-human, leave-for-human, skip-max-this-run.
    pub(crate) action: String,
}

/// The `aida supervise` entry point. Pure-ish: reads the store + event log,
/// decides per park, and (when `execute`) mutates the store and launches the
/// re-drives.
// trace:STORY-1051 | ai:claude
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
    let mut decisions = Vec::new();
    let mut launches: Vec<String> = Vec::new();
    let mut cap_findings: Vec<(String, String, u32)> = Vec::new();
    let mut launched = 0usize;
    let now = chrono::Utc::now();

    // Oldest-parked first, so the longest-stuck spec is recovered first.
    let mut parked: Vec<usize> = store
        .requirements
        .iter()
        .enumerate()
        .filter_map(|(idx, r)| matches!(r.status, RequirementStatus::NeedsAttention).then_some(idx))
        .collect();
    parked.sort_by(|a, b| {
        store.requirements[*a]
            .modified_at
            .cmp(&store.requirements[*b].modified_at)
    });

    for idx in parked {
        let spec = store.requirements[idx].display_id();
        // Attempt count + last re-drive time come from the event log (fork B).
        let (attempts, last_redrive) = events::supervisor_redrive_state(project_root, &spec);
        let mut decision = classify_requirement(&store.requirements[idx], attempts);

        if decision.class == ParkClass::NeedsHuman {
            decisions.push(decision);
            continue;
        }

        // Transient. Cap reached → reclassify to needs-human and stop.
        if attempts >= opts.max_attempts {
            if opts.execute {
                let id = store.requirements[idx].id;
                if !reclassify_needs_human_atomically(backend, id, opts.max_attempts)? {
                    decision.action = "skip-status-moved".to_string();
                    decisions.push(decision);
                    continue;
                }
                cap_findings.push((spec.clone(), decision.reason.clone(), attempts));
                events::emit(
                    project_root,
                    &events::Event::new(
                        Some(spec.clone()),
                        "",
                        events::EventKind::ReclassifiedNeedsHuman {
                            kind: decision.reason.clone(),
                            attempts,
                        },
                    ),
                );
            }
            decision.action = "reclassify-needs-human".to_string();
            decisions.push(decision);
            continue;
        }

        // Backoff: wait `backoff[attempts]` since the last re-drive (or since the
        // spec was parked, for the first re-drive) before trying again.
        let since = last_redrive.unwrap_or(store.requirements[idx].modified_at);
        let wait = backoff_for(&opts.backoff, attempts);
        let elapsed = (now - since).to_std().unwrap_or(Duration::ZERO);
        if elapsed < wait {
            decision.action = "backoff-wait".to_string();
            decisions.push(decision);
            continue;
        }

        // Per-run cap on how many we re-drive.
        if opts.max.is_some_and(|m| launched >= m) {
            decision.action = "skip-max-this-run".to_string();
            decisions.push(decision);
            continue;
        }

        // Re-drive: re-queue (status → Approved, clear the failure) and emit the
        // event that IS the attempt record. The actual drive is launched after
        // the write lands. STORY-1429: the transition goes through the one
        // owner with one targeted single-spec write; a spec that moved in
        // the meantime (a human triaged it, a drain claimed it) is skipped. The
        // supervisor is never a human at a terminal, so it never clears an
        // escalation.
        // trace:STORY-1429 | ai:claude
        let next_attempt = attempts + 1;
        if opts.execute {
            match supervisor_requeue(
                backend,
                project_root,
                store.requirements[idx].id,
                &spec,
                &decision.reason,
                next_attempt,
                opts.max_attempts,
            )? {
                true => {
                    launches.push(spec.clone());
                    launched += 1;
                }
                false => {
                    decision.action = "skip-status-moved".to_string();
                    decisions.push(decision);
                    continue;
                }
            }
        }
        decision.attempts = next_attempt;
        decision.action = if opts.execute {
            "re-drive".to_string()
        } else {
            "would-re-drive".to_string()
        };
        decisions.push(decision);
    }

    for (spec, kind, attempts) in cap_findings {
        backend.add_requirement(cap_finding(&spec, &kind, attempts))?;
    }

    render_decisions(&decisions, opts.json)?;

    if opts.execute {
        for spec in launches {
            launch_redrive(project_root, &spec)?;
        }
    }
    Ok(())
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
    let human = |reason: &str| SuperviseDecision {
        spec: spec.clone(),
        class: ParkClass::NeedsHuman,
        reason: reason.to_string(),
        attempts,
        action: "leave-for-human".to_string(),
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

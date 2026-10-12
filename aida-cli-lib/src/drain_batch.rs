//! batch and single-branch drain drivers and cluster PRs (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

// Child-process batch pipelining preserves the existing per-spec engine while
// passing the in-process outcome back to the parent scheduler.
// trace:STORY-1091 trace:ADR-28 | ai:codex
pub(crate) fn write_pipelined_child_result_sidecar(result: &auto_complete::OrchestrationResult) {
    let Some(path) = std::env::var_os("AIDA_PIPELINED_RESULT_PATH") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let escalation = result.escalation.as_ref().map(|e| {
        serde_json::json!({
            "kind": match e.kind {
                auto_complete::EscalationKind::MergeDecision => "merge-decision",
                auto_complete::EscalationKind::DesignFork => "design-fork",
                auto_complete::EscalationKind::SupervisedMerge => "supervised-merge",
            },
            "reason": e.reason,
        })
    });
    let value = serde_json::json!({
        "exit_code": result.exit_code,
        "failed_phase": result.failed_phase.map(|p| p.index()),
        "punt_reason": result.punt_reason,
        "shipped_spec_id": result.shipped_spec_id,
        "escalation": escalation,
        "inconclusive_reason": result.inconclusive_reason,
        "shelved": result.shelved_reason.is_some(),
        "held_reason": result.held_reason,
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&value) {
        let _ = std::fs::write(path, bytes);
    }
}

// trace:STORY-1091 trace:ADR-28 | ai:codex
pub(crate) fn read_pipelined_child_result_sidecar(
    path: &std::path::Path,
) -> Option<auto_complete::OrchestrationResult> {
    let bytes = std::fs::read(path).ok()?;
    let _ = std::fs::remove_file(path);
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let exit_code = value
        .get("exit_code")
        .and_then(|v| v.as_i64())
        .and_then(|v| i32::try_from(v).ok())?;
    let failed_phase = value
        .get("failed_phase")
        .and_then(|v| v.as_i64())
        .and_then(|v| i32::try_from(v).ok())
        .and_then(auto_complete::Phase::from_index);
    let escalation = value.get("escalation").and_then(|v| {
        let kind = match v.get("kind").and_then(|k| k.as_str())? {
            "merge-decision" => auto_complete::EscalationKind::MergeDecision,
            "design-fork" => auto_complete::EscalationKind::DesignFork,
            "supervised-merge" => auto_complete::EscalationKind::SupervisedMerge,
            _ => return None,
        };
        Some(auto_complete::EscalationSummary {
            kind,
            reason: v
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("pipelined child escalated")
                .to_string(),
        })
    });
    let shelved_reason = value
        .get("shelved")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        .then(|| aida_core::FailureReason {
            phase: failed_phase
                .unwrap_or(auto_complete::Phase::Ci)
                .slug()
                .to_string(),
            phase_index: failed_phase
                .unwrap_or(auto_complete::Phase::Ci)
                .index()
                .try_into()
                .unwrap_or(2),
            kind: "pipelined-child-shelved".to_string(),
            detail: "pipelined child parked this spec".to_string(),
            recovery_hint: Some(
                "inspect the child drain output and `aida findings list`".to_string(),
            ),
            shelved_by: Some("orchestrator".to_string()),
            shelved_at: chrono::Utc::now(),
        });
    Some(auto_complete::OrchestrationResult {
        exit_code,
        failed_phase,
        failure: None,
        phase_durations: Vec::new(),
        total_ms: 0,
        punt_reason: value
            .get("punt_reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        shipped_spec_id: value
            .get("shipped_spec_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        escalation,
        inconclusive_reason: value
            .get("inconclusive_reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        shelved_reason,
        held_reason: value
            .get("held_reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

/// BUG-438: should a finished single-spec drain clear its `drain-state.json`?
/// Clear when this run owns the file AND it is not a *failed resume* — a resume
/// that failed keeps the checkpoint so the operator can fix the blocker and
/// `--resume-drain` again, rather than the checkpoint being consumed on
/// failure. A successful resume (work done) and any non-resume drain clear as
/// before. Pure so the rule is unit-pinned. trace:BUG-438 | ai:claude
pub(crate) fn should_clear_drain_state(
    owns_drain_state: bool,
    is_resume: bool,
    exit_code: i32,
) -> bool {
    owns_drain_state && !(is_resume && exit_code != 0)
}

#[cfg(test)]
#[path = "tests/drain_state_clear_tests.rs"]
mod drain_state_clear_tests;

/// Resolve the queued members of a `batch:NAME` tag in pickup order, filtered
/// to the active role. Terminal items (Completed/Rejected) are dropped — they
/// are already shipped. Returns `(entry, display_id, title, status)` tuples.
/// Shared by the plain `--batch` head-pickup path and the TASK-285
/// `--batch --auto-complete` drain. trace:TASK-229, TASK-285 | ai:claude
pub(crate) fn resolve_batch_members(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    role: Option<&str>,
) -> Result<
    Vec<(
        aida_core::QueueEntry,
        String,
        String,
        aida_core::RequirementStatus,
    )>,
> {
    let project_root = find_project_root().ok();
    resolve_batch_members_with_context(
        storage,
        user_id,
        batch_name,
        role,
        project_root.as_deref(),
        &mut std::io::stderr(),
    )
}

/// Return queued members carrying `batch:NAME` that the autonomous drain will
/// not pick up, preserving the same reason label shown by queue diagnostics.
// trace:BUG-1422 | ai:codex
pub(crate) fn resolve_batch_ineligible_members(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    role: Option<&str>,
) -> Result<Vec<events::IneligibleBatchMember>> {
    let store = storage.load()?;
    let want = format!("batch:{batch_name}");
    let session_role = std::env::var("AIDA_SESSION_ROLE").ok();
    let (role_filter, _only_unrouted) =
        resolve_queue_role_filter(role, false, session_role.as_deref());
    let entries = queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        role_filter.as_deref(),
        false,
    )?;
    let mut ineligible = Vec::new();
    for entry in entries {
        if !entry_matches_role_filter(entry.for_role.as_deref(), role_filter.as_deref(), false) {
            continue;
        }
        let Some(req) = storage.resolve_queued_requirement(&entry.requirement_id)? else {
            continue;
        };
        if !req.tags.iter().any(|tag| tag.eq_ignore_ascii_case(&want)) {
            continue;
        }
        if let Some(reason) = queue_cmd::queue_fresh_pickup_reason_label(
            &queue_cmd::queue_drain_pickup_policy(&req, &store, false, storage.path().parent()),
        ) {
            ineligible.push(events::IneligibleBatchMember {
                spec: req.display_id(),
                reason,
            });
        }
    }
    Ok(ineligible)
}

/// TASK-1456 (follow-up to BUG-1515): the `next N` / bare `--auto-complete`
/// drain's candidate scan (`auto_complete_head_candidates`) only asks "is
/// this Approved/Planned" — `Done` was never a drivable head status, refused
/// or not, so it is silently absent from that filter rather than reported.
/// A queue that is ENTIRELY Done specs carrying a still-live review refusal
/// therefore hit the generic "no drivable items in the queue — nothing to
/// drive" idle message with the refusals never named — the exact "relaunches
/// every few minutes and does nothing" symptom BUG-1515 measured, just
/// outside the `--batch` path BUG-1422 already covers via
/// `resolve_batch_ineligible_members`.
///
/// Scans the SAME role-routed queue `auto_complete_head_candidates_with_roles`
/// reads for this drain and returns the display_id + recovery hint
/// (`queue_fresh_pickup_reason_label`) for every member whose
/// `queue_drain_pickup_policy` resolves to `AwaitingRework` specifically —
/// PRIN-5: this reports ONLY the verdicts it can positively confirm are
/// outstanding-and-at-the-tip (`done_spec_outstanding_refusal`'s own
/// contract), never a guess about a Done spec whose verdict state it
/// couldn't read.
// trace:TASK-1456 | ai:claude
/// TASK-1456: the human-readable idle-drain message naming every all-refused
/// candidate and its recovery route — a pure formatter (no `eprintln!`
/// inside it) so the exact wording is assertable directly, without spinning
/// up a drain.
// trace:TASK-1456 | ai:claude
pub(crate) fn next_n_rework_idle_message(
    rework_needed: &[events::IneligibleBatchMember],
) -> String {
    let rendered = rework_needed
        .iter()
        .map(|member| format!("{} ({})", member.spec, member.reason))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "queue has {} item{} but none are eligible — every candidate needs \
         rework, not a fresh drive: {}",
        rework_needed.len(),
        if rework_needed.len() == 1 { "" } else { "s" },
        rendered,
    )
}

pub(crate) fn resolve_queue_rework_needed(
    storage: &Storage,
    user_id: &str,
    role_override: Option<&str>,
) -> Result<Vec<events::IneligibleBatchMember>> {
    let store = storage.load()?;
    let effective_role = queue_cmd::effective_auto_complete_role(role_override);
    let role_filter = Some(effective_role);
    let entries = queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        role_filter.as_deref(),
        false,
    )?;
    let mut rework = Vec::new();
    for entry in entries {
        if !entry_matches_role_filter(entry.for_role.as_deref(), role_filter.as_deref(), false) {
            continue;
        }
        let Some(req) = storage.resolve_queued_requirement(&entry.requirement_id)? else {
            continue;
        };
        let policy =
            queue_cmd::queue_drain_pickup_policy(&req, &store, false, storage.path().parent());
        if !matches!(policy, queue_cmd::QueueFreshPickup::AwaitingRework) {
            continue;
        }
        if let Some(reason) = queue_cmd::queue_fresh_pickup_reason_label(&policy) {
            rework.push(events::IneligibleBatchMember {
                spec: req.display_id(),
                reason,
            });
        }
    }
    Ok(rework)
}

/// Injectable shell around batch resolution so the missing-object diagnostic
/// and event contract can be regression-tested without changing process cwd.
// trace:BUG-1264 | ai:codex
pub(crate) fn resolve_batch_members_with_context(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    role: Option<&str>,
    project_root: Option<&std::path::Path>,
    diagnostic: &mut dyn std::io::Write,
) -> Result<
    Vec<(
        aida_core::QueueEntry,
        String,
        String,
        aida_core::RequirementStatus,
    )>,
> {
    let store = storage.load()?;
    let want = format!("batch:{}", batch_name);
    let session_role = std::env::var("AIDA_SESSION_ROLE").ok();
    let (role_filter, _only_unrouted) =
        resolve_queue_role_filter(role, false, session_role.as_deref());
    // Batch drains must see the same cross-user role-routed queue as ordinary
    // pickup. Otherwise an item filed in a person's queue for `implementer`
    // disappears when the drain identity is the synthetic role user.
    // trace:BUG-1264 | ai:codex
    let entries = queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        role_filter.as_deref(),
        false,
    )?;
    let mut members: Vec<(
        aida_core::QueueEntry,
        String,
        String,
        aida_core::RequirementStatus,
    )> = Vec::new();
    for entry in entries {
        if !entry_matches_role_filter(entry.for_role.as_deref(), role_filter.as_deref(), false) {
            continue;
        }
        // Resolve each queue UUID through the targeted backend path instead of
        // assuming the full-store projection contains an object at a path
        // derived from its current display id. Merge-gate aliases can leave the
        // authoritative YAML under the node-qualified/origin id.
        // trace:BUG-1264 | ai:codex
        let Some(req) = storage.resolve_queued_requirement(&entry.requirement_id)? else {
            let member = entry.requirement_id.to_string();
            let reason = "requirement id does not resolve to a stored object";
            let _ = writeln!(
                diagnostic,
                "  {} batch:{} — skipping unresolvable member {}: {}",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                batch_name,
                member,
                reason,
            );
            if let Some(root) = project_root {
                let (_, run_uuid) = drain_state::current_context(root);
                events::emit(
                    root,
                    &events::Event::new(
                        Some(member),
                        run_uuid,
                        events::EventKind::SpecSkipped {
                            reason: reason.to_string(),
                        },
                    ),
                );
            }
            continue;
        };
        if !req.tags.iter().any(|t| t.eq_ignore_ascii_case(&want)) {
            continue;
        }
        // BUG-1017: batch pickup shares the same fresh-pickup policy as
        // queue next, list, and explicit dry-runs. Done remains visible as
        // awaiting-merge work but is not re-drained from scratch. BUG-1120:
        // headless drains also skip guided/operator/decide members; those
        // modes require the keyboard path.
        if let Some(reason_label) = queue_cmd::queue_fresh_pickup_reason_label(
            &queue_cmd::queue_drain_pickup_policy(&req, &store, false, project_root),
        ) {
            eprintln!(
                "  {} batch:{} — skipping un-pickable member {} ({})",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                batch_name,
                req.display_id(),
                reason_label,
            );
            continue;
        }
        members.push((
            entry,
            req.display_id(),
            req.title.clone(),
            req.status.clone(),
        ));
    }
    Ok(members)
}

// TASK-1297: one routed-queue row, reduced to the two fields the "what did
// the `--batch` filter exclude" count needs — decoupled from `Storage` /
// `QueueEntry` so the pure derivation below can be pinned with a plain
// fixture instead of a live store.
// trace:TASK-1297 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct RoutedSpecSnapshot {
    pub(crate) tags: Vec<String>,
    pub(crate) status: aida_core::RequirementStatus,
}

// TASK-1297: pure count of "M other approved specs routed to this role,
// excluded from the batch by the `--batch` filter" — the number a mis-scoped
// `--batch` launcher silently drove past for hours (2026-09-19 incident: 3
// open batch members re-driven every wave while 16 approved, role-routed
// specs sat outside the filter). `want_tag` is the `batch:<name>` tag
// (case-insensitively) that marks batch membership; a snapshot carrying it
// is a member and is never counted, regardless of status — only an
// approved, non-member routed spec counts, matching the acceptance wording
// ("M other approved specs routed to role:X are not in this batch"). Pure so
// a fixture pins the derivation without a live queue/store.
// trace:TASK-1297 | ai:claude
pub(crate) fn count_routed_excluded_from_batch(
    snapshots: &[RoutedSpecSnapshot],
    want_tag: &str,
) -> usize {
    snapshots
        .iter()
        .filter(|s| {
            s.status == aida_core::RequirementStatus::Approved
                && !s.tags.iter().any(|t| t.eq_ignore_ascii_case(want_tag))
        })
        .count()
}

// TASK-1297: impure shell for `count_routed_excluded_from_batch` — reads the
// same role-routed queue `resolve_batch_members_with_context` reads
// (`queue_list_with_role_fallback` + `entry_matches_role_filter` +
// `Storage::resolve_queued_requirement`), so the count agrees with batch
// resolution under the BUG-1264 alias fallback (a member whose object lives
// under its origin_id alias resolves the same way here as it does for
// membership). Returns the count plus the resolved role label (for the
// human-facing line), `None` when role resolution fails. Best-effort: a
// queue-read failure surfaces as `Err` so the caller can skip the line
// rather than print a wrong number.
// trace:TASK-1297 | ai:claude
pub(crate) fn count_batch_excluded_routed_specs(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    role: Option<&str>,
) -> Result<(usize, Option<String>)> {
    let want = format!("batch:{}", batch_name);
    let session_role = std::env::var("AIDA_SESSION_ROLE").ok();
    let (role_filter, _only_unrouted) =
        resolve_queue_role_filter(role, false, session_role.as_deref());
    let entries = queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        role_filter.as_deref(),
        false,
    )?;
    let mut snapshots = Vec::with_capacity(entries.len());
    for entry in entries {
        if !entry_matches_role_filter(entry.for_role.as_deref(), role_filter.as_deref(), false) {
            continue;
        }
        if let Some(req) = storage.resolve_queued_requirement(&entry.requirement_id)? {
            snapshots.push(RoutedSpecSnapshot {
                tags: req.tags.into_iter().collect(),
                status: req.status,
            });
        }
    }
    Ok((
        count_routed_excluded_from_batch(&snapshots, &want),
        role_filter,
    ))
}

// TASK-1297: the operator-facing "M other approved specs routed to role:X
// are not in this batch" clause — `None` when `excluded == 0` so a
// non-filtering batch drain (or one that filtered nothing away) renders no
// extra noise, per acceptance.
// trace:TASK-1297 | ai:claude
pub(crate) fn batch_exclusion_clause(excluded: usize, role_label: Option<&str>) -> Option<String> {
    if excluded == 0 {
        return None;
    }
    let role_phrase = match role_label {
        Some(r) => format!("role:{r}"),
        None => "this role".to_string(),
    };
    let (plural, verb) = if excluded == 1 {
        ("", "is")
    } else {
        ("s", "are")
    };
    Some(format!(
        "{excluded} other approved spec{plural} routed to {role_phrase} {verb} not in this batch"
    ))
}

#[cfg(test)]
#[path = "tests/task_1297_batch_exclusion_tests.rs"]
mod task_1297_batch_exclusion_tests;

/// Real [`auto_complete::BatchDriver`] — re-resolves the `batch:NAME` head
/// against the live queue and runs each member's full `--auto-complete`
/// lifecycle. trace:TASK-285 | ai:claude
pub(crate) struct RealBatchDriver<'a> {
    pub(crate) storage: &'a Storage,
    pub(crate) user_id: String,
    pub(crate) batch_name: String,
    pub(crate) role: Option<String>,
    pub(crate) variant: auto_complete::AutoCompleteVariant,
    pub(crate) json: bool,
    pub(crate) permission_mode: Option<String>,
    /// STORY-263: headless mode, propagated to every member's
    /// `run_auto_complete`.
    pub(crate) no_human: Option<auto_complete::NoHumanMode>,
    /// STORY-306: advisor-escalation mode, propagated to every member.
    pub(crate) escalate_mode: auto_complete::EscalateMode,
    /// BUG-311: outer `--steal` flag, propagated to every member's phase-1
    /// subprocess so a dormant lease on the member's scope is cleared rather
    /// than blocking the drain with the canned "pass --steal" message.
    pub(crate) steal: bool,
    /// TASK-559: outer `--force-claim` flag, propagated to every member's
    /// phase-1 subprocess so ambiguous spec status can be claimed.
    pub(crate) force_claim: bool,
    /// STORY-281: outer `--allow-stale-base` flag, propagated to every
    /// member's phase-3 pre-flight check so the stale-base + overlap
    /// signal becomes a warning instead of a refusal.
    pub(crate) allow_stale_base: bool,
    /// STORY-429: outer `--no-auto-rebase` flag, propagated to every member.
    pub(crate) no_auto_rebase: bool,
    // TASK-966: project root + drain-start for the `--max-tokens` meter. `None`
    // when no token cap is active. trace:TASK-966 | ai:claude
    pub(crate) token_meter: Option<(std::path::PathBuf, std::time::SystemTime)>,
    /// STORY-1091: configured in-flight implementer/CI window.
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipeline_depth: usize,
    /// STORY-1091: child `aida queue work <spec> --auto-complete=through-ci`
    /// processes keyed by opaque scheduler handles.
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipelined_children: std::collections::HashMap<usize, std::process::Child>,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipelined_result_paths: std::collections::HashMap<usize, std::path::PathBuf>,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) next_pipelined_handle: usize,
}

impl auto_complete::BatchDriver for RealBatchDriver<'_> {
    // trace:BUG-1980 | ai:claude
    fn stop_requested(&mut self) -> bool {
        crate::drain_cmd::stop_requested_from_env()
    }

    fn next_head(&mut self) -> Option<String> {
        match resolve_batch_members(
            self.storage,
            &self.user_id,
            &self.batch_name,
            self.role.as_deref(),
        ) {
            Ok(members) => members.into_iter().next().map(|m| m.1),
            Err(e) => {
                // A store/queue read failure is not "batch drained" — surface
                // it and stop rather than reporting a false clean drain.
                eprintln!(
                    "{} could not resolve batch members: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    e
                );
                std::process::exit(1);
            }
        }
    }

    fn run_spec(&mut self, spec: &str) -> auto_complete::OrchestrationResult {
        // BUG-660: retry a transient/retryable per-spec failure (locked cache,
        // GH-API blip) through an exponential backoff instead of hammering or
        // immediately shelving — an unattended drive rides out a brief blip.
        // trace:BUG-660 | ai:claude
        drive_robustness::run_with_transient_backoff(|| {
            run_auto_complete(
                self.storage,
                &self.user_id,
                spec,
                self.variant,
                self.json,
                self.permission_mode.as_deref(),
                self.no_human,
                self.escalate_mode,
                // STORY-301: a batch / nextN member does not own the drain-state
                // file — the batch orchestrator created it.
                false,
                self.steal,
                self.force_claim,
                self.allow_stale_base,
                self.no_auto_rebase,
                None,
            )
        })
    }

    // TASK-966: cumulative reported tokens across this drain's headless logs.
    fn cumulative_tokens(&mut self) -> u64 {
        match &self.token_meter {
            Some((root, since)) => sum_headless_log_tokens(root, *since),
            None => 0,
        }
    }
}

/// The environment a pipelined batch child is spawned with.
///
/// A pure helper for the same reason `integrate::drive_args` is one: the env is
/// load-bearing and otherwise only assertable by spawning a real drain. The
/// borrow flag in particular is invisible in the argv, so a guardrail that only
/// reads arguments cannot see it go missing — which is how BUG-1568 shipped.
// trace:BUG-1568 | ai:claude
pub(crate) fn pipelined_child_env(result_path: &std::path::Path) -> Vec<(&'static str, String)> {
    vec![
        ("AIDA_PIPELINED_BATCH_CHILD", "1".to_string()),
        // The child is a `queue work --auto-complete` in its own right and
        // takes the drain lock at its top, while the parent batch drive still
        // holds it. Borrow rather than acquire. See the call sites.
        ("AIDA_DRAIN_BORROW", "1".to_string()),
        (
            "AIDA_PIPELINED_RESULT_PATH",
            result_path.display().to_string(),
        ),
    ]
}

/// BUG-1570: a pipelined child that exits with an unrecognised code and leaves
/// no result sidecar did NOT report a drive outcome for this spec — and may
/// never have reached the spec at all. A refused drain lock, a spawn failure
/// and a missing binary all land here identically.
///
/// Reporting that as the SPEC's CI phase failing is what let a drain-level lock
/// refusal spend 17.5 hours attributed to an innocent spec: the batch summary
/// read "drain stopped at BUG-1462 (phase 2 failed)", so every reader went to
/// debug BUG-1462, which had nothing wrong with it.
///
/// `FailureKind::Internal` is deliberate. This is an orchestrator-layer fault,
/// which makes it un-shelvable — the spec must not be parked NeedsAttention for
/// something it did not do, and a hint that routes to AIDA rather than to the
/// spec's CI is the correct destination.
// trace:BUG-1570 | ai:claude
pub(crate) fn pipelined_child_reported_nothing(
    spec: &str,
    code: Option<i32>,
) -> auto_complete::OrchestrationResult {
    let exit = code
        .map(|c| format!("exit code {c}"))
        .unwrap_or_else(|| "a signal".to_string());
    let mut result = auto_complete::OrchestrationResult::failed(auto_complete::Phase::Ci);
    result.failure = Some(auto_complete::PhaseFailure::of(
        auto_complete::FailureKind::Internal,
        format!(
            "the pipelined child for {spec} ended with {exit} without reporting a drive \
             outcome, so it may never have started {spec} at all — a refused drain lock, a \
             failed spawn and a missing binary are indistinguishable here. This is an \
             orchestrator-layer fault, NOT a phase failure of {spec}; read the child's own \
             drain log before treating {spec} as the problem."
        ),
    ));
    result
}

/// BUG-1570: decide what a finished pipelined child's exit MEANS.
///
/// Pure, and shared by both pipelined drivers, so the classification is pinned
/// in BOTH directions at one seam. That pairing is the point: a child that
/// reported NOTHING must become an orchestrator-layer `Internal` fault, and a
/// child that DID report a drive outcome — a real CI failure it already shelved
/// the spec for — must keep travelling the shelvable path. Fixing the first
/// without pinning the second would flatten every genuine shelve into
/// `Internal`, which is un-shelvable, and the batch drain would stop dead
/// instead of continuing past a red member.
///
/// `sidecar` is a closure because reading the sidecar CONSUMES the file; the
/// unreported arm must not touch it.
// trace:BUG-1570 | ai:claude
pub(crate) fn classify_pipelined_child_outcome(
    spec: &str,
    exit_code: Option<i32>,
    success: bool,
    sidecar: impl FnOnce() -> Option<auto_complete::OrchestrationResult>,
) -> auto_complete::OrchestrationResult {
    if success {
        return sidecar().unwrap_or_else(auto_complete::OrchestrationResult::ok);
    }
    if exit_code == Some(auto_complete::DRIVE_EXIT_SHELVED) {
        return sidecar().unwrap_or_else(|| {
            let mut result = auto_complete::OrchestrationResult::failed(auto_complete::Phase::Ci);
            result.shelved_reason = Some(aida_core::FailureReason {
                phase: "ci".to_string(),
                phase_index: 2,
                kind: "pipelined-child-shelved".to_string(),
                detail: "pipelined implementer/CI child parked this spec".to_string(),
                recovery_hint: Some(
                    "inspect the child drain output and `aida findings list`".to_string(),
                ),
                shelved_by: Some("orchestrator".to_string()),
                shelved_at: chrono::Utc::now(),
            });
            result
        });
    }
    pipelined_child_reported_nothing(spec, exit_code)
}

/// Shared argv builder for a pipelined batch / `nextN` child drain
/// (`aida queue work <spec> --auto-complete=<mode> ...`), used by both
/// [`RealBatchDriver`] and [`RealNextNDriver`].
///
/// The `--escalate-blocks` / `--escalate-defaults` pair is pushed only when
/// the resolved no-human mode is [`auto_complete::NoHumanMode::Both`],
/// mirroring the child's STORY-306 kickoff validation: the advisor tier
/// exists only in a fully-headless drain, and the child rejects the flags
/// anywhere else. Pushing them unconditionally made every interactive
/// batch / `nextN` child exit at validation before doing any work.
// trace:STORY-1091 trace:ADR-28 | ai:codex
// trace:BUG-1805 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn pipelined_child_common_args(
    spec: &str,
    mode: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    steal: bool,
    force_claim: bool,
    allow_stale_base: bool,
    no_auto_rebase: bool,
) -> Vec<String> {
    let mut args = vec![
        "queue".to_string(),
        "work".to_string(),
        spec.to_string(),
        format!("--auto-complete={}", mode.slug()),
    ];
    if json {
        args.push("--json".to_string());
    }
    if let Some(permission_mode) = permission_mode {
        args.push("--permission-mode".to_string());
        args.push(permission_mode.to_string());
    }
    if let Some(no_human) = no_human {
        args.push(format!("--no-human={}", no_human.slug()));
    }
    if no_human == Some(auto_complete::NoHumanMode::Both) {
        match escalate_mode {
            auto_complete::EscalateMode::Blocks => args.push("--escalate-blocks".to_string()),
            auto_complete::EscalateMode::Defaults => args.push("--escalate-defaults".to_string()),
        }
    }
    if steal {
        args.push("--steal".to_string());
    }
    if force_claim {
        args.push("--force-claim".to_string());
    }
    if allow_stale_base {
        args.push("--allow-stale-base".to_string());
    }
    if no_auto_rebase {
        args.push("--no-auto-rebase".to_string());
    }
    args
}

impl RealBatchDriver<'_> {
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    // trace:BUG-1805 | ai:claude
    pub(crate) fn child_common_args(
        &self,
        spec: &str,
        mode: auto_complete::AutoCompleteVariant,
    ) -> Vec<String> {
        pipelined_child_common_args(
            spec,
            mode,
            self.json,
            self.permission_mode.as_deref(),
            self.no_human,
            self.escalate_mode,
            self.steal,
            self.force_claim,
            self.allow_stale_base,
            self.no_auto_rebase,
        )
    }
}

impl auto_complete::PipelinedBatchDriver for RealBatchDriver<'_> {
    fn pipeline_depth(&self) -> usize {
        self.pipeline_depth
    }

    fn start_spec_through_ci(&mut self, spec: &str) -> auto_complete::PipelinedHandle {
        let handle = auto_complete::PipelinedHandle(self.next_pipelined_handle);
        self.next_pipelined_handle += 1;
        // trace:TASK-1603 | ai:codex
        // The child registers its run before bumping status. Pre-bumping here
        // creates an InProgress/no-lease window before it has ownership.
        let exe = aida_exe_path();
        let result_path = find_main_worktree_root()
            .unwrap_or_else(|_| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            })
            .join(".aida")
            .join("pipelined-results")
            .join(format!("{}-{}.json", spec, handle.0));
        let mut cmd = std::process::Command::new(exe);
        // BUG-1568: the child takes the drain lock at its top while the parent
        // batch drive still holds it, so it must BORROW. Without that it is
        // refused with "a drain is already running (pid <parent>)", the batch
        // driver reads the refusal as a phase failure of the member, and the
        // whole batch stops having shipped nothing.
        //
        // This is BUG-748's defect in a second place: that fix taught the
        // INTEGRATOR child to borrow, and the pipelined batch child never got
        // the same treatment. It only became reachable when pipeline_depth was
        // raised from 1 to 2 — at depth 1 no child overlaps a live parent lock.
        // Measured after that change: 221 consecutive drains shipping nothing
        // over 17.5 hours. trace:BUG-1568 trace:BUG-748 | ai:claude
        cmd.args(self.child_common_args(spec, auto_complete::AutoCompleteVariant::ThroughCi));
        for (k, v) in pipelined_child_env(&result_path) {
            cmd.env(k, v);
        }
        match cmd.spawn_retrying_etxtbsy() {
            Ok(child) => {
                self.pipelined_children.insert(handle.0, child);
                self.pipelined_result_paths.insert(handle.0, result_path);
                handle
            }
            Err(e) => {
                eprintln!(
                    "{} could not start pipelined implementer for {}: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    spec,
                    e
                );
                handle
            }
        }
    }

    fn wait_spec_through_ci(
        &mut self,
        handle: auto_complete::PipelinedHandle,
    ) -> auto_complete::OrchestrationResult {
        let Some(mut child) = self.pipelined_children.remove(&handle.0) else {
            return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Implementer);
        };
        let result_path = self.pipelined_result_paths.remove(&handle.0);
        // The sidecar is named `<spec>-<handle>.json`, and the handle is
        // numeric, so the last `-` separates them. Spec ids contain `-`
        // themselves (BUG-1462), which is why this splits from the RIGHT.
        // trace:BUG-1570 | ai:claude
        let spec_for_report = result_path
            .as_deref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit_once('-').map(|(spec, _)| spec.to_string()))
            .unwrap_or_else(|| "the dispatched spec".to_string());
        let read_sidecar = || {
            result_path
                .as_deref()
                .and_then(read_pipelined_child_result_sidecar)
        };
        match child.wait() {
            Ok(status) => classify_pipelined_child_outcome(
                &spec_for_report,
                status.code(),
                status.success(),
                read_sidecar,
            ),
            // `wait()` itself failed, so there is no status to read at all.
            Err(_) => classify_pipelined_child_outcome(&spec_for_report, None, false, read_sidecar),
        }
    }

    fn finish_spec_after_ci(&mut self, spec: &str) -> auto_complete::OrchestrationResult {
        if i32::from(self.variant.last_phase()) <= auto_complete::Phase::Ci.index() {
            return auto_complete::OrchestrationResult::ok();
        }
        let project_root = match find_main_worktree_root() {
            Ok(root) => root,
            Err(_) => {
                return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Reviewer)
            }
        };
        let lookup = detect_open_pr_for_spec_via_forge(&project_root, spec);
        let (branch, pr) = match lookup {
            PrLookup::Found(pr) => (pr.head_branch, Some(pr.number as u32)),
            _ => return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Reviewer),
        };
        let resume = Some(ResumeEntry {
            // Re-probe phase 2 in this process so the reviewer carries a
            // head-bound CI conclusion, even if the pipelined child already
            // observed green. trace:BUG-1460 | ai:codex
            start_phase: auto_complete::Phase::Ci,
            branch,
            pr,
            head_sha: None,
            from_pr: true,
            ci_terminal_sha: None,
            ci_terminal_green: None,
        });
        run_auto_complete(
            self.storage,
            &self.user_id,
            spec,
            self.variant,
            self.json,
            self.permission_mode.as_deref(),
            self.no_human,
            self.escalate_mode,
            false,
            self.steal,
            self.force_claim,
            self.allow_stale_base,
            self.no_auto_rebase,
            resume,
        )
    }
}

/// Entry point for `aida queue work --batch NAME --auto-complete` (TASK-285).
/// Drains the whole batch. Depth 1 keeps the historical one-full-lifecycle per
/// member loop; depth >1 starts later implementer/CI children while this parent
/// serializes reviewer/merge/pull/build for ready PRs. Never returns.
// trace:TASK-285 STORY-1091
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_auto_complete_batch(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    role: Option<&str>,
    max: Option<usize>,
    // EPIC-28: outer `--max-failures` cap — after this many phase failures
    // shelve in a single batch, stop the drain entirely (the env is
    // probably broken). `None` falls back to the built-in default (5).
    // trace:EPIC-28 | ai:claude
    max_failures: Option<usize>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    // BUG-311: outer `--steal`, threaded to every batch member's phase 1.
    steal: bool,
    // TASK-559: outer `--force-claim`, threaded to every batch member's phase 1.
    force_claim: bool,
    // STORY-281: outer `--allow-stale-base`, threaded to every batch
    // member's phase 3 pre-flight stale-base check.
    allow_stale_base: bool,
    // STORY-429: outer `--no-auto-rebase`, threaded to every batch member.
    no_auto_rebase: bool,
    // TASK-966: hard budget caps for the whole drain. trace:TASK-966 | ai:claude
    caps: &drain_caps::DrainCaps,
) -> ! {
    // TASK-1297: resolve the batch head AND the "M other approved specs
    // routed to this role are excluded" count up front — both read the same
    // role-routed queue, so the start-up line, the drain-state file, and the
    // closing summary all agree on one snapshot. Best-effort: a resolution
    // failure just means the line/state don't render, not a drain abort.
    // trace:TASK-1297 | ai:claude
    let batch_members = resolve_batch_members(storage, user_id, batch_name, role).ok();
    let batch_member_count = batch_members.as_ref().map(Vec::len).unwrap_or(0);
    let excluded = count_batch_excluded_routed_specs(storage, user_id, batch_name, role).ok();
    let (excluded_count, excluded_role) = excluded.unwrap_or((0, None));

    if !json {
        eprintln!();
        eprintln!(
            "{} {} {}",
            "🚀".bold(),
            format!("auto-complete batch: batch:{batch_name}").bold(),
            format!("({})", variant.describe()).dimmed()
        );
        if let Some(limit) = max {
            eprintln!(
                "  {} drain capped at {} item{}",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                limit,
                if limit == 1 { "" } else { "s" }
            );
        }
        // TASK-1297: the filter is doing what it was asked — the SILENCE
        // about what it excluded was the defect. State both numbers, unless
        // there is nothing to declare (M == 0). trace:TASK-1297 | ai:claude
        if let Some(clause) = batch_exclusion_clause(excluded_count, excluded_role.as_deref()) {
            eprintln!(
                "  {} batch:{batch_name}: {batch_member_count} member{}; {clause}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                if batch_member_count == 1 { "" } else { "s" },
            );
        }
    }

    // STORY-301: write the drain-state file so `aida drain status` can show
    // the batch, its members, the current position, and what happens on exit.
    // Best-effort — a resolution / write failure leaves the drain running,
    // just unobservable. trace:STORY-301 | ai:claude
    let drain_root = find_main_worktree_root().ok();
    if let Some(root) = &drain_root {
        if let Some(members) = &batch_members {
            let specs: Vec<String> = members.iter().map(|m| m.1.clone()).collect();
            let pipeline_depth = DrainTuning::resolve(root).pipeline_depth();
            let _ = drain_state::DrainState::new_batch(batch_name, &specs)
                .with_pipeline_depth(pipeline_depth)
                // TASK-1297: carry the exclusion count so `aida drain status`
                // can show it while the drain is live. trace:TASK-1297
                .with_excluded_from_batch(excluded_count)
                .write(root);
        }
    }

    // TASK-967: drain origin (wall clock + base HEAD) for the exit summary.
    // trace:TASK-967 | ai:claude
    let drain_started = std::time::SystemTime::now();
    let drain_invocation = last_drain::DrainInvocation::capture();
    let drain_clock = std::time::Instant::now();
    let drain_base_sha = drain_root.as_deref().and_then(current_branch_head_sha);
    // TASK-966: arm the token meter only when a `--max-tokens` cap is set.
    let token_meter = caps
        .max_tokens
        .and(drain_root.clone())
        .map(|root| (root, drain_started));
    let pipeline_depth = drain_root
        .as_deref()
        .map(|root| DrainTuning::resolve(root).pipeline_depth())
        .unwrap_or_else(drain_state::default_pipeline_depth);
    let mut driver = RealBatchDriver {
        storage,
        user_id: user_id.to_string(),
        batch_name: batch_name.to_string(),
        role: role.map(|s| s.to_string()),
        variant,
        json,
        permission_mode: permission_mode.map(|s| s.to_string()),
        no_human,
        escalate_mode,
        steal,
        force_claim,
        allow_stale_base,
        no_auto_rebase,
        token_meter,
        pipeline_depth,
        pipelined_children: std::collections::HashMap::new(),
        pipelined_result_paths: std::collections::HashMap::new(),
        next_pipelined_handle: 1,
    };
    // EPIC-28: a `None` `--max-failures` from the CLI means "use the
    // built-in default cap" — set here so the orchestrator never runs
    // with an unbounded failure budget. trace:EPIC-28 | ai:claude
    let max_failures = max_failures.or(Some(DEFAULT_MAX_FAILURES));
    // TASK-966: thread the hard budget caps through the batch drain.
    let mut cap_stop = None;
    let result = auto_complete::drain_batch_pipelined_with_caps(
        &mut driver,
        max,
        max_failures,
        caps,
        drain_clock,
        &mut cap_stop,
    );
    // BUG-1422: re-resolve the members the drain refuses so an empty eligible
    // set is distinguishable from a genuinely completed batch.
    // trace:BUG-1422 | ai:codex
    let ineligible =
        resolve_batch_ineligible_members(storage, user_id, batch_name, role).unwrap_or_default();
    // An empty batch (nothing shipped, nothing punted, nothing to drain) is a
    // user error — the named batch tag matched no queued work. Surface it with
    // a non-zero exit so scripts notice, even though `drain_batch` calls it
    // `Drained`. A batch where every member punted is *not* empty (STORY-276).
    let exit_code = if let Some(stop) = &cap_stop {
        emit_drain_cap_stop(stop, json);
        DRAIN_CAP_EXIT_CODE
    } else if matches!(result.outcome, auto_complete::BatchDrainOutcome::Drained)
        && result.shipped.is_empty()
        && result.punted.is_empty()
        && result.escalated.is_empty()
    {
        1
    } else {
        result.exit_code
    };
    // TASK-1297: re-resolve the exclusion count for the closing summary — the
    // queue can have moved during the drain (more work approved, or this
    // drain's own shipped members left the queue), so the closing number
    // answers "how much is still sitting outside this batch right now" rather
    // than repeating the stale start-of-drain figure. Best-effort.
    // trace:TASK-1297 | ai:claude
    let (closing_excluded_count, closing_excluded_role) =
        count_batch_excluded_routed_specs(storage, user_id, batch_name, role)
            .ok()
            .unwrap_or((excluded_count, excluded_role));
    emit_batch_drain_summary(
        batch_name,
        &result,
        exit_code,
        json,
        closing_excluded_count,
        closing_excluded_role.as_deref(),
        &ineligible,
    );
    // TASK-967: permanent exit summary + cost-per-drain telemetry.
    finalize_drain_summary(
        "batch",
        format!("batch:{batch_name}"),
        &result.outcome,
        cap_stop.as_ref(),
        drain_summary::DrainTallies {
            shipped: result.shipped.len(),
            shelved: result.shelved.len(),
            skipped: result.skipped.len(),
            punted: result.punted.len(),
            escalated: result.escalated.len(),
        },
        drain_root.as_deref(),
        drain_base_sha.as_deref(),
        drain_started,
        drain_clock.elapsed(),
        &drain_invocation,
        json,
        // TASK-1297: the "M other approved specs routed to this role are not
        // in this batch" figure, echoed onto the terminal QueueDrained event
        // so a monitor can alarm on it. trace:TASK-1297 | ai:claude
        Some(closing_excluded_count),
        &ineligible,
    );
    // STORY-493: at drain-end, durably digest any mailbox traffic the drain
    // produced into the git-canonical orphan store. Best-effort + non-fatal —
    // never let a digest failure change the drain's exit code. trace:STORY-493
    if let Some(root) = &drain_root {
        maybe_digest_mailbox_best_effort(&root.join(".aida-store"), "drain-end");
    }
    // STORY-301: clean batch exit removes the drain-state file. A crash mid-
    // drain skips this, leaving the file for `aida drain status` to flag stale.
    if let Some(root) = &drain_root {
        let _ = drain_state::DrainState::clear(root);
    }
    std::process::exit(exit_code);
}

/// Real [`auto_complete::SingleBranchDriver`] (TASK-1003 / SPIKE-70) — drives a
/// coupled batch on ONE shared branch in ONE worktree. `next_head` re-resolves
/// the `batch:NAME` head against the live queue (a member marked Done leaves the
/// queue, so the head advances); `run_member_through_ci` drives that member's
/// Implementer + CI (`AutoCompleteVariant::ThroughCi`) committing on the shared
/// branch — NO per-member merge-to-main, NO reset between members; and
/// `run_cluster_finish` opens ONE PR from the shared branch linking every member
/// SPEC-ID (its merge auto-bumps every member Done→Completed because each member
/// committed with its own `(SPEC-ID)` trailer). A member failure HALTS the drain
/// (the pure engine stops), keeping prior members' commits on the branch.
///
/// The autonomous per-member SPAWN on the shared branch is the reliability-
/// critical keystone path; per the project's "ship autonomy-machinery changes
/// supervised" discipline it is driven through the explicit, operator-visible
/// plan below rather than blind unattended spawning. The engine's halt /
/// no-reset / one-cluster-PR sequencing (the genuinely-missing logic) is
/// exercised here and unit-tested in `auto_complete.rs`. trace:TASK-1003 SPIKE-70 | ai:claude
pub(crate) struct RealSingleBranchDriver<'a> {
    pub(crate) storage: &'a Storage,
    pub(crate) user_id: String,
    pub(crate) batch_name: String,
    pub(crate) role: Option<String>,
    pub(crate) json: bool,
    /// The shared feature branch every member commits onto.
    pub(crate) shared_branch: String,
    /// Members announced/driven so far, in order — drives the plan output and is
    /// the witness that the head advances exactly once per member.
    pub(crate) driven: Vec<String>,
    /// The repo the shared branch lives in — where the ONE cluster PR opens.
    /// `None` when the main worktree could not be resolved (the cluster step
    /// then prints its plan instead of opening anything).
    // trace:TASK-1136 | ai:claude
    pub(crate) project_root: Option<std::path::PathBuf>,
    /// The number of the ONE cluster PR once it opened. `None` when nothing was
    /// on the branch to open a PR for.
    // trace:TASK-1136 | ai:claude
    pub(crate) cluster_pr: Option<u64>,
    /// The drain's headless mode, if any — decides whether the per-member
    /// checkpoint pauses for the operator or auto-continues.
    // trace:TASK-1137 | ai:claude
    pub(crate) no_human: Option<auto_complete::NoHumanMode>,
    /// Whether this process can actually ask a question and get an answer (a
    /// terminal on BOTH stdin and stdout). Captured once at construction so the
    /// checkpoint never blocks on a prompt nobody can answer.
    // trace:TASK-1137 | ai:claude
    pub(crate) interactive: bool,
}

impl auto_complete::SingleBranchDriver for RealSingleBranchDriver<'_> {
    fn next_head(&mut self) -> Option<String> {
        match resolve_batch_members(
            self.storage,
            &self.user_id,
            &self.batch_name,
            self.role.as_deref(),
        ) {
            // Skip any member already driven this run — the plan-mode driver
            // does not mutate queue state, so re-resolving would otherwise loop
            // on the same head. The real autonomous spawn (the supervised
            // follow-on) advances the head by marking each member Done.
            Ok(members) => members
                .into_iter()
                .map(|m| m.1)
                .find(|id| !self.driven.iter().any(|d| d == id)),
            Err(e) => {
                eprintln!(
                    "{} could not resolve batch members: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    e
                );
                std::process::exit(1);
            }
        }
    }

    fn run_member_through_ci(&mut self, spec: &str) -> auto_complete::OrchestrationResult {
        let n = self.driven.len() + 1;
        if !self.json {
            eprintln!(
                "  {} member {}: implement + CI on {} (commit in place, no merge, no reset)",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                n,
                self.shared_branch.cyan(),
            );
        }
        self.driven.push(spec.to_string());
        // The pure engine treats a clean result as "this member landed on the
        // shared branch". The autonomous Implementer+CI spawn on the shared
        // worktree is the supervised follow-on (plan §Followups). trace:TASK-1003
        auto_complete::OrchestrationResult::ok()
    }

    fn run_cluster_finish(&mut self, members: &[String]) -> auto_complete::OrchestrationResult {
        // ONE pull request for the whole cluster. Its title carries EVERY
        // member id in the trailing group the squash subject preserves, and its
        // body lists them under `## Covers` — so the single merge completes all
        // N through the ordinary Done→Completed scan rather than crediting only
        // whichever id reached the subject. trace:TASK-1136 | ai:claude
        let title = pr_ship::cluster_pr_title(&self.batch_name, members);
        let body = pr_ship::cluster_pr_body(&self.batch_name, &self.shared_branch, members);
        if !self.json {
            eprintln!(
                "  {} cluster: ONE PR from {} linking {} member{} → review + merge once \
                 (the one merge completes every linked member)",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                self.shared_branch.cyan(),
                members.len(),
                if members.len() == 1 { "" } else { "s" },
            );
            for (i, id) in members.iter().enumerate() {
                eprintln!("     {:>2}. {}", i + 1, id.bold());
            }
        }
        // Nothing accumulated on the shared branch yet (the per-member
        // Implementer+CI spawn is the supervised follow-on) means there is
        // nothing to open a PR for — surface the exact cluster PR that WILL be
        // opened instead of failing the wrap-up on an empty branch.
        let ready = self
            .project_root
            .as_deref()
            .map(|root| shared_branch_has_cluster_commits(root, &self.shared_branch))
            .unwrap_or(false);
        if !ready {
            if !self.json {
                eprintln!(
                    "     {} {} {}",
                    "title:".dimmed(),
                    title,
                    "(opens once the members' commits are on the branch)".dimmed(),
                );
            }
            return auto_complete::OrchestrationResult::ok();
        }
        let root = self
            .project_root
            .clone()
            .expect("ready implies a resolved project root");
        match open_cluster_pr(&root, &self.shared_branch, &title, &body) {
            Ok(number) => {
                self.cluster_pr = Some(number);
                auto_complete::OrchestrationResult::ok()
            }
            Err(e) => {
                eprintln!(
                    "{} could not open the cluster pull request: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    e
                );
                auto_complete::OrchestrationResult::failed(auto_complete::Phase::Merge)
            }
        }
    }

    /// Per-member checkpoint. In a single-branch drain each member commits ON
    /// TOP of the last one, so a bad increment poisons everything after it —
    /// with a human at the keyboard we stop and let them inspect what just
    /// landed before the next member stacks on it; fully headless we continue,
    /// because a prompt with nobody to answer it would hang the drain forever.
    // trace:TASK-1137 | ai:claude
    fn checkpoint_between_members(&mut self, prev: &str, next: &str) -> bool {
        match auto_complete::single_branch_checkpoint_action(self.no_human, self.interactive) {
            auto_complete::CheckpointAction::AutoContinue => {
                if !self.json {
                    eprintln!(
                        "  {} checkpoint: {} landed on {} — continuing with {} (nobody at the \
                         keyboard to review it)",
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                        prev.bold(),
                        self.shared_branch.cyan(),
                        next.bold(),
                    );
                }
                true
            }
            auto_complete::CheckpointAction::Prompt => {
                eprintln!();
                eprintln!(
                    "  {} checkpoint: {} landed on {} ({} member{} on the branch so far)",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    prev.bold(),
                    self.shared_branch.cyan(),
                    self.driven.len(),
                    if self.driven.len() == 1 { "" } else { "s" },
                );
                eprintln!(
                    "     {} inspect it now — {} commits on top of it and later members \
                     inherit anything wrong with it",
                    "→".dimmed(),
                    next.bold(),
                );
                // Declining is a CLEAN stop: the engine parks the drain with
                // every prior member's commit intact on the shared branch. A
                // read failure is treated the same way — never proceed to stack
                // more work on an increment nobody consented to.
                match prompt_yes_no(&format!("  Continue with {next}? [Y/n] "), true) {
                    Ok(go) => {
                        if !go {
                            eprintln!(
                                "  {} stopping here — the branch keeps every member committed \
                                 so far",
                                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                            );
                        }
                        go
                    }
                    Err(_) => false,
                }
            }
        }
    }
}

/// True when the shared single-branch drain branch exists locally AND carries
/// commits the default branch does not — i.e. there is a real cluster to open a
/// pull request for. A plan-mode run (no member actually spawned) leaves the
/// branch absent or empty, and opening a PR then would fail with "no commits
/// between".
// trace:TASK-1136 | ai:claude
pub(crate) fn shared_branch_has_cluster_commits(
    project_root: &std::path::Path,
    branch: &str,
) -> bool {
    let local_exists = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !local_exists {
        return false;
    }
    let base = crate::forge::default_branch_of(project_root);
    branch_ahead_of(project_root, branch, &format!("origin/{base}"))
        .or_else(|| branch_ahead_of(project_root, branch, &base))
        .is_some_and(|ahead| ahead > 0)
}

/// Push the shared branch and open the ONE cluster pull request through the
/// forge — the same `open_change` path a per-spec PR rides, differing only in
/// that the title/body name every member instead of one. Returns the change
/// number.
// trace:TASK-1136 | ai:claude
pub(crate) fn open_cluster_pr(
    project_root: &std::path::Path,
    branch: &str,
    title: &str,
    body: &str,
) -> Result<u64> {
    let push = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["push", "-u", "origin", branch])
        .status()
        .context("could not invoke `git push`")?;
    if !push.success() {
        anyhow::bail!("`git push -u origin {branch}` failed — investigate before retrying");
    }
    let change = crate::forge::forge_for_open_change(project_root)
        .open_change(crate::forge::OpenChange {
            branch: branch.to_string(),
            base: crate::forge::default_branch_of(project_root),
            title: title.to_string(),
            body: body.to_string(),
            draft: false,
        })
        .context("could not open the cluster pull request")?;
    if change.id == 0 {
        anyhow::bail!(
            "the cluster pull request opened but no number was found in its output: {}",
            change.url
        );
    }
    Ok(change.id)
}

/// Entry point for `aida queue work --batch NAME --auto-complete --single-branch`
/// (TASK-1003 / SPIKE-70). Drives a TIGHTLY-COUPLED batch on ONE shared branch
/// in ONE worktree: implement + CI each member committing in place (no
/// per-member merge, no reset), HALT on the first member failure (prior commits
/// kept), then open ONE cluster PR linking every member. Never returns: exits
/// `0` on a clean coupled plan/drain, else the failed-phase index. trace:TASK-1003
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_auto_complete_single_branch(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    role: Option<&str>,
    max: Option<usize>,
    // TASK-1137: decides the per-member checkpoint — a human at the keyboard
    // validates each increment before the next stacks on it; a fully headless
    // drain continues without asking. trace:TASK-1137 | ai:claude
    no_human: Option<auto_complete::NoHumanMode>,
) -> ! {
    // Resolve the coupled member set up front so the plan is concrete.
    let members = match resolve_batch_members(storage, user_id, batch_name, role) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "{} could not resolve batch members: {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                e
            );
            std::process::exit(1);
        }
    };
    if members.is_empty() {
        eprintln!(
            "{} no queued items tagged `batch:{}` — tag the coupled members via \
             `aida edit <id> --tags batch:{}` first",
            crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
            batch_name,
            batch_name,
        );
        std::process::exit(1);
    }

    // Derive the ONE shared branch every member accumulates onto.
    let drain_root = find_main_worktree_root().ok();
    let short_sha = drain_root
        .as_deref()
        .and_then(current_branch_head_sha)
        .map(|s| s.chars().take(7).collect::<String>())
        .unwrap_or_else(|| "base".to_string());
    let shared_branch = format!("single-branch/{batch_name}-{short_sha}");

    if !json {
        eprintln!();
        eprintln!(
            "{} {} {}",
            "🚀".bold(),
            format!("coupled single-branch drain: batch:{batch_name}").bold(),
            format!("({})", variant.describe()).dimmed(),
        );
        eprintln!(
            "  {} ONE shared branch {} in ONE worktree → ONE cluster PR; \
             commit-per-member, halt-on-failure (prior commits kept)",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            shared_branch.cyan(),
        );
        eprintln!(
            "  {} coupled members, in order:",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
        );
        for (i, (_, display_id, title, status)) in members.iter().enumerate() {
            eprintln!(
                "     {:>2}. {} [{}] {}",
                i + 1,
                display_id.bold(),
                format!("{}", status).dimmed(),
                title,
            );
        }
    }

    let mut driver = RealSingleBranchDriver {
        storage,
        user_id: user_id.to_string(),
        batch_name: batch_name.to_string(),
        role: role.map(|s| s.to_string()),
        json,
        shared_branch: shared_branch.clone(),
        driven: Vec::new(),
        project_root: drain_root.clone(),
        cluster_pr: None,
        no_human,
        // A prompt needs a terminal on BOTH ends to be answerable — and `--json`
        // opts out too, since the prompt writes to stdout and would corrupt the
        // machine-readable stream its consumer is parsing.
        // trace:TASK-1137 | ai:claude
        interactive: !json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
    };
    let result = auto_complete::drain_batch_single_branch(&mut driver, max);
    let cluster_pr = driver.cluster_pr;

    if !json {
        eprintln!();
        match &result.outcome {
            auto_complete::SingleBranchOutcome::Clustered => {
                eprintln!(
                    "{} coupled plan ready: {} member{} accumulate on {} → ONE cluster PR",
                    crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                    result.cluster_members.len(),
                    if result.cluster_members.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    shared_branch.cyan(),
                );
                match cluster_pr {
                    // The one PR is open and links every member; its merge
                    // completes all of them in a single pass. trace:TASK-1136
                    Some(n) => eprintln!(
                        "  {} cluster pull request #{} links every member — review + merge it \
                         once and all {} complete together",
                        "→".cyan(),
                        n,
                        result.cluster_members.len(),
                    ),
                    // The autonomous Implementer+CI spawn on the shared worktree
                    // is the reliability-critical keystone path — drive it
                    // supervised, then the cluster PR opens over its commits.
                    None => eprintln!(
                        "  {} drive the coupled set on the shared branch supervised \
                         (`--zen`); the one cluster PR then opens over its commits",
                        "→".cyan(),
                    ),
                }
            }
            auto_complete::SingleBranchOutcome::Halted(_) => {
                eprintln!(
                    "{} halted on {} — {} prior member{} kept on the branch; \
                     the failed member is parked for triage (no cluster PR)",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    result.stopped_at.as_deref().unwrap_or("?"),
                    result.committed.len(),
                    if result.committed.len() == 1 { "" } else { "s" },
                );
            }
            // The per-member checkpoint stopped the drain — a clean exit, not a
            // failure. Prior members stay committed on the shared branch; the
            // same command resumes from the next member. trace:TASK-1137
            auto_complete::SingleBranchOutcome::Paused => {
                eprintln!(
                    "{} paused before {} — {} member{} committed on {} (no cluster pull \
                     request yet)",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan().bold(),
                    result.stopped_at.as_deref().unwrap_or("the next member"),
                    result.committed.len(),
                    if result.committed.len() == 1 { "" } else { "s" },
                    shared_branch.cyan(),
                );
                eprintln!(
                    "  {} re-run the same command to pick up from there once you are happy \
                     with what landed",
                    "→".cyan(),
                );
            }
            other => {
                eprintln!(
                    "  {} single-branch drain stopped: {:?}",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    other
                );
            }
        }
    }
    std::process::exit(result.exit_code);
}

/// Entry point for `aida queue work --batches A,B,C --auto-complete`
/// (TASK-310). Drains named batches left-to-right, exhausting one before
/// moving to the next. Empty batches are skipped; a failed phase or `--max`
/// cap stops the chain at the active batch.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_auto_complete_batches(
    storage: &Storage,
    user_id: &str,
    batch_names: &[String],
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    role: Option<&str>,
    max: Option<usize>,
    // EPIC-28: per-batch `--max-failures` cap — see `handle_auto_complete_batch`.
    // trace:EPIC-28 | ai:claude
    max_failures: Option<usize>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    steal: bool,
    force_claim: bool,
    // STORY-281: outer `--allow-stale-base`, threaded into every member's
    // RealBatchDriver via the per-batch closure below.
    allow_stale_base: bool,
    // STORY-429: outer `--no-auto-rebase`, threaded into every member.
    no_auto_rebase: bool,
    // TASK-966: hard budget caps for the whole chain. trace:TASK-966 | ai:claude
    caps: &drain_caps::DrainCaps,
) -> ! {
    if !json {
        eprintln!();
        eprintln!(
            "{} {} {}",
            "🚀".bold(),
            format!(
                "auto-complete batches: {}",
                batch_names
                    .iter()
                    .map(|b| format!("batch:{b}"))
                    .collect::<Vec<_>>()
                    .join(" → ")
            )
            .bold(),
            format!("({})", variant.describe()).dimmed()
        );
        if let Some(limit) = max {
            eprintln!(
                "  {} drain capped at {} item{} total",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                limit,
                if limit == 1 { "" } else { "s" }
            );
        }
    }

    let drain_root = find_main_worktree_root().ok();
    let mut batch_index = 0usize;
    // EPIC-28: same default as the single-batch path. trace:EPIC-28 | ai:claude
    let max_failures = max_failures.or(Some(DEFAULT_MAX_FAILURES));
    // TASK-966: a single drain-start + token meter shared across every batch in
    // the chain so `--max-runtime` / `--max-tokens` are cumulative.
    let chain_started = std::time::SystemTime::now();
    let drain_invocation = last_drain::DrainInvocation::capture();
    // TASK-967: drain-wide wall clock + base HEAD for the exit summary.
    // trace:TASK-967 | ai:claude
    let chain_clock = std::time::Instant::now();
    let drain_base_sha = drain_root.as_deref().and_then(current_branch_head_sha);
    let token_meter_root = caps.max_tokens.and(drain_root.clone());
    let mut cap_stop = None;
    let result = auto_complete::drain_batch_chain_with_caps(
        batch_names,
        max,
        max_failures,
        caps,
        chain_clock,
        &mut cap_stop,
        |batch_name| {
            batch_index += 1;
            let members_for_state = resolve_batch_members(storage, user_id, batch_name, role);
            if !json
                && members_for_state
                    .as_ref()
                    .map(|members| !members.is_empty())
                    .unwrap_or(true)
            {
                eprintln!(
                    "  {} drain: batch:{} ({}/{})",
                    "→".dimmed(),
                    batch_name.cyan(),
                    batch_index,
                    batch_names.len()
                );
            }
            if let Some(root) = &drain_root {
                if let Ok(members) = members_for_state {
                    let specs: Vec<String> = members.into_iter().map(|m| m.1).collect();
                    let pipeline_depth = DrainTuning::resolve(root).pipeline_depth();
                    // TASK-1297: per-sub-batch exclusion count, so `aida
                    // drain status` shows it live for whichever batch in the
                    // chain is currently running. trace:TASK-1297 | ai:claude
                    let excluded =
                        count_batch_excluded_routed_specs(storage, user_id, batch_name, role)
                            .map(|(count, _)| count)
                            .unwrap_or(0);
                    let _ = drain_state::DrainState::new_batch(batch_name, &specs)
                        .with_pipeline_depth(pipeline_depth)
                        .with_excluded_from_batch(excluded)
                        .write(root);
                }
            }
            Box::new(RealBatchDriver {
                storage,
                user_id: user_id.to_string(),
                batch_name: batch_name.to_string(),
                role: role.map(|s| s.to_string()),
                variant,
                json,
                permission_mode: permission_mode.map(|s| s.to_string()),
                no_human,
                escalate_mode,
                steal,
                force_claim,
                allow_stale_base,
                no_auto_rebase,
                // TASK-966: shared start + root → cumulative token meter.
                token_meter: token_meter_root.clone().map(|root| (root, chain_started)),
                pipeline_depth: drain_root
                    .as_deref()
                    .map(|root| DrainTuning::resolve(root).pipeline_depth())
                    .unwrap_or_else(drain_state::default_pipeline_depth),
                pipelined_children: std::collections::HashMap::new(),
                pipelined_result_paths: std::collections::HashMap::new(),
                next_pipelined_handle: 1,
            })
        },
    );

    let no_activity = result.shipped.is_empty()
        && result.punted.is_empty()
        && result.escalated.is_empty()
        && matches!(result.outcome, auto_complete::BatchDrainOutcome::Drained);
    let exit_code = if let Some(stop) = &cap_stop {
        emit_drain_cap_stop(stop, json);
        DRAIN_CAP_EXIT_CODE
    } else if no_activity {
        1
    } else {
        result.exit_code
    };
    emit_batch_chain_summary(batch_names, &result, exit_code, json);
    // TASK-967: permanent exit summary + cost-per-drain telemetry. The label is
    // the chained batches; the chain's wall clock + base HEAD bound the diff.
    finalize_drain_summary(
        "batch-chain",
        batch_names
            .iter()
            .map(|b| format!("batch:{b}"))
            .collect::<Vec<_>>()
            .join(" → "),
        &result.outcome,
        cap_stop.as_ref(),
        drain_summary::DrainTallies {
            shipped: result.shipped.len(),
            shelved: result.shelved.len(),
            skipped: result.skipped.len(),
            punted: result.punted.len(),
            escalated: result.escalated.len(),
        },
        drain_root.as_deref(),
        drain_base_sha.as_deref(),
        chain_started,
        chain_clock.elapsed(),
        &drain_invocation,
        json,
        // TASK-1297: the chained-batches path (`--batch a,b,c`) does not yet
        // surface the per-batch exclusion count — out of scope for this
        // fix, which targets the single-batch drain the 2026-09-19 incident
        // hit. trace:TASK-1297 | ai:claude
        // BUG-1425: the chain does not compute one aggregate exclusion count.
        // trace:BUG-1425 | ai:codex
        None,
        &[],
    );
    // STORY-493: same best-effort drain-end mailbox digest as the single-batch
    // path. Non-fatal — never affects the drain's exit code. trace:STORY-493
    if let Some(root) = &drain_root {
        maybe_digest_mailbox_best_effort(&root.join(".aida-store"), "drain-end");
    }
    if let Some(root) = &drain_root {
        let _ = drain_state::DrainState::clear(root);
    }
    std::process::exit(exit_code);
}

/// Print the closing summary for a multi-batch drain. Per-batch summaries are
/// intentionally compact: empty batches are omitted from human output so an
/// already-drained intermediate batch is a silent skip. trace:TASK-310
pub(crate) fn emit_batch_chain_summary(
    batch_names: &[String],
    result: &auto_complete::BatchChainDrainResult,
    exit_code: i32,
    json: bool,
) {
    use auto_complete::BatchDrainOutcome;

    if json {
        let outcome = match &result.outcome {
            BatchDrainOutcome::Drained => "drained",
            BatchDrainOutcome::MaxReached => "max-reached",
            BatchDrainOutcome::Failed(_) => "failed",
            BatchDrainOutcome::Stalled => "stalled",
            BatchDrainOutcome::Mismatched { .. } => "mismatched",
            BatchDrainOutcome::Inconclusive => "inconclusive",
            BatchDrainOutcome::Held => "held",
            // EPIC-28 trace:EPIC-28 | ai:claude
            BatchDrainOutcome::DrainedWithShelved => "drained-with-shelved",
        };
        let mut obj = serde_json::Map::new();
        obj.insert(
            "event".to_string(),
            serde_json::Value::String("batch-chain-drain".into()),
        );
        obj.insert(
            "batches".to_string(),
            serde_json::Value::Array(
                batch_names
                    .iter()
                    .map(|b| serde_json::Value::String(format!("batch:{b}")))
                    .collect(),
            ),
        );
        obj.insert(
            "outcome".to_string(),
            serde_json::Value::String(outcome.into()),
        );
        obj.insert(
            "shipped".to_string(),
            serde_json::Value::Array(
                result
                    .shipped
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "shipped_count".to_string(),
            serde_json::Value::Number((result.shipped.len() as u64).into()),
        );
        obj.insert(
            "punted".to_string(),
            serde_json::Value::Array(
                result
                    .punted
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "punted_count".to_string(),
            serde_json::Value::Number((result.punted.len() as u64).into()),
        );
        obj.insert(
            "escalated".to_string(),
            serde_json::Value::Array(
                result
                    .escalated
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "escalated_count".to_string(),
            serde_json::Value::Number((result.escalated.len() as u64).into()),
        );
        // EPIC-28: chain-wide shelved + skipped totals. trace:EPIC-28
        obj.insert(
            "shelved".to_string(),
            serde_json::Value::Array(
                result
                    .shelved
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "shelved_count".to_string(),
            serde_json::Value::Number((result.shelved.len() as u64).into()),
        );
        obj.insert(
            "skipped_count".to_string(),
            serde_json::Value::Number((result.skipped.len() as u64).into()),
        );
        obj.insert(
            "stopped_batch".to_string(),
            match &result.stopped_batch {
                Some(b) => serde_json::Value::String(format!("batch:{b}")),
                None => serde_json::Value::Null,
            },
        );
        obj.insert(
            "stopped_at".to_string(),
            match &result.stopped_at {
                Some(s) => serde_json::Value::String(s.clone()),
                None => serde_json::Value::Null,
            },
        );
        obj.insert(
            "exit_code".to_string(),
            serde_json::Value::Number(exit_code.into()),
        );
        println!("{}", serde_json::Value::Object(obj));
        return;
    }

    let shipped_list = if result.shipped.is_empty() {
        "(none)".to_string()
    } else {
        result.shipped.join(", ")
    };
    let n = result.shipped.len();
    let plural = if n == 1 { "" } else { "s" };
    eprintln!();

    if result.shipped.is_empty() && result.punted.is_empty() && result.escalated.is_empty() {
        eprintln!(
            "{} no queued items found in any requested batch: {}",
            crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
            batch_names
                .iter()
                .map(|b| format!("batch:{b}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        return;
    }

    match &result.outcome {
        BatchDrainOutcome::Drained => {
            eprintln!(
                "{} batch chain drained — {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
        BatchDrainOutcome::MaxReached => {
            eprintln!(
                "{} batch chain — `--max` reached, {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
            if let Some(batch) = &result.stopped_batch {
                eprintln!(
                    "  {} stopped while draining `batch:{batch}` — re-run the same command to continue",
                    "→".dimmed()
                );
            }
        }
        BatchDrainOutcome::Failed(phase) => {
            let batch = result.stopped_batch.as_deref().unwrap_or("<batch>");
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch chain stopped in `batch:{batch}` at {} (phase {} failed)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold(),
                phase.index()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} later batches were not started — fix {stopped}, then re-run the chain",
                "→".dimmed()
            );
        }
        BatchDrainOutcome::Stalled => {
            let batch = result.stopped_batch.as_deref().unwrap_or("<batch>");
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch chain stopped in `batch:{batch}` — {} stayed at the head after a successful run",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
        }
        BatchDrainOutcome::Mismatched {
            dispatched,
            shipped,
        } => {
            let batch = result.stopped_batch.as_deref().unwrap_or("<batch>");
            eprintln!(
                "{} batch chain stopped in `batch:{batch}` — dispatched {} but PR credited {}",
                crate::glyph(crate::glyphs::Glyph::Info).cyan().bold(),
                dispatched.bold(),
                shipped.bold()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
        }
        BatchDrainOutcome::Inconclusive => {
            let batch = result.stopped_batch.as_deref().unwrap_or("<batch>");
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch chain paused in `batch:{batch}` at {} — phase 1 inconclusive",
                "⏸".yellow().bold(),
                stopped.bold()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
        }
        // BUG-250: a member deliberately held its PR — the chain pauses there
        // until the operator runs the gate and opens the PR. trace:BUG-250
        BatchDrainOutcome::Held => {
            let batch = result.stopped_batch.as_deref().unwrap_or("<batch>");
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch chain paused in `batch:{batch}` at {} — PR deliberately held",
                "⏸".yellow().bold(),
                stopped.bold()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
        }
        // EPIC-28: every batch drained, but at least one member shelved
        // or was skipped. trace:EPIC-28 | ai:claude
        BatchDrainOutcome::DrainedWithShelved => {
            eprintln!(
                "{} batch chain drained — {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
    }

    if !result.shelved.is_empty() {
        let sn = result.shelved.len();
        eprintln!(
            "  {} {sn} spec{} shelved on phase failure: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if sn == 1 { "" } else { "s" },
            result.shelved.join(", ")
        );
    }
    if !result.skipped.is_empty() {
        let kn = result.skipped.len();
        let render: Vec<String> = result
            .skipped
            .iter()
            .map(|(spec, reason)| format!("{spec} ({reason})"))
            .collect();
        eprintln!(
            "  {} {kn} dependent spec{} skipped: {}",
            "⤳".yellow(),
            if kn == 1 { "" } else { "s" },
            render.join(", ")
        );
    }
    if !result.punted.is_empty() {
        let pn = result.punted.len();
        eprintln!(
            "  {} {pn} spec{} punted to Needs Attention: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if pn == 1 { "" } else { "s" },
            result.punted.join(", ")
        );
    }
    if !result.escalated.is_empty() {
        let en = result.escalated.len();
        eprintln!(
            "  {} {en} spec{} escalated to a human: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if en == 1 { "" } else { "s" },
            result.escalated.join(", ")
        );
    }
}

/// Print the closing summary of a `--batch --auto-complete` drain: what
/// shipped, where it stopped, and what is left queued. The per-spec failure
/// epilogue + recovery hint are already printed by `orchestrate`; this adds
/// the batch-level framing (which members shipped, queue-intact-for-retry).
/// `exit_code` is the process's effective exit code (see caller).
/// `excluded_from_batch` is the "M other approved specs routed to this role
/// are not in this batch" count, re-resolved at drain-end so the closing
/// summary reflects the post-drain queue.
// trace:TASK-285 trace:TASK-1297 | ai:claude
pub(crate) fn emit_batch_drain_summary(
    batch_name: &str,
    result: &auto_complete::BatchDrainResult,
    exit_code: i32,
    json: bool,
    excluded_from_batch: usize,
    excluded_from_batch_role: Option<&str>,
    ineligible: &[events::IneligibleBatchMember],
) {
    use auto_complete::BatchDrainOutcome;

    if json {
        let outcome = match &result.outcome {
            BatchDrainOutcome::Drained => "drained",
            BatchDrainOutcome::MaxReached => "max-reached",
            BatchDrainOutcome::Failed(_) => "failed",
            BatchDrainOutcome::Stalled => "stalled",
            BatchDrainOutcome::Mismatched { .. } => "mismatched",
            BatchDrainOutcome::Inconclusive => "inconclusive",
            BatchDrainOutcome::Held => "held",
            // EPIC-28: a clean drain that shelved at least one member —
            // the operator still has triage to do. trace:EPIC-28
            BatchDrainOutcome::DrainedWithShelved => "drained-with-shelved",
        };
        let mut obj = serde_json::Map::new();
        obj.insert(
            "event".to_string(),
            serde_json::Value::String("batch-drain".into()),
        );
        obj.insert(
            "batch".to_string(),
            serde_json::Value::String(format!("batch:{batch_name}")),
        );
        obj.insert(
            "outcome".to_string(),
            serde_json::Value::String(outcome.into()),
        );
        obj.insert(
            "shipped".to_string(),
            serde_json::Value::Array(
                result
                    .shipped
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "shipped_count".to_string(),
            serde_json::Value::Number((result.shipped.len() as u64).into()),
        );
        // STORY-276: punted members — parked in Needs Attention, not shipped.
        obj.insert(
            "punted".to_string(),
            serde_json::Value::Array(
                result
                    .punted
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "punted_count".to_string(),
            serde_json::Value::Number((result.punted.len() as u64).into()),
        );
        // STORY-306: escalated members — left for a human, not shipped.
        obj.insert(
            "escalated".to_string(),
            serde_json::Value::Array(
                result
                    .escalated
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "escalated_count".to_string(),
            serde_json::Value::Number((result.escalated.len() as u64).into()),
        );
        obj.insert(
            "stopped_at".to_string(),
            match &result.stopped_at {
                Some(s) => serde_json::Value::String(s.clone()),
                None => serde_json::Value::Null,
            },
        );
        obj.insert(
            "exit_code".to_string(),
            serde_json::Value::Number(exit_code.into()),
        );
        // TASK-1297: the excluded-by-filter figure, so a machine consumer
        // can alarm without scraping the human line. trace:TASK-1297 | ai:claude
        obj.insert(
            "excluded_from_batch".to_string(),
            serde_json::Value::Number((excluded_from_batch as u64).into()),
        );
        obj.insert(
            "ineligible".to_string(),
            serde_json::to_value(ineligible).unwrap_or_else(|_| serde_json::Value::Array(vec![])),
        );
        println!("{}", serde_json::Value::Object(obj));
        return;
    }

    let shipped_list = if result.shipped.is_empty() {
        "(none)".to_string()
    } else {
        result.shipped.join(", ")
    };
    let n = result.shipped.len();
    let plural = if n == 1 { "" } else { "s" };
    eprintln!();
    match &result.outcome {
        BatchDrainOutcome::Drained
            if result.shipped.is_empty()
                && result.punted.is_empty()
                && result.escalated.is_empty()
                && ineligible.is_empty() =>
        {
            eprintln!(
                "{} no queued items tagged `batch:{batch_name}` — tag members \
                 via `aida edit <id> --tags batch:{batch_name}` first",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
            );
        }
        BatchDrainOutcome::Drained
            if result.shipped.is_empty()
                && result.punted.is_empty()
                && result.escalated.is_empty() =>
        {
            let rendered = ineligible
                .iter()
                .map(|member| format!("{} ({})", member.spec, member.reason))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "{} batch `batch:{batch_name}` cannot progress — {} member{} remain but none are eligible: {}",
                "⏸".yellow().bold(),
                ineligible.len(),
                if ineligible.len() == 1 { "" } else { "s" },
                rendered,
            );
        }
        BatchDrainOutcome::Drained => {
            eprintln!(
                "{} batch `batch:{batch_name}` drained — {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
        BatchDrainOutcome::MaxReached => {
            eprintln!(
                "{} batch `batch:{batch_name}` — `--max` reached, {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
            eprintln!(
                "  {} more members remain queued — re-run to continue: \
                 `aida queue work --batch {batch_name} --auto-complete`",
                "→".dimmed()
            );
        }
        BatchDrainOutcome::Failed(phase) => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch `batch:{batch_name}` drain stopped at {} (phase {} failed)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold(),
                phase.index()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} the rest of the batch is untouched — fix {stopped} (hint above), \
                 then re-run `aida queue work --batch {batch_name} --auto-complete`",
                "→".dimmed()
            );
        }
        BatchDrainOutcome::Stalled => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch `batch:{batch_name}` drain stopped — {} stayed at the head \
                 after a successful run (queue did not advance)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} check {stopped}'s status (`aida show {stopped}`) — it may not have \
                 been dequeued",
                "→".dimmed()
            );
        }
        // BUG-245: phase 1 shipped a different spec than the dispatched head.
        // Credit the truth, name what stayed queued, and tell the operator
        // how to advance — either pick the dispatched spec back up or skip
        // it. trace:BUG-245 | ai:claude
        BatchDrainOutcome::Mismatched {
            dispatched,
            shipped,
        } => {
            eprintln!(
                "{} batch `batch:{batch_name}` drain stopped — phase 1 was dispatched \
                 for {} but the PR credited {}",
                crate::glyph(crate::glyphs::Glyph::Info).cyan().bold(),
                dispatched.bold(),
                shipped.bold(),
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} {} is still queued — pick it back up with `aida queue work {}`, \
                 or remove it with `aida queue remove {}` if {} subsumed it",
                "→".dimmed(),
                dispatched,
                dispatched,
                dispatched,
                shipped,
            );
        }
        // BUG-257: a transient GH-API outage during phase-1 PR lookup paused
        // the drain. The spec is left in its current state — neither shipped
        // nor failed — so a retry once the API is reachable proceeds without
        // any cleanup. trace:BUG-257 | ai:claude
        BatchDrainOutcome::Inconclusive => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch `batch:{batch_name}` drain paused at {} — phase 1 \
                 inconclusive (GH API unreachable)",
                "⏸".yellow().bold(),
                stopped.bold(),
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} transient — retry once the API is reachable: \
                 `gh api /rate_limit` then re-run \
                 `aida queue work --batch {batch_name} --auto-complete`",
                "→".dimmed()
            );
        }
        // BUG-250: a member deliberately held its PR — the batch pauses there
        // for the operator's manual gate. trace:BUG-250 | ai:claude
        BatchDrainOutcome::Held => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch `batch:{batch_name}` drain paused at {} — PR deliberately held",
                "⏸".yellow().bold(),
                stopped.bold(),
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} run your gate, open the PR (`gh pr create`), then re-run \
                 `aida queue work --batch {batch_name} --auto-complete`",
                "→".dimmed()
            );
        }
        // EPIC-28: the batch drained but parked at least one spec. The
        // top line stays green-ish — independents shipped — and the
        // shelved/skipped summary below points the operator at the
        // triage surface. trace:EPIC-28 | ai:claude
        BatchDrainOutcome::DrainedWithShelved => {
            eprintln!(
                "{} batch `batch:{batch_name}` drained with shelved members — \
                 {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
    }
    // STORY-276: name the members a headless implementer punted — the drain
    // advanced past them, but they parked in Needs Attention rather than
    // shipping, so they need advisor triage.
    if !result.punted.is_empty() {
        let pn = result.punted.len();
        eprintln!(
            "  {} {pn} spec{} punted to Needs Attention: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if pn == 1 { "" } else { "s" },
            result.punted.join(", ")
        );
    }
    // STORY-306: name the members the drain escalated to a human — the
    // reviewer would not auto-merge, or the advisor would not resolve a
    // design-fork. The drain advanced past them; a human decides.
    if !result.escalated.is_empty() {
        let en = result.escalated.len();
        eprintln!(
            "  {} {en} spec{} escalated to a human: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if en == 1 { "" } else { "s" },
            result.escalated.join(", ")
        );
    }
    // EPIC-28: shelved + skipped members. Shelved are the failures the
    // drain tolerated; skipped are the dependents we never even tried
    // because their blocker had been shelved. trace:EPIC-28 | ai:claude
    if !result.shelved.is_empty() {
        let sn = result.shelved.len();
        eprintln!(
            "  {} {sn} spec{} shelved on phase failure: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if sn == 1 { "" } else { "s" },
            result.shelved.join(", ")
        );
    }
    if !result.skipped.is_empty() {
        let kn = result.skipped.len();
        let render: Vec<String> = result
            .skipped
            .iter()
            .map(|(spec, reason)| format!("{spec} ({reason})"))
            .collect();
        eprintln!(
            "  {} {kn} dependent spec{} skipped: {}",
            "⤳".yellow(),
            if kn == 1 { "" } else { "s" },
            render.join(", ")
        );
    }
    // TASK-1297: closing-summary echo of the "M other approved specs routed
    // to this role are not in this batch" figure — the same clause printed
    // at start-up, re-resolved post-drain. Silent when M == 0.
    // trace:TASK-1297 | ai:claude
    if let Some(clause) = batch_exclusion_clause(excluded_from_batch, excluded_from_batch_role) {
        eprintln!(
            "  {} {clause}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
}

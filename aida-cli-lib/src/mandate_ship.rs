//! STORY-1698's mandate ship cover, moved out of `pr_cmd.rs` (STORY-1700
//! condition C1) so the drive-mandate gate can protect this file without
//! gating every PR that touches `pr_cmd.rs`. The code is unchanged.
// trace:STORY-1700 | ai:claude

use crate::pr_cmd::{
    fetch_change_info_via_resolved_forge, normalized_spec_ids, pr_gate_subject,
    ship_approval_policy,
};
use crate::{load_store_for_lookup, pr_ship};

/// STORY-1698: who a fire-fighting mandate lets ship — the mandate id and the
/// advisor principal it is bound to.
// trace:STORY-1698 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MandateCover {
    pub id: String,
    pub principal: String,
}

/// Assemble exclusion subjects only; never use this union for authority inputs.
// trace:TASK-1790 | ai:codex
pub(crate) fn mandate_exclusion_specs(
    named: &[String],
    traced: std::collections::BTreeSet<String>,
    resolve: impl FnMut(&str) -> crate::mandate::ShipSpec,
) -> Vec<crate::mandate::ShipSpec> {
    let mut ids = named.to_vec();
    ids.extend(traced);
    normalized_spec_ids(&ids)
        .iter()
        .map(String::as_str)
        .map(resolve)
        .collect()
}

/// STORY-1698: does an active fire-fighting mandate let THIS session ship PR
/// `pr` past its merge-hold? Reads the mandate records from disk at this
/// instant (never cached); only when one is active does it read the forge,
/// the verdicts and the store. `head` is read lazily for the same reason.
// trace:STORY-1698 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn mandate_ship_cover(
    project_root: &std::path::Path,
    main_worktree: &std::path::Path,
    hold_root: &std::path::Path,
    pr: u64,
    known_ids: &[String],
    head_branch: Option<&str>,
    head: impl FnOnce() -> Option<String>,
    override_stale_approval: bool,
    // STORY-1697: `ship_decision` for a ship; `rework_release_decision` for
    // an approval releasing its rework hold. trace:STORY-1697 | ai:claude
    decide: fn(
        &crate::mandate::ShipCheck<'_>,
    ) -> std::result::Result<String, crate::mandate::MandateRefusal>,
) -> std::result::Result<MandateCover, crate::mandate::MandateRefusal> {
    use crate::mandate::{HoldFacts, MandateRefusal, MandateState, ShipCheck};
    let state = crate::mandate::current_state_reporting(true);
    if !matches!(state, MandateState::Active(_)) {
        return Err(match state {
            MandateState::Expired(m) => MandateRefusal::Expired { id: m.id },
            MandateState::Stopped(m) => MandateRefusal::Stopped { id: m.id },
            _ => MandateRefusal::NoMandate,
        });
    }
    let grant = crate::seat_authority::current_grant(project_root).map(|g| {
        (
            aida_core::team::canonical_role(&g.seat),
            g.principal.trim().to_string(),
        )
    });
    let head = head();
    let (ids, branch, forge_resolved) = pr_gate_subject(main_worktree, pr, known_ids, head_branch);
    let candidates = pr_ship::merge_gate_verdict_candidates(
        &[project_root, main_worktree],
        pr,
        &ids,
        head.as_deref(),
    );
    let author_ids: &[String] = if forge_resolved { &ids } else { &[] };
    let policy = ship_approval_policy(main_worktree, author_ids, branch.as_deref(), &candidates);
    // trace:TASK-1790 | ai:codex
    // Trace-derived IDs can only ADD exclusions. Build verdict/author inputs
    // above from named IDs first; never pass the union to those authority paths.
    let unreadable = |e: anyhow::Error| MandateRefusal::TraceEvidenceUnreadable(format!("{e:#}"));
    let info = fetch_change_info_via_resolved_forge(
        main_worktree,
        pr,
        crate::forge::resolve_open_change_forge_kind(main_worktree),
    )
    .map_err(&unreadable)?;
    let trace_evidence = crate::mandate_traces::collect_pr(
        main_worktree,
        &info.base_ref,
        head.as_deref().unwrap_or(""),
        &info.head_oid,
    )
    .map_err(&unreadable)?;
    // STORY-1700 (R1): the files the pinned diff changes, for the gate check.
    // trace:STORY-1700 | ai:claude
    let changed_paths: Vec<String> = trace_evidence.paths.iter().cloned().collect();
    let gates = crate::mandate_gates::load_human_gates(main_worktree).map_err(|e| format!("{e:#}"));
    let store_root = crate::store_sync::detect_distributed_store_from(project_root)
        .ok_or_else(|| MandateRefusal::TraceEvidenceUnreadable("no git-canonical store".into()))?;
    let store_snapshot = crate::mandate_traces::store_snapshot(&store_root).map_err(&unreadable)?;
    let store = load_store_for_lookup(project_root);
    let specs = mandate_exclusion_specs(&ids, trace_evidence.ids, |id| {
        let req = store.as_ref().and_then(|store| {
            store
                .requirements
                .iter()
                .find(|r| {
                    r.spec_id
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(id))
                        || r.agreed_id
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case(id))
                })
                .cloned()
        });
        // STORY-1698: sticky exclusion reads every stored version.
        let history = crate::mandate::store_history_versions(&store_root, id);
        crate::mandate::ShipSpec {
            id: id.to_owned(),
            current: req,
            history,
        }
    });
    // STORY-1698: every excluded class seen in any version goes into the
    // append-only ledger first, then the ledger is read back as the first
    // source of exclusion. trace:STORY-1698 | ai:claude
    let mandate_dir = crate::mandate::mandate_dir();
    if let Ok(dir) = &mandate_dir {
        for spec in &specs {
            let changes = spec
                .current
                .as_ref()
                .map(crate::mandate::field_change_versions)
                .unwrap_or_default();
            let history: &[aida_core::Requirement] = spec.history.as_deref().unwrap_or(&[]);
            crate::mandate::note_exclusions(
                dir,
                &spec.id,
                spec.current
                    .iter()
                    .chain(changes.iter())
                    .chain(history.iter()),
            );
        }
    }
    let ledger = match &mandate_dir {
        Ok(dir) => crate::mandate::read_exclusions(dir),
        Err(e) => Err(format!("{e:#}")),
    };
    // Keep all current/history evidence within the same observed store version.
    // A concurrent store write refuses this attempt; a retry builds fresh facts.
    if crate::mandate_traces::store_snapshot(&store_root).map_err(&unreadable)? != store_snapshot {
        return Err(MandateRefusal::TraceEvidenceUnreadable(
            "the store changed while collecting exclusions; retry".into(),
        ));
    }
    let hold_record = crate::merge_hold::read_hold_record(hold_root, pr);
    let hold_reason = crate::merge_hold::read_hold(hold_root, pr);
    let hold = HoldFacts::of(hold_record.as_ref(), hold_reason.as_deref());
    let id = decide(&ShipCheck {
        state: &state,
        now: chrono::Utc::now(),
        grant: grant.as_ref().map(|(s, p)| (s.as_str(), p.as_str())),
        specs: &specs,
        specs_resolved: forge_resolved && !ids.is_empty(),
        candidates: &candidates,
        policy: &policy,
        head: head.as_deref(),
        override_stale_approval,
        ledger: &ledger,
        hold: &hold,
        changed_paths: Some(&changed_paths),
        ship_gate: crate::mandate_gates::gate_reason(&gates, crate::mandate_gates::GateOp::Ship),
        hold_clear_gate: crate::mandate_gates::gate_reason(
            &gates,
            crate::mandate_gates::GateOp::HoldClear,
        ),
    })?;
    Ok(MandateCover {
        id,
        principal: grant.map(|(_, p)| p).unwrap_or_default(),
    })
}

/// STORY-1698: put a mandate-released hold back after any failure between
/// the release and a landed merge — the marker, then the forge label. `Err`
/// names whichever half could not be restored, for the refusal text.
// trace:STORY-1698 | ai:claude
pub(crate) fn rearm_mandate_hold_with(
    hold_root: &std::path::Path,
    record: &crate::merge_hold::MergeHoldRecord,
    sync_label: impl FnOnce() -> std::result::Result<(), String>,
) -> std::result::Result<(), String> {
    let mut failed = Vec::new();
    if let Err(e) = crate::merge_hold::rearm_typed_hold(hold_root, record) {
        failed.push(format!("the hold marker could not be rewritten ({e})"));
    }
    if let Err(e) = sync_label() {
        failed.push(format!("the merge-hold label could not be restored ({e})"));
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

/// [`rearm_mandate_hold_with`] against the real forge, as refusal text: what
/// to append to the error the ship returns.
// trace:STORY-1698 | ai:claude
pub(crate) fn rearm_after_mandate_failure(
    hold_root: &std::path::Path,
    record: Option<&crate::merge_hold::MergeHoldRecord>,
    pr: u64,
) -> String {
    let Some(record) = record else {
        return " The mandate-released hold had no record to re-arm; a human must re-place it with `aida merge-hold add`.".to_string();
    };
    match rearm_mandate_hold_with(hold_root, record, || {
        crate::merge_hold::sync_label(hold_root, pr, true).map_err(|e| e.to_string())
    }) {
        Ok(()) => " The merge-hold is re-armed.".to_string(),
        Err(why) => format!(
            " RE-ARMING THE MERGE-HOLD FAILED: {why}. A human must re-place it with `aida merge-hold add {pr}`."
        ),
    }
}

/// STORY-1698: is mandate `id` still active at this instant? Re-read from
/// disk immediately before the merge call.
// trace:STORY-1698 | ai:claude
pub(crate) fn mandate_still_active(id: &str) -> bool {
    matches!(crate::mandate::current_state(), crate::mandate::MandateState::Active(m) if m.id == id)
}

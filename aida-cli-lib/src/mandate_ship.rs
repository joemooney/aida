//! STORY-1698's mandate ship cover, moved out of `pr_cmd.rs` (STORY-1700
//! condition C1) so the drive-mandate gate can protect this file without
//! gating every PR that touches `pr_cmd.rs`. The move itself changed no
//! code; STORY-1700 R1 then added the gate inputs the decisions read: the
//! PR's changed paths, the human gates, and the ship / hold-clear gate
//! reasons.
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
// trace:TASK-1800 | ai:antigravity
pub(crate) fn mandate_exclusion_specs(
    named: &[String],
    traced: std::collections::BTreeSet<String>,
    mut resolve: impl FnMut(&str, bool) -> crate::mandate::ShipSpec,
) -> Vec<crate::mandate::ShipSpec> {
    let named_set: std::collections::BTreeSet<String> =
        normalized_spec_ids(named).into_iter().collect();
    let mut ids = named.to_vec();
    ids.extend(traced);
    normalized_spec_ids(&ids)
        .iter()
        .map(String::as_str)
        .map(|id| resolve(id, named_set.contains(id)))
        .collect()
}

/// TASK-2067: resolve every exclusion subject — the named IDs and the
/// trace-derived ones — to its current record, its recorded aliases and every
/// stored version, exactly as the mandate cover judges them. Shared by the
/// cover and the TASK-1800 replay so the replay measures production
/// resolution.
// trace:TASK-2067 | ai:claude
pub(crate) fn resolve_exclusion_specs(
    store: Option<&aida_core::RequirementsStore>,
    store_root: &std::path::Path,
    named: &[String],
    traced: std::collections::BTreeSet<String>,
) -> Vec<crate::mandate::ShipSpec> {
    let mut history_keys_of: Vec<Vec<String>> = Vec::new();
    let mut specs = mandate_exclusion_specs(named, traced, |id, is_named| {
        let req = store.and_then(|store| {
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
        // TASK-1800: collect any recorded aliases (both spec_id and agreed_id)
        // so that an agreed_id resolves to its stored spec_id object path for
        // history, and the ledger check / note_exclusions cover both keys.
        let mut aliases = Vec::new();
        let mut history_keys = vec![id.to_owned()];
        if let Some(ref r) = req {
            for candidate in [r.spec_id.as_deref(), r.agreed_id.as_deref()]
                .into_iter()
                .flatten()
            {
                let upper = candidate.trim().to_ascii_uppercase();
                if !upper.is_empty() && !upper.eq_ignore_ascii_case(id) {
                    if !aliases
                        .iter()
                        .any(|a: &String| a.eq_ignore_ascii_case(&upper))
                    {
                        aliases.push(upper.clone());
                    }
                    if !history_keys
                        .iter()
                        .any(|k: &String| k.eq_ignore_ascii_case(&upper))
                    {
                        history_keys.push(upper);
                    }
                }
            }
        }
        // History is read in one batch below, once every spec is resolved.
        // trace:BUG-2179 | ai:claude
        history_keys_of.push(history_keys);
        crate::mandate::ShipSpec {
            id: id.to_owned(),
            aliases,
            named: is_named,
            current: req,
            history: Ok(Vec::new()),
        }
    });
    // trace:TASK-1800 | ai:claude
    // A deleted spec has no current record to name its aliases, so find every
    // object that ever carried its ID in store history. A history that cannot
    // be searched refuses below; it never makes the ID dangling.
    // BUG-2179: one search serves every deleted spec, and one walk plus one
    // object read serves every spec's history, instead of a full-history
    // `git log --all` per spec.
    let deleted: Vec<String> = specs
        .iter()
        .filter(|spec| spec.current.is_none())
        .map(|spec| spec.id.clone())
        .collect();
    let alias_search = crate::mandate::store_history_alias_map(store_root, &deleted);
    for (spec, keys) in specs.iter_mut().zip(history_keys_of.iter_mut()) {
        if spec.current.is_some() {
            continue;
        }
        let found = match &alias_search {
            Ok(map) => map
                .get(&spec.id.to_ascii_uppercase())
                .cloned()
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        for alias in found {
            if !spec.aliases.iter().any(|a| a.eq_ignore_ascii_case(&alias)) {
                spec.aliases.push(alias.clone());
            }
            if !keys.iter().any(|k| k.eq_ignore_ascii_case(&alias)) {
                keys.push(alias);
            }
        }
    }
    let all_keys: Vec<String> = history_keys_of.iter().flatten().cloned().collect();
    let batch = crate::mandate::StoreHistoryBatch::read(store_root, &all_keys);
    for (spec, keys) in specs.iter_mut().zip(&history_keys_of) {
        // STORY-1698 / TASK-1800: sticky exclusion reads every stored version
        // across the ID and any resolved alias object path.
        spec.history = (|| -> std::result::Result<Vec<aida_core::Requirement>, String> {
            if spec.current.is_none() {
                alias_search.as_ref().map_err(String::clone)?;
            }
            let batch = batch.as_ref().map_err(String::clone)?;
            let mut all_versions = Vec::new();
            for key in keys {
                all_versions.extend(batch.versions(key)?);
            }
            if spec.named && all_versions.is_empty() {
                return Err(format!("no version of {} in the store's history", spec.id));
            }
            Ok(all_versions)
        })();
    }
    specs
}

/// BUG-2179: how many times the mandate cover rebuilds its store evidence when
/// a concurrent writer changes the store mid-collection, before refusing.
const MANDATE_COLLECT_ATTEMPTS: u32 = 3;

/// TASK-2019: the pause before the next attempt after a dirty or moving store,
/// so an in-flight store write can commit.
// trace:TASK-2067 | ai:antigravity
const MANDATE_COLLECT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);

/// BUG-2179: run `collect` until the store `snapshot` taken before it and the
/// one taken after it are `stable`, so all current/history evidence comes from
/// one observed store version. A concurrent write discards that attempt and the
/// next builds fresh facts; after `MANDATE_COLLECT_ATTEMPTS` it refuses.
///
/// TASK-2019: `snapshot` returns `Ok(None)` for a dirty store (uncommitted
/// evidence). That attempt is retried within the same bound, after `backoff`,
/// instead of refusing at once; a store still dirty on the last attempt
/// refuses. BUG-2195: `stable` decides whether two snapshots are the same
/// store version for the exclusion reader, so an unrelated ref move does not
/// count as a change.
// trace:BUG-2179 | ai:claude
// trace:TASK-2067 | ai:antigravity
fn collect_on_stable_store<S, T>(
    mut snapshot: impl FnMut() -> std::result::Result<Option<S>, crate::mandate::MandateRefusal>,
    mut stable: impl FnMut(&S, &S) -> bool,
    mut collect: impl FnMut() -> T,
    backoff: std::time::Duration,
) -> std::result::Result<T, crate::mandate::MandateRefusal> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let dirty = match snapshot()? {
            None => true,
            Some(before) => {
                let collected = collect();
                match snapshot()? {
                    Some(after) if stable(&before, &after) => return Ok(collected),
                    Some(_) => false,
                    None => true,
                }
            }
        };
        if attempt >= MANDATE_COLLECT_ATTEMPTS {
            return Err(crate::mandate::MandateRefusal::TraceEvidenceUnreadable(
                if dirty {
                    format!("{} ({attempt} attempts)", crate::mandate_traces::StoreDirty)
                } else {
                    format!(
                        "the store changed while collecting exclusions ({attempt} attempts); retry"
                    )
                },
            ));
        }
        std::thread::sleep(backoff);
    }
}

/// STORY-1698: does an active fire-fighting mandate let THIS session ship PR
/// `pr` past its merge-hold? Reads the mandate records from disk at this
/// instant (never cached); only when one is active does it read the forge,
/// the verdicts and the store. `head` is read lazily for the same reason.
// trace:STORY-1698 | ai:claude
// trace:TASK-1800 | ai:antigravity
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
    // TASK-1968: whether `decide` reads `ShipCheck::carry_over`. Only a ship
    // does; the proof (an `ls-remote` plus fetches) is skipped otherwise and
    // `carry_over` is `None` (not attempted). trace:TASK-1968 | ai:claude
    prove_carry_over: bool,
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
    let (ids, branch, forge_resolved, authorship_ids) =
        pr_gate_subject(main_worktree, pr, known_ids, head_branch);
    let candidates = pr_ship::merge_gate_verdict_candidates(
        &[project_root, main_worktree],
        pr,
        &ids,
        head.as_deref(),
    );
    // trace:BUG-2199 | ai:claude
    let author_ids: &[String] = if forge_resolved { &authorship_ids } else { &[] };
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
    // BUG-2179: a live multi-agent store is written to while the evidence is
    // gathered. Rebuild every fact from a fresh snapshot a bounded number of
    // times; after the last attempt the change still refuses (fail closed).
    // trace:BUG-2179 | ai:claude
    // BUG-2195 / TASK-2019: a dirty store is retried within the bound, and
    // two snapshots are compared by the exclusion reader's stability rule.
    // trace:TASK-2067 | ai:antigravity
    let snapshot = || match crate::mandate_traces::store_snapshot(&store_root) {
        Ok(snap) => Ok(Some(snap)),
        Err(e) if e.is::<crate::mandate_traces::StoreDirty>() => Ok(None),
        Err(e) => Err(unreadable(e)),
    };
    let stable = |before: &crate::mandate_traces::StoreSnapshot,
                  after: &crate::mandate_traces::StoreSnapshot| {
        crate::mandate_traces::store_snapshot_stable(&store_root, before, after)
    };
    let (specs, ledger) = collect_on_stable_store(
        snapshot,
        stable,
        || {
            let store = load_store_for_lookup(project_root);
            let specs = resolve_exclusion_specs(
                store.as_ref(),
                &store_root,
                &ids,
                trace_evidence.ids.clone(),
            );
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
                    for key in std::iter::once(&spec.id).chain(spec.aliases.iter()) {
                        crate::mandate::note_exclusions(
                            dir,
                            key,
                            spec.current
                                .iter()
                                .chain(changes.iter())
                                .chain(history.iter()),
                        );
                    }
                }
            }
            let ledger = match &mandate_dir {
                Ok(dir) => crate::mandate::read_exclusions(dir),
                Err(e) => Err(format!("{e:#}")),
            };
            (specs, ledger)
        },
        MANDATE_COLLECT_BACKOFF,
    )?;
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
        // trace:TASK-1960 | ai:claude
        carry_over: prove_carry_over
            .then(|| {
                mandate_carry_over(
                    main_worktree,
                    pr,
                    &candidates,
                    head.as_deref(),
                    Some(info.base_ref.as_str()),
                )
            })
            .flatten(),
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

/// TASK-1960: the carry-over a mandate ship may rely on — `None` when the
/// approval does not need one (or cannot get one: no moved head), else the
/// proven head or the reason it failed. Proof only: the gate that pins the
/// merge records and announces it.
// trace:TASK-1960 | ai:claude
pub(crate) fn mandate_carry_over(
    repo: &std::path::Path,
    pr: u64,
    candidates: &[crate::review_verdict::RecordedVerdict],
    head: Option<&str>,
    base_branch: Option<&str>,
) -> Option<std::result::Result<String, String>> {
    let refusal = pr_ship::approval_head_refusal(candidates, head)?;
    let site = crate::review_carryover::CarryOverSite::quiet(repo, base_branch);
    crate::review_carryover::prove_for_refusal(&site, pr, &refusal)
        .map(|proved| proved.map(|p| p.new_head).map_err(|why| why.reason()))
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

#[cfg(test)]
#[path = "tests/bug_2179_mandate_recollect_tests.rs"]
mod bug_2179_mandate_recollect_tests;

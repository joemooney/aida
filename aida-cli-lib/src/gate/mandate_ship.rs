//! STORY-1698's mandate ship cover, moved out of `pr_cmd.rs` (STORY-1700
//! condition C1) so the drive-mandate gate can protect this file without
//! gating every PR that touches `pr_cmd.rs`. The move itself changed no
//! code; STORY-1700 R1 then added the gate inputs the decisions read: the
//! PR's changed paths, the human gates, and the ship / hold-clear gate
//! reasons.
// trace:STORY-1700 | ai:claude
// mandate-gate

use crate::gate::approval::ship_approval_policy;
use crate::gate::dispatch_deps::load_store_for_lookup;
use crate::pr_cmd::{fetch_change_info_via_resolved_forge, normalized_spec_ids, pr_gate_subject};
use crate::pr_ship;

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
    mut resolve: impl FnMut(&str, bool) -> crate::gate::mandate::ShipSpec,
) -> Vec<crate::gate::mandate::ShipSpec> {
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
) -> Vec<crate::gate::mandate::ShipSpec> {
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
        crate::gate::mandate::ShipSpec {
            id: id.to_owned(),
            aliases,
            named: is_named,
            shipped: false,
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
    let code_root = store_root
        .file_name()
        .filter(|name| *name == ".aida-store")
        .and_then(|_| store_root.parent())
        .unwrap_or(store_root);
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
        // TASK-2314: a trace-derived operator-only exclusion expires only when
        // the traced spec is Completed AND its git linkage shows a merged
        // commit on the default branch. Check default-branch commit linkage
        // only for non-named Completed specs.
        // trace:TASK-2314 | ai:agy
        spec.shipped = !spec.named
            && spec
                .current
                .as_ref()
                .is_some_and(|req| matches!(req.status, aida_core::RequirementStatus::Completed))
            && crate::shipped_linkage::has_shipped_commit_on_default_branch(code_root, keys);
    }
    specs
}

/// BUG-2214: with no marker on disk, "a human already cleared the hold" is
/// accepted only when three independently writable sources all agree: the
/// local clearance record (a human/operator principal, no mandate), a
/// TTY-attested human release event that is the PR's newest hold change, and
/// the forge's label state (`aida:merge-hold` absent, the BUG-1693
/// recorded/cleared pair consistent). `labels` is read last, so the forge
/// round-trip is spent only when both local halves qualify. Anything less —
/// a forged clearance file, a stripped label, an unattested or re-placed
/// event, an unreadable forge — stays [`HoldFacts::None`], which the cover
/// refuses as `NoHoldMarker` (fail closed).
// trace:BUG-2214 | ai:claude
pub(crate) fn cleared_by_human_facts(
    hold_root: &std::path::Path,
    pr: u64,
    labels: impl FnOnce() -> std::result::Result<Option<crate::merge_hold::PinnedChange>, String>,
) -> crate::gate::mandate::HoldFacts {
    use crate::gate::mandate::HoldFacts;
    if crate::gate::merge_hold_core::human_recorded_clearance(hold_root, pr).is_none()
        || !crate::gate::merge_hold_core::human_attested_release(hold_root, pr)
    {
        return HoldFacts::None;
    }
    match labels() {
        Ok(Some(c)) if !c.hold_label && c.recorded_label == c.cleared_label => {
            HoldFacts::ClearedByHuman
        }
        _ => HoldFacts::None,
    }
}

/// BUG-2179: how many times the mandate cover rebuilds its store evidence when
/// a concurrent writer changes the store mid-collection, before refusing.
/// BUG-056: five, with a growing pause, so a burst of fleet writes to the
/// excluded specs themselves is outlasted (~5 s in all) before refusing.
// trace:BUG-B6B1278890774EF993D2EBC327862470-056 | ai:claude
const MANDATE_COLLECT_ATTEMPTS: u32 = 5;

/// TASK-2019: the pause before the next attempt after a dirty or moving store,
/// so an in-flight store write can commit. BUG-056: attempt `n` waits `n`
/// times this.
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
/// count as a change. BUG-056: `stable` also sees what was collected, so only a
/// change to the evidence that collection read discards it.
// trace:BUG-2179 | ai:claude
// trace:TASK-2067 | ai:antigravity
// trace:BUG-B6B1278890774EF993D2EBC327862470-056 | ai:claude
fn collect_on_stable_store<S, T>(
    mut snapshot: impl FnMut() -> std::result::Result<Option<S>, crate::gate::mandate::MandateRefusal>,
    mut stable: impl FnMut(&S, &S, &T) -> bool,
    mut collect: impl FnMut() -> T,
    backoff: std::time::Duration,
) -> std::result::Result<T, crate::gate::mandate::MandateRefusal> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let dirty = match snapshot()? {
            None => true,
            Some(before) => {
                let collected = collect();
                match snapshot()? {
                    Some(after) if stable(&before, &after, &collected) => return Ok(collected),
                    Some(_) => false,
                    None => true,
                }
            }
        };
        if attempt >= MANDATE_COLLECT_ATTEMPTS {
            return Err(
                crate::gate::mandate::MandateRefusal::TraceEvidenceUnreadable(if dirty {
                    format!(
                        "{} ({attempt} attempts)",
                        crate::gate::mandate_traces::StoreDirty
                    )
                } else {
                    format!(
                        "the store changed while collecting exclusions ({attempt} attempts); retry"
                    )
                }),
            );
        }
        std::thread::sleep(backoff * attempt);
    }
}

/// [`mandate_ship_cover`] for a ship (`aida pr ship` and the train): the
/// decision is bound here to [`crate::gate::mandate::ship_decision`], with the
/// carry-over proof, so no caller outside the gate can choose a different
/// one.
// trace:STORY-1698 | ai:claude
// trace:STORY-1889 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn mandate_ship_cover_for_ship(
    project_root: &std::path::Path,
    main_worktree: &std::path::Path,
    hold_root: &std::path::Path,
    pr: u64,
    known_ids: &[String],
    head_branch: Option<&str>,
    head: impl FnOnce() -> Option<String>,
    override_stale_approval: bool,
) -> std::result::Result<MandateCover, crate::gate::mandate::MandateRefusal> {
    mandate_ship_cover(
        project_root,
        main_worktree,
        hold_root,
        pr,
        known_ids,
        head_branch,
        head,
        override_stale_approval,
        crate::gate::mandate::ship_decision,
        true,
    )
}

/// STORY-1698: does an active fire-fighting mandate let THIS session ship PR
/// `pr` past its merge-hold? Reads the mandate records from disk at this
/// instant (never cached); only when one is active does it read the forge,
/// the verdicts and the store. `head` is read lazily for the same reason.
/// Gate-private: callers outside the gate use [`mandate_ship_cover_for_ship`].
// trace:STORY-1698 | ai:claude
// trace:TASK-1800 | ai:antigravity
#[allow(clippy::too_many_arguments)]
pub(in crate::gate) fn mandate_ship_cover(
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
        &crate::gate::mandate::ShipCheck<'_>,
    ) -> std::result::Result<String, crate::gate::mandate::MandateRefusal>,
    // TASK-1968: whether `decide` reads `ShipCheck::carry_over`. Only a ship
    // does; the proof (an `ls-remote` plus fetches) is skipped otherwise and
    // `carry_over` is `None` (not attempted). trace:TASK-1968 | ai:claude
    prove_carry_over: bool,
) -> std::result::Result<MandateCover, crate::gate::mandate::MandateRefusal> {
    use crate::gate::mandate::{HoldFacts, MandateRefusal, MandateState, ShipCheck};
    let state = crate::gate::mandate::current_state_reporting(true);
    match &state {
        MandateState::Active(m) if !m.covers(crate::gate::mandate::Scope::ShipHeld) => {
            return Err(MandateRefusal::ScopeNotCovered {
                id: m.id.clone(),
                scope: crate::gate::mandate::SCOPE_SHIP_HELD,
            });
        }
        MandateState::Active(_) => {}
        MandateState::Expired(m) => return Err(MandateRefusal::Expired { id: m.id.clone() }),
        MandateState::Stopped(m) => return Err(MandateRefusal::Stopped { id: m.id.clone() }),
        MandateState::None => return Err(MandateRefusal::NoMandate),
    }
    let grant = crate::gate::seat_authority::current_grant(project_root).map(|g| {
        (
            aida_core::team::canonical_role(&g.seat),
            g.principal.trim().to_string(),
        )
    });
    let head = head();
    let (ids, branch, forge_resolved, authorship_ids) =
        pr_gate_subject(main_worktree, pr, known_ids, head_branch);
    let candidates = crate::gate::ship::merge_gate_verdict_candidates(
        &[project_root, main_worktree],
        pr,
        &ids,
        head.as_deref(),
    );
    // trace:BUG-2199 | ai:claude
    let author_ids: &[String] = if forge_resolved { &authorship_ids } else { &[] };
    let policy = crate::gate::approval::ship_approval_policy(
        main_worktree,
        author_ids,
        branch.as_deref(),
        &candidates,
    );
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
    // BUG-2214: a forge-side head move (`gh pr update-branch`) leaves the new
    // head unfetched locally, and the evidence diff below fails on the missing
    // commit. Fetch the base and head first; a fetch that still fails falls
    // through to `collect_pr`, which fails closed. trace:BUG-2214 | ai:claude
    if let Some(branch) = head_branch {
        if !have_ship_refs_locally(main_worktree, &info.head_oid, &info.base_ref) {
            let _ = crate::pr_cmd::fetch_ship_refs(main_worktree, &info.base_ref, branch);
        }
    }
    let trace_evidence = crate::gate::mandate_traces::collect_pr(
        main_worktree,
        &info.base_ref,
        head.as_deref().unwrap_or(""),
        &info.head_oid,
    )
    .map_err(&unreadable)?;
    // STORY-1700 (R1): the files the pinned diff changes, for the gate check.
    // trace:STORY-1700 | ai:claude
    let changed_paths: Vec<String> = trace_evidence.paths.iter().cloned().collect();
    let gates =
        crate::gate::mandate_gates::load_human_gates(main_worktree).map_err(|e| format!("{e:#}"));
    let store_root = crate::store_sync::detect_distributed_store_from(project_root)
        .ok_or_else(|| MandateRefusal::TraceEvidenceUnreadable("no git-canonical store".into()))?;
    // BUG-2179: a live multi-agent store is written to while the evidence is
    // gathered. Rebuild every fact from a fresh snapshot a bounded number of
    // times; after the last attempt the change still refuses (fail closed).
    // trace:BUG-2179 | ai:claude
    // BUG-2195 / TASK-2019: a dirty store is retried within the bound, and
    // two snapshots are compared by the exclusion reader's stability rule.
    // trace:TASK-2067 | ai:antigravity
    let snapshot = || match crate::gate::mandate_traces::store_snapshot(&store_root) {
        Ok(snap) => Ok(Some(snap)),
        Err(e) if e.is::<crate::gate::mandate_traces::StoreDirty>() => Ok(None),
        Err(e) => Err(unreadable(e)),
    };
    // BUG-056: the before snapshot pins the store version; only a change to
    // the evidence this collection read (its specs' objects and ID lines)
    // discards it, so unrelated concurrent fleet writes no longer exhaust the
    // bound. A store that did not load is judged by the strict rule.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-056 | ai:claude
    let stable = |before: &crate::gate::mandate_traces::StoreSnapshot,
                  after: &crate::gate::mandate_traces::StoreSnapshot,
                  collected: &(bool, Vec<crate::gate::mandate::ShipSpec>, _)| {
        let (loaded, specs, _) = collected;
        let scope = if *loaded {
            crate::gate::mandate_traces::EvidenceScope::of_ids(
                specs
                    .iter()
                    .flat_map(|spec| std::iter::once(&spec.id).chain(spec.aliases.iter())),
            )
        } else {
            crate::gate::mandate_traces::EvidenceScope::strict()
        };
        crate::gate::mandate_traces::store_snapshot_stable(&store_root, before, after, &scope)
    };
    let (_, specs, ledger) = collect_on_stable_store(
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
            let mandate_dir = crate::gate::mandate::mandate_dir();
            if let Ok(dir) = &mandate_dir {
                for spec in &specs {
                    let changes = spec
                        .current
                        .as_ref()
                        .map(crate::gate::mandate::field_change_versions)
                        .unwrap_or_default();
                    let history: &[aida_core::Requirement] = spec.history.as_deref().unwrap_or(&[]);
                    for key in std::iter::once(&spec.id).chain(spec.aliases.iter()) {
                        crate::gate::mandate::note_exclusions(
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
                Ok(dir) => crate::gate::mandate::read_exclusions(dir),
                Err(e) => Err(format!("{e:#}")),
            };
            (store.is_some(), specs, ledger)
        },
        MANDATE_COLLECT_BACKOFF,
    )?;
    let hold_record = crate::gate::merge_hold_core::read_hold_record(hold_root, pr);
    let hold_reason = crate::gate::merge_hold_core::read_hold(hold_root, pr);
    // BUG-2214: "no marker" may be a hold a human already cleared — accepted
    // only when the clearance record, the attested release event and the forge
    // labels all agree (see [`cleared_by_human_facts`]). trace:BUG-2214 | ai:claude
    let hold = match HoldFacts::of(hold_record.as_ref(), hold_reason.as_deref()) {
        HoldFacts::None => cleared_by_human_facts(hold_root, pr, || {
            crate::merge_hold::fetch_pinned_change(
                project_root,
                crate::forge::resolve_forge_kind(project_root),
                pr,
            )
            .map_err(|e| e.to_string())
        }),
        hold => hold,
    };
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
        // trace:STORY-1888 | ai:claude
        protected: &crate::gate::mandate_gates::ProtectedPaths::of(&gates),
        ship_gate: crate::gate::mandate_gates::gate_reason(
            &gates,
            crate::gate::mandate_gates::GateOp::Ship,
        ),
        hold_clear_gate: crate::gate::mandate_gates::gate_reason(
            &gates,
            crate::gate::mandate_gates::GateOp::HoldClear,
        ),
    })?;
    Ok(MandateCover {
        id,
        principal: grant.map(|(_, p)| p).unwrap_or_default(),
    })
}

/// BUG-2214: the one mandate hold release — ship's exact sequence, shared by
/// `aida pr ship` and the train's land step so the train never re-implements
/// it: the release note, the marker clear, the ledger clearance
/// (`advisor:<principal>` under the mandate id), the PR comment, the
/// `MergeHoldChanged` event and the label removal. Each half is best-effort
/// exactly as ship's was; the caller owns the post-release hold-gate wait and
/// the re-arm on any later failure ([`rearm_after_mandate_failure`]).
// trace:BUG-2214 | ai:claude
// trace:STORY-1698 | ai:claude
pub(crate) fn release_hold_under_mandate(
    hold_root: &std::path::Path,
    change: &crate::forge::ChangeRef,
    cover: &MandateCover,
    hold_record: &crate::gate::merge_hold_core::MergeHoldRecord,
    released_by_verdict: Option<crate::gate::merge_hold_core::VerdictRef>,
    head_sha: Option<&str>,
    reason: &str,
) {
    use colored::Colorize;
    let pr = hold_record.pr;
    // A release the ledger did not record leaves the hold open: nothing may
    // announce it as released (no note, comment, event or label removal).
    // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    if let Err(err) = crate::gate::merge_hold_core::release_hold_under_mandate(
        hold_root,
        hold_record,
        &cover.principal,
        &cover.id,
        released_by_verdict,
    ) {
        eprintln!(
            "  {} could not record the mandate release on PR-{pr}, so its merge hold stays armed: {err}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
        return;
    }
    eprintln!(
        "  {} PR-{pr}: merge-hold {}",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
        crate::gate::mandate::release_note(&cover.id)
    );
    let _ = crate::forge::forge_for(hold_root).comment(
        change,
        &format!(
            "Merge-hold {} (`{}`). Approval at `{}`; CI green.",
            crate::gate::mandate::release_note(&cover.id),
            cover.principal,
            head_sha.unwrap_or("?"),
        ),
    );
    // BUG-1423: the coordination-seat release, in the event taxonomy.
    // trace:BUG-1423 | ai:claude
    let mut hold_event = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MergeHoldChanged {
            pr: pr as u32,
            placed: false,
            // STORY-1698: a mandate release names the mandate.
            reason: Some(format!(
                "{} — {reason}",
                crate::gate::mandate::release_note(&cover.id)
            )),
            // BUG-2214: a mandate release is an advisor action and never
            // attests as human.
            principal: Some(format!("advisor:{}", cover.principal)),
            human_attested: false,
        },
    );
    hold_event.seat = crate::events::active_seat();
    crate::events::emit(hold_root, &hold_event);
    if let Err(err) = crate::merge_hold::sync_label(hold_root, pr, false) {
        eprintln!(
            "  {} {}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            crate::pr_cmd::ship_unlabel_failure(pr, &err),
        );
    }
}

/// TASK-1960: the carry-over a mandate ship may rely on — `None` when the
/// approval does not need one (or cannot get one: no moved head), else the
/// proven head or the reason it failed. Proof only: the gate that pins the
/// merge records and announces it.
// trace:TASK-1960 | ai:claude
pub(crate) fn mandate_carry_over(
    repo: &std::path::Path,
    pr: u64,
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    head: Option<&str>,
    base_branch: Option<&str>,
) -> Option<std::result::Result<String, String>> {
    let refusal = crate::gate::ship::approval_head_refusal(candidates, head)?;
    let site = crate::gate::review_carryover::CarryOverSite::quiet(repo, base_branch);
    crate::gate::review_carryover::prove_for_refusal(&site, pr, &refusal)
        .map(|proved| proved.map(|p| p.new_head).map_err(|why| why.reason()))
}

/// STORY-1698: put a mandate-released hold back after any failure between
/// the release and a landed merge — the marker, then the forge label. `Err`
/// names whichever half could not be restored, for the refusal text.
// trace:STORY-1698 | ai:claude
pub(crate) fn rearm_mandate_hold_with(
    hold_root: &std::path::Path,
    record: &crate::gate::merge_hold_core::MergeHoldRecord,
    sync_label: impl FnOnce() -> std::result::Result<(), String>,
) -> std::result::Result<(), String> {
    let mut failed = Vec::new();
    if let Err(e) = crate::gate::merge_hold_core::rearm_hold(hold_root, record) {
        failed.push(format!(
            "the hold could not be re-armed on the gate ledger ({e})"
        ));
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
    record: Option<&crate::gate::merge_hold_core::MergeHoldRecord>,
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
    matches!(crate::gate::mandate::current_state(), crate::gate::mandate::MandateState::Active(m) if m.id == id)
}

/// Whether both `head_oid^{commit}` and `refs/remotes/origin/<base_ref>^{commit}`
/// already exist locally in `main_worktree`, so `mandate_ship_cover` can skip
/// the pre-evidence fetch.
// trace:BUG-2214 trace:TASK-1864 | ai:agy
pub(crate) fn have_ship_refs_locally(
    main_worktree: &std::path::Path,
    head_oid: &str,
    base_ref: &str,
) -> bool {
    let have_head = !head_oid.is_empty()
        && std::process::Command::new("git")
            .arg("-C")
            .arg(main_worktree)
            .args(["cat-file", "-e", &format!("{}^{{commit}}", head_oid)])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    let have_base = std::process::Command::new("git")
        .arg("-C")
        .arg(main_worktree)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/origin/{}^{{commit}}", base_ref),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    have_head && have_base
}

#[cfg(test)]
#[path = "tests/bug_2179_mandate_recollect_tests.rs"]
mod bug_2179_mandate_recollect_tests;

#[cfg(test)]
#[path = "tests/bug_2214_cleared_hold_tests.rs"]
mod bug_2214_cleared_hold_tests;

#[cfg(test)]
#[path = "tests/task_2067_trace_resolution_tests.rs"]
mod task_2067_trace_resolution_tests;

// trace:ARCH-distributed-conflict | ai:claude
//! Conflict detection and resolution for distributed AIDA.
//!
//! When two nodes edit the same requirement concurrently, we need to:
//! 1. Detect the conflict (two versions with divergent histories)
//! 2. Surface it to the user (don't silently overwrite)
//! 3. Provide resolution options (accept-mine, accept-theirs, merge)
//!
//! This module implements field-level conflict detection by comparing
//! two versions of a requirement and identifying which fields diverged.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::Requirement;

/// A detected conflict between two versions of a requirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementConflict {
    /// The requirement's UUID
    pub id: Uuid,
    /// The spec_id (for display)
    pub spec_id: String,
    /// Fields that have conflicting values
    pub fields: Vec<FieldConflict>,
    /// Timestamp of the local version
    pub local_modified: DateTime<Utc>,
    /// Timestamp of the remote version
    pub remote_modified: DateTime<Utc>,
}

/// A conflict on a single field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldConflict {
    /// Name of the conflicting field
    pub field: String,
    /// Local value
    pub local_value: String,
    /// Remote value
    pub remote_value: String,
}

/// How to resolve a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Keep the local version for all conflicting fields
    AcceptLocal,
    /// Keep the remote version for all conflicting fields
    AcceptRemote,
    /// Keep the version with the later timestamp (LWW). On an exact timestamp
    /// tie the winner is the deterministic content-hash winner (a data-function
    /// both clones compute identically), not "local". trace:BUG-578
    LastWriteWins,
}

/// Detect conflicts between a local and remote version of a requirement.
/// Returns None if there are no conflicts (versions are identical or one is strictly newer).
pub fn detect_conflict(local: &Requirement, remote: &Requirement) -> Option<RequirementConflict> {
    if local.id != remote.id {
        return None; // Different requirements, not a conflict
    }

    // If timestamps are identical, no conflict
    if local.modified_at == remote.modified_at {
        return None;
    }

    let mut fields = Vec::new();

    // Compare fields that matter for conflict detection
    if local.title != remote.title {
        fields.push(FieldConflict {
            field: "title".to_string(),
            local_value: local.title.clone(),
            remote_value: remote.title.clone(),
        });
    }

    if local.description != remote.description {
        fields.push(FieldConflict {
            field: "description".to_string(),
            local_value: truncate(&local.description, 100),
            remote_value: truncate(&remote.description, 100),
        });
    }

    if local.effective_status() != remote.effective_status() {
        fields.push(FieldConflict {
            field: "status".to_string(),
            local_value: local.effective_status(),
            remote_value: remote.effective_status(),
        });
    }

    if local.effective_priority() != remote.effective_priority() {
        fields.push(FieldConflict {
            field: "priority".to_string(),
            local_value: local.effective_priority(),
            remote_value: remote.effective_priority(),
        });
    }

    if local.owner != remote.owner {
        fields.push(FieldConflict {
            field: "owner".to_string(),
            local_value: local.owner.clone(),
            remote_value: remote.owner.clone(),
        });
    }

    if local.tags != remote.tags {
        fields.push(FieldConflict {
            field: "tags".to_string(),
            local_value: format!("{:?}", local.tags),
            remote_value: format!("{:?}", remote.tags),
        });
    }

    if fields.is_empty() {
        return None; // Same content despite different timestamps
    }

    Some(RequirementConflict {
        id: local.id,
        spec_id: local
            .spec_id
            .clone()
            .unwrap_or_else(|| local.id.to_string()),
        fields,
        local_modified: local.modified_at,
        remote_modified: remote.modified_at,
    })
}

/// Apply a resolution strategy to a conflict.
/// Returns the resolved requirement.
pub fn resolve_conflict(
    local: &Requirement,
    remote: &Requirement,
    resolution: Resolution,
) -> Requirement {
    match resolution {
        Resolution::AcceptLocal => local.clone(),
        Resolution::AcceptRemote => remote.clone(),
        Resolution::LastWriteWins => {
            // Later modified_at wins; on an EXACT tie a deterministic
            // data-function (greater stable content hash) decides, so the
            // outcome is order-independent across clones rather than
            // "local"-wins. trace:BUG-578 | ai:claude
            match local.modified_at.cmp(&remote.modified_at) {
                std::cmp::Ordering::Greater => local.clone(),
                std::cmp::Ordering::Less => remote.clone(),
                std::cmp::Ordering::Equal => deterministic_scalar_winner(local, remote),
            }
        }
    }
}

/// Three-way structured merge of one spec YAML during a store-leg rebase.
///
/// MU-204 / STORY-641: when two clones concurrently edit the SAME spec, the
/// store-leg rebase hits a textual conflict in `objects/TYPE/000/SPEC.yaml`
/// even though the spec is structurally reconcilable: the `history:`,
/// `comments:`, and `processing_record:` arrays are append-only (each entry
/// carries an immutable `id`), and the scalar fields can be resolved with the
/// same last-write-wins policy `resolve_conflict` already uses.
///
/// This is a PURE function so it is unit-testable in isolation; the git
/// plumbing (reading the three stages, writing the result, continuing the
/// rebase) lives in the pull path.
///
/// Policy, matching `conflict.rs::resolve_conflict` (LastWriteWins by
/// `modified_at`) and never silently dropping an edit:
/// - **Scalar fields** (title, description, status, priority, owner, …):
///   take the whole `ours`/`theirs` requirement that has the later
///   `modified_at` as the scalar base (LWW). On an *exact* `modified_at` tie
///   (ms collision or clock skew), the winner is the variant with the greater
///   stable content hash — a pure function of the data that BOTH clones compute
///   identically, so two clones merging the same concurrent same-field edits in
///   opposite roles converge to the SAME winner. (Previously "ties → ours",
///   which was order-dependent / last-puller-wins — BUG-578.)
/// - **`history:`** — union by `HistoryEntry.id` (dedupe), ordered by
///   `timestamp` then `id` for determinism. Both clones' entries survive.
/// - **`comments:`** — union by `Comment.id`, ordered by `created_at` then
///   `id`. (`base` may contain comments removed on neither side; the union of
///   ours+theirs preserves everything either side has.)
/// - **`processing_record:`** — union by `ProcessingRecord.id`, ordered by
///   `timestamp` then `id`.
/// - **`tags:`** — union of both sides (a `HashSet`, so a set union).
/// - **`relationships:`** — set union keyed by `(rel_type, target_id)` so two
///   clones concurrently adding a different edge to the SAME spec both survive
///   instead of the LWW base silently dropping the loser's edge (STORY-645).
/// - **`dependencies:`** — set union by target uuid, same rationale.
///
/// **Scalar fields are merged FIELD-BY-FIELD against the base, not by picking
/// one whole snapshot (BUG-586).** The old policy resolved ALL scalars with a
/// single object-level LWW — it took the entire later-`modified_at` snapshot,
/// so a concurrent edit to a *different* field on the older side (e.g. clone A
/// sets priority, clone B sets owner) was silently dropped even though the two
/// edits never touched the same field. The unioned oplog already captured both,
/// but materialization ignored it. Now each scalar resolves independently
/// (`merge_scalar`): a field changed on only ONE side relative to `base` takes
/// that side's value; a field changed on BOTH sides to different values is a
/// genuine conflict resolved by the deterministic LWW winner (BUG-578), never
/// order-dependently. So different-field concurrent edits both survive.
///
/// `base` anchors the field-level 3-way diff (which side changed each field) and
/// the id-keyed array unions.
// trace:BUG-586 | ai:claude
// trace:STORY-645 | ai:claude
pub fn merge_spec_three_way(
    base: &Requirement,
    ours: &Requirement,
    theirs: &Requirement,
) -> Requirement {
    // Determine the deterministic conflict-winner snapshot ONCE: the side with
    // the later modified_at, or — on an EXACT tie — the greater stable content
    // hash (a data-function both clones compute identically, never "ours"). This
    // is used ONLY to break genuine same-field-both-sides conflicts, so the
    // outcome is order-independent across clones. trace:BUG-578 | ai:claude
    let ours_is_winner = match ours.modified_at.cmp(&theirs.modified_at) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        // EXACT modified_at tie: defer to the same deterministic data-function
        // resolve_conflict uses — greater stable content hash, UUID as the final
        // tiebreak. Both clones compute this identically, so merging the same
        // pair in opposite ours/theirs roles converges to the SAME winner.
        std::cmp::Ordering::Equal => deterministic_winner_is_a(ours, theirs),
    };

    // Start from the conflict-winner snapshot so any field NOT explicitly
    // field-merged below (custom_fields maps, optional metadata blobs, etc.)
    // still has a deterministic, order-independent value. The scalar fields are
    // then OVERRIDDEN by the per-field 3-way merge so different-field edits both
    // survive. trace:BUG-586 | ai:claude
    let winner: &Requirement = if ours_is_winner { ours } else { theirs };
    let mut merged = winner.clone();

    // ---- BUG-586: field-by-field scalar merge against the base ----
    // For each scalar, `merge_scalar` returns the side that changed it (when
    // only one side did) or the conflict-winner's value (when both sides changed
    // it to different values). `winner`/`loser` give the deterministic tie-break
    // direction. trace:BUG-586 | ai:claude
    merged.title = merge_scalar(&base.title, &ours.title, &theirs.title, ours_is_winner).clone();
    merged.description = merge_scalar(
        &base.description,
        &ours.description,
        &theirs.description,
        ours_is_winner,
    )
    .clone();
    merged.status =
        merge_scalar(&base.status, &ours.status, &theirs.status, ours_is_winner).clone();
    merged.custom_status = merge_scalar(
        &base.custom_status,
        &ours.custom_status,
        &theirs.custom_status,
        ours_is_winner,
    )
    .clone();
    merged.priority = merge_scalar(
        &base.priority,
        &ours.priority,
        &theirs.priority,
        ours_is_winner,
    )
    .clone();
    merged.custom_priority = merge_scalar(
        &base.custom_priority,
        &ours.custom_priority,
        &theirs.custom_priority,
        ours_is_winner,
    )
    .clone();
    merged.owner = merge_scalar(&base.owner, &ours.owner, &theirs.owner, ours_is_winner).clone();
    merged.assignee = merge_scalar(
        &base.assignee,
        &ours.assignee,
        &theirs.assignee,
        ours_is_winner,
    )
    .clone();
    merged.feature = merge_scalar(
        &base.feature,
        &ours.feature,
        &theirs.feature,
        ours_is_winner,
    )
    .clone();
    merged.weight = *merge_scalar(&base.weight, &ours.weight, &theirs.weight, ours_is_winner);
    merged.archived = *merge_scalar(
        &base.archived,
        &ours.archived,
        &theirs.archived,
        ours_is_winner,
    );
    merged.archived_at = merge_scalar(
        &base.archived_at,
        &ours.archived_at,
        &theirs.archived_at,
        ours_is_winner,
    )
    .clone();
    merged.deferred = *merge_scalar(
        &base.deferred,
        &ours.deferred,
        &theirs.deferred,
        ours_is_winner,
    );
    merged.deferred_at = merge_scalar(
        &base.deferred_at,
        &ours.deferred_at,
        &theirs.deferred_at,
        ours_is_winner,
    )
    .clone();
    merged.deferred_until = merge_scalar(
        &base.deferred_until,
        &ours.deferred_until,
        &theirs.deferred_until,
        ours_is_winner,
    )
    .clone();
    merged.human_only = *merge_scalar(
        &base.human_only,
        &ours.human_only,
        &theirs.human_only,
        ours_is_winner,
    );
    // STORY-776: the advisor's execution-mode classification survives a
    // divergent sync — a mode set (or cleared) on only one side wins over the
    // other side's unchanged value, instead of being clobbered by the
    // object-level winner snapshot.
    merged.execution_mode = *merge_scalar(
        &base.execution_mode,
        &ours.execution_mode,
        &theirs.execution_mode,
        ours_is_winner,
    );

    // History: union by entry id, deterministic order. base contributes
    // nothing new (append-only ⇒ ours+theirs ⊇ base) but we fold it in too so
    // an entry present only in base (e.g. one side rewrote the array) is never
    // dropped.
    merged.history = union_history(&[&base.history, &ours.history, &theirs.history]);

    // Comments: union by comment id, deterministic order.
    merged.comments = union_comments(&[&base.comments, &ours.comments, &theirs.comments]);

    // Processing records: union by record id, deterministic order.
    merged.processing_record = union_processing_records(&[
        &base.processing_record,
        &ours.processing_record,
        &theirs.processing_record,
    ]);

    // Tags: 3-way set merge against base, so a concurrent REMOVAL wins over the
    // other side's mere retention instead of being resurrected by a 2-way union
    // (the classic G-Set "can't remove" problem). An item present in `base` but
    // absent from a side means that side removed it; the removal survives. Items
    // added by either side (absent from base) survive. trace:BUG-602 | ai:claude
    merged.tags = merge_string_set_three_way(&base.tags, &ours.tags, &theirs.tags);

    // Relationships: set union keyed by the natural identity (rel_type,
    // target_id). Without this, the LWW scalar base would silently drop the
    // loser's concurrently-added relationship edge — so two clones each running
    // `aida rel add` on the SAME spec produced a manual conflict (STORY-645).
    // The first occurrence of a key wins, so its created_at/created_by metadata
    // is preserved deterministically.
    // 3-way set merge against base keyed by (rel_type, target_id): an edge in
    // base but absent from a side was REMOVED by that side, and the removal wins
    // over the other side's retention (no tombstone resurrection — BUG-602).
    // Edges added by either side (absent from base) survive. trace:STORY-645
    // trace:BUG-602 | ai:claude
    merged.relationships = merge_relationships_three_way(
        &base.relationships,
        &ours.relationships,
        &theirs.relationships,
    );

    // Dependencies: 3-way set merge by target uuid, same removal-wins rationale.
    // trace:STORY-645 trace:BUG-602 | ai:claude
    merged.dependencies =
        merge_dependencies_three_way(&base.dependencies, &ours.dependencies, &theirs.dependencies);

    merged
}

/// Three-way merge of a single scalar field against the merge base (BUG-586).
///
/// The whole point: a field changed on only ONE side must take that side's
/// value, so two clones editing DIFFERENT fields of the same spec both keep
/// their edits instead of the older-`modified_at` side losing everything.
///
/// Rules (a true 3-way per-field merge):
/// - `ours == theirs`            → no divergence, return that value.
/// - only `ours` changed vs base → take `ours`.
/// - only `theirs` changed vs base → take `theirs`.
/// - BOTH changed to *different* values → a genuine same-field conflict;
///   resolve by the deterministic LWW winner (`ours_is_winner`, computed once
///   from `modified_at` + the BUG-578 content-hash tie-break), so the outcome is
///   order-independent across clones. trace:BUG-586 | ai:claude
fn merge_scalar<'a, T: PartialEq>(
    base: &T,
    ours: &'a T,
    theirs: &'a T,
    ours_is_winner: bool,
) -> &'a T {
    if ours == theirs {
        // Both sides agree (either neither touched it, or both set the same
        // value) — no conflict.
        return ours;
    }
    let ours_changed = ours != base;
    let theirs_changed = theirs != base;
    match (ours_changed, theirs_changed) {
        // Only one side changed it: that side's edit survives. This is the case
        // BUG-586 was silently dropping for the older-modified_at side.
        (true, false) => ours,
        (false, true) => theirs,
        // Both changed it to different values (genuine conflict) — or, defensively,
        // neither differs from base yet the two sides differ (impossible given the
        // ours==theirs guard, but total). Deterministic LWW winner decides.
        (true, true) | (false, false) => {
            if ours_is_winner {
                ours
            } else {
                theirs
            }
        }
    }
}

/// Decide the scalar-base winner when two versions have the EXACT same
/// `modified_at` (a millisecond collision or clock skew).
///
/// The discriminator MUST be a pure function of the data that both clones
/// compute identically, so a same-field concurrent edit converges to the same
/// winner no matter which clone runs the merge (the old "ours"/"local" fallback
/// was order-dependent — BUG-578). We compare a stable content hash and pick the
/// greater; the UUID `id` breaks the (astronomically unlikely) hash collision so
/// the function is total and never falls back to side-identity.
///
/// trace:BUG-578 | ai:claude
fn deterministic_scalar_winner(a: &Requirement, b: &Requirement) -> Requirement {
    if deterministic_winner_is_a(a, b) {
        a.clone()
    } else {
        b.clone()
    }
}

/// The deterministic-tiebreak predicate: `true` iff `a` is the winner of an
/// exact-`modified_at` tie. Compares the stable content hash (greater wins) and
/// falls back to the UUID — both pure data-functions, so two clones agree no
/// matter which side they call `a`. trace:BUG-578 | ai:claude
fn deterministic_winner_is_a(a: &Requirement, b: &Requirement) -> bool {
    let (ha, hb) = (stable_content_hash(a), stable_content_hash(b));
    match ha.cmp(&hb) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        // Hash collision (or genuinely identical content): fall back to the
        // UUID, which is still a data-function — never "ours".
        std::cmp::Ordering::Equal => a.id >= b.id,
    }
}

/// A stable, order-independent content hash of a requirement, used only to
/// break exact `modified_at` ties deterministically. Hashes the deterministic
/// YAML serialization (the same byte form AIDA writes on disk, which has a
/// fixed field order), so both clones derive an identical value for identical
/// content. If serialization somehow fails, fall back to a debug-format hash so
/// the function is total.
///
/// trace:BUG-578 | ai:claude
fn stable_content_hash(req: &Requirement) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match serde_yaml::to_string(req) {
        Ok(s) => s.hash(&mut hasher),
        Err(_) => format!("{req:?}").hash(&mut hasher),
    }
    hasher.finish()
}

/// Union HistoryEntry arrays by `id`, ordered by `(timestamp, id)`.
fn union_history(
    sources: &[&Vec<crate::models::HistoryEntry>],
) -> Vec<crate::models::HistoryEntry> {
    use std::collections::HashMap;
    let mut by_id: HashMap<Uuid, crate::models::HistoryEntry> = HashMap::new();
    for src in sources {
        for entry in src.iter() {
            by_id.entry(entry.id).or_insert_with(|| entry.clone());
        }
    }
    let mut out: Vec<_> = by_id.into_values().collect();
    out.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.id.cmp(&b.id)));
    out
}

/// Union Comment arrays by `id`, ordered by `(created_at, id)`.
fn union_comments(sources: &[&Vec<crate::models::Comment>]) -> Vec<crate::models::Comment> {
    use std::collections::HashMap;
    let mut by_id: HashMap<Uuid, crate::models::Comment> = HashMap::new();
    for src in sources {
        for c in src.iter() {
            by_id.entry(c.id).or_insert_with(|| c.clone());
        }
    }
    let mut out: Vec<_> = by_id.into_values().collect();
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    out
}

/// Union ProcessingRecord arrays by `id`, ordered by `(timestamp, id)`.
fn union_processing_records(
    sources: &[&Vec<crate::models::ProcessingRecord>],
) -> Vec<crate::models::ProcessingRecord> {
    use std::collections::HashMap;
    let mut by_id: HashMap<Uuid, crate::models::ProcessingRecord> = HashMap::new();
    for src in sources {
        for r in src.iter() {
            by_id.entry(r.id).or_insert_with(|| r.clone());
        }
    }
    let mut out: Vec<_> = by_id.into_values().collect();
    out.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.id.cmp(&b.id)));
    out
}

/// 3-way set merge of a string set (tags) against the merge base.
///
/// The 2-way union (`ours ∪ theirs`) is WRONG for a removable set: an item the
/// base had and ONE side removed gets resurrected because the other side still
/// carries it (the classic G-Set "can't remove" problem the red-team flagged).
///
/// The correct 3-way rule mirrors `merge_scalar`:
/// - an item ADDED by either side (absent from `base`) survives;
/// - an item REMOVED by either side (present in `base`, absent on that side)
///   stays removed — the removal wins over the other side's mere retention;
/// - an item present everywhere survives.
///
/// Formally: `result = (ours ∪ theirs) − removed`, where
/// `removed = (base − ours) ∪ (base − theirs)`. An item that one side removed
/// AND the other side re-added is treated as removed (the removal is the newer
/// intent relative to base for the removing side; we bias to removal so a stale
/// retention can never silently undo a delete — symmetric with how `merge_scalar`
/// lets a single-side change win). trace:BUG-602 | ai:claude
fn merge_string_set_three_way(
    base: &std::collections::HashSet<String>,
    ours: &std::collections::HashSet<String>,
    theirs: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let mut result: std::collections::HashSet<String> = ours.union(theirs).cloned().collect();
    // Remove anything either side deleted relative to base.
    for item in base {
        let removed_by_ours = !ours.contains(item);
        let removed_by_theirs = !theirs.contains(item);
        if removed_by_ours || removed_by_theirs {
            result.remove(item);
        }
    }
    result
}

/// 3-way set merge of Relationship edges keyed by `(rel_type, target_id)`.
///
/// Same removal-wins semantics as `merge_string_set_three_way`: an edge in
/// `base` that a side dropped stays dropped (no tombstone resurrection); an edge
/// either side added survives. The first surviving occurrence's metadata
/// (created_at/created_by) is kept for determinism; output order is first-seen.
/// trace:STORY-645 trace:BUG-602 | ai:claude
fn merge_relationships_three_way(
    base: &[crate::models::Relationship],
    ours: &[crate::models::Relationship],
    theirs: &[crate::models::Relationship],
) -> Vec<crate::models::Relationship> {
    use std::collections::HashSet;
    type Key = (crate::models::RelationshipType, Uuid);
    let key = |r: &crate::models::Relationship| (r.rel_type.clone(), r.target_id);
    let keyset =
        |src: &[crate::models::Relationship]| -> HashSet<Key> { src.iter().map(key).collect() };
    let base_keys = keyset(base);
    let ours_keys = keyset(ours);
    let theirs_keys = keyset(theirs);

    let mut seen: HashSet<Key> = HashSet::new();
    let mut out: Vec<crate::models::Relationship> = Vec::new();
    // Union of ours+theirs in first-seen order, minus anything either side
    // removed relative to base.
    for rel in ours.iter().chain(theirs.iter()) {
        let k = key(rel);
        // Skip edges that base had but a side deleted.
        if base_keys.contains(&k) && (!ours_keys.contains(&k) || !theirs_keys.contains(&k)) {
            continue;
        }
        if seen.insert(k) {
            out.push(rel.clone());
        }
    }
    out
}

/// 3-way set merge of dependency uuid lists, removal-wins (BUG-602).
/// trace:STORY-645 trace:BUG-602 | ai:claude
fn merge_dependencies_three_way(base: &[Uuid], ours: &[Uuid], theirs: &[Uuid]) -> Vec<Uuid> {
    use std::collections::HashSet;
    let base_set: HashSet<Uuid> = base.iter().copied().collect();
    let ours_set: HashSet<Uuid> = ours.iter().copied().collect();
    let theirs_set: HashSet<Uuid> = theirs.iter().copied().collect();

    let mut seen: HashSet<Uuid> = HashSet::new();
    let mut out: Vec<Uuid> = Vec::new();
    for dep in ours.iter().chain(theirs.iter()) {
        if base_set.contains(dep) && (!ours_set.contains(dep) || !theirs_set.contains(dep)) {
            continue; // removed by a side relative to base
        }
        if seen.insert(*dep) {
            out.push(*dep);
        }
    }
    out
}

/// Three-way merge of one per-user queue registry file
/// (`registry/queues/<user>.yaml`) during a store-leg rebase or union merge.
///
/// BUG-725: the queue registry is one file per queue-user, so the SAME user
/// working from two machines produces concurrent commits to the SAME file and
/// the store pull-rebase used to abort with "conflict in non-mergeable path".
/// The file is structurally reconcilable exactly like the spec arrays:
/// entries are keyed by `requirement_id` (the substrate's own `queue_add`
/// upserts on that key), so the merge is an id-keyed union.
///
/// This is a PURE function so it is unit-testable in isolation; the git
/// plumbing (reading the three stages, writing the result) lives in
/// `git_ops::resolve_queue_conflict`.
///
/// Policy, per key (`requirement_id`), mirroring `merge_scalar` / the BUG-602
/// removal-wins set merges:
/// - **Added by either side** (absent from `base`) → survives. Two machines
///   queueing different specs both keep their entries.
/// - **Removed by either side** (present in `base`, absent on that side) →
///   stays removed. `aida queue done` / `remove` on one machine is never
///   resurrected by the other machine's mere retention.
/// - **Present on both sides, identical** → kept as-is.
/// - **Present on both sides, divergent** (e.g. both machines repositioned
///   it): if only ONE side changed it relative to `base`, that side's entry
///   wins; if BOTH changed it, last-writer-wins by `added_at` (a re-add
///   stamps a fresh `added_at`, so the newer add carries its position). An
///   exact-`added_at` tie resolves by the greater stable YAML serialization —
///   a pure data-function both clones compute identically, so the merge is
///   order-independent (never "ours"), per the BUG-578 precedent.
///
/// Output is sorted by `(position, added_at, requirement_id)` so both clones
/// serialize byte-identical results. Entries are never invented and a
/// surviving entry is always one side's verbatim entry — the merge cannot
/// corrupt or drop concurrently-queued work.
// trace:BUG-725 | ai:claude
pub fn merge_queue_three_way(
    base: &[crate::models::QueueEntry],
    ours: &[crate::models::QueueEntry],
    theirs: &[crate::models::QueueEntry],
) -> Vec<crate::models::QueueEntry> {
    use crate::models::QueueEntry;
    use std::collections::HashMap;

    // First occurrence per key within a side wins (files are position-sorted,
    // and queue_add's upsert keeps at most one entry per requirement anyway).
    let index = |src: &[QueueEntry]| -> HashMap<Uuid, QueueEntry> {
        let mut m: HashMap<Uuid, QueueEntry> = HashMap::new();
        for e in src {
            m.entry(e.requirement_id).or_insert_with(|| e.clone());
        }
        m
    };
    let base_by_id = index(base);
    let ours_by_id = index(ours);
    let theirs_by_id = index(theirs);

    let mut out: Vec<QueueEntry> = Vec::new();
    let mut emitted: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    for entry in ours.iter().chain(theirs.iter()) {
        let id = entry.requirement_id;
        if !emitted.insert(id) {
            continue;
        }
        let in_base = base_by_id.contains_key(&id);
        let o = ours_by_id.get(&id);
        let t = theirs_by_id.get(&id);
        // Removal-wins: base had it and a side dropped it → stays dropped.
        if in_base && (o.is_none() || t.is_none()) {
            continue;
        }
        let merged = match (o, t) {
            (Some(o), Some(t)) => {
                if o == t {
                    o.clone()
                } else {
                    match base_by_id.get(&id) {
                        // Only one side changed it vs base → that side wins.
                        Some(b) if o == b => t.clone(),
                        Some(b) if t == b => o.clone(),
                        // Both changed it (or no base): deterministic LWW.
                        _ => queue_entry_lww(o, t).clone(),
                    }
                }
            }
            // Present on one side only and not removed → a fresh add.
            (Some(one), None) | (None, Some(one)) => one.clone(),
            (None, None) => continue, // unreachable: entry came from one side
        };
        out.push(merged);
    }
    out.sort_by(|a, b| {
        a.position
            .cmp(&b.position)
            .then(a.added_at.cmp(&b.added_at))
            .then(a.requirement_id.cmp(&b.requirement_id))
    });
    out
}

/// Deterministic last-writer-wins between two divergent queue entries for the
/// same requirement: later `added_at` wins; an exact tie resolves by the
/// greater stable YAML serialization (a pure data-function, so both clones
/// converge no matter which side they call "ours").
// trace:BUG-725 | ai:claude
fn queue_entry_lww<'a>(
    a: &'a crate::models::QueueEntry,
    b: &'a crate::models::QueueEntry,
) -> &'a crate::models::QueueEntry {
    match a.added_at.cmp(&b.added_at) {
        std::cmp::Ordering::Greater => a,
        std::cmp::Ordering::Less => b,
        std::cmp::Ordering::Equal => {
            let (ya, yb) = (
                serde_yaml::to_string(a).unwrap_or_else(|_| format!("{a:?}")),
                serde_yaml::to_string(b).unwrap_or_else(|_| format!("{b:?}")),
            );
            if ya >= yb {
                a
            } else {
                b
            }
        }
    }
}

/// Detect conflicts between a local store and a set of remote requirements.
/// Returns all detected conflicts.
pub fn detect_store_conflicts(
    local_reqs: &[Requirement],
    remote_reqs: &[Requirement],
) -> Vec<RequirementConflict> {
    let mut conflicts = Vec::new();

    for local in local_reqs {
        for remote in remote_reqs {
            if local.id == remote.id {
                if let Some(conflict) = detect_conflict(local, remote) {
                    conflicts.push(conflict);
                }
                break;
            }
        }
    }

    conflicts
}

// trace:BUG-475
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}...")
    }
}

/// Format a conflict for display.
impl std::fmt::Display for RequirementConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Conflict on {}", self.spec_id)?;
        writeln!(
            f,
            "  Local modified:  {}",
            self.local_modified.format("%Y-%m-%d %H:%M:%S")
        )?;
        writeln!(
            f,
            "  Remote modified: {}",
            self.remote_modified.format("%Y-%m-%d %H:%M:%S")
        )?;
        for field in &self.fields {
            writeln!(f, "  Field: {}", field.field)?;
            writeln!(f, "    Local:  {}", field.local_value)?;
            writeln!(f, "    Remote: {}", field.remote_value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_req(title: &str, status: &str) -> Requirement {
        let mut req = Requirement::new(title.to_string(), "description".to_string());
        req.set_status_from_str(status);
        req
    }

    #[test]
    fn test_no_conflict_identical() {
        let req = make_req("Title", "Draft");
        assert!(detect_conflict(&req, &req).is_none());
    }

    #[test]
    fn test_no_conflict_same_content_different_time() {
        let mut local = make_req("Title", "Draft");
        let mut remote = local.clone();
        // Same content, different timestamps — no real conflict
        local.modified_at = Utc::now();
        remote.modified_at = Utc::now();
        // modified_at will differ by nanoseconds but content is the same
        assert!(detect_conflict(&local, &remote).is_none());
    }

    #[test]
    fn test_conflict_on_title() {
        let local = make_req("Local Title", "Draft");
        let mut remote = local.clone();
        remote.title = "Remote Title".to_string();
        remote.modified_at = Utc::now();

        let conflict = detect_conflict(&local, &remote);
        assert!(conflict.is_some());

        let c = conflict.unwrap();
        assert_eq!(c.fields.len(), 1);
        assert_eq!(c.fields[0].field, "title");
        assert_eq!(c.fields[0].local_value, "Local Title");
        assert_eq!(c.fields[0].remote_value, "Remote Title");
    }

    #[test]
    fn test_conflict_multiple_fields() {
        let mut local = make_req("Title", "Draft");
        local.owner = "joe".to_string();

        let mut remote = local.clone();
        remote.title = "Changed Title".to_string();
        remote.owner = "alice".to_string();
        remote.set_status_from_str("Approved");
        remote.modified_at = Utc::now();

        let conflict = detect_conflict(&local, &remote).unwrap();
        assert_eq!(conflict.fields.len(), 3); // title, status, owner
    }

    #[test]
    fn test_resolve_accept_local() {
        let local = make_req("Local", "Draft");
        let mut remote = local.clone();
        remote.title = "Remote".to_string();
        remote.modified_at = Utc::now();

        let resolved = resolve_conflict(&local, &remote, Resolution::AcceptLocal);
        assert_eq!(resolved.title, "Local");
    }

    #[test]
    fn test_resolve_accept_remote() {
        let local = make_req("Local", "Draft");
        let mut remote = local.clone();
        remote.title = "Remote".to_string();
        remote.modified_at = Utc::now();

        let resolved = resolve_conflict(&local, &remote, Resolution::AcceptRemote);
        assert_eq!(resolved.title, "Remote");
    }

    #[test]
    fn test_resolve_lww() {
        let mut local = make_req("Local", "Draft");
        let mut remote = local.clone();
        remote.title = "Remote".to_string();

        // Make remote newer
        local.modified_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        remote.modified_at = chrono::DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let resolved = resolve_conflict(&local, &remote, Resolution::LastWriteWins);
        assert_eq!(resolved.title, "Remote"); // remote is newer
    }

    #[test]
    fn test_store_conflicts() {
        let req1_local = make_req("Req 1 Local", "Draft");
        let mut req1_remote = req1_local.clone();
        req1_remote.title = "Req 1 Remote".to_string();
        req1_remote.modified_at = Utc::now();

        let req2 = make_req("Req 2", "Draft"); // no conflict

        let conflicts = detect_store_conflicts(&[req1_local, req2.clone()], &[req1_remote, req2]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].fields[0].field, "title");
    }

    #[test]
    fn test_conflict_display() {
        let mut local = make_req("Local", "Draft");
        local.spec_id = Some("FR-1-001".to_string());
        let mut remote = local.clone();
        remote.title = "Remote".to_string();
        remote.modified_at = Utc::now();

        let conflict = detect_conflict(&local, &remote).unwrap();
        let display = format!("{}", conflict);
        assert!(display.contains("FR-1-001"));
        assert!(display.contains("title"));
        assert!(display.contains("Local"));
        assert!(display.contains("Remote"));
    }

    // trace:BUG-475
    #[test]
    fn test_truncate_ascii_truncates() {
        let s = "a".repeat(150);
        let out = truncate(&s, 100);
        assert_eq!(out, format!("{}...", "a".repeat(100)));
    }

    // trace:BUG-475
    #[test]
    fn test_truncate_short_unchanged() {
        assert_eq!(truncate("short", 100), "short");
    }

    // trace:BUG-475 — multi-byte char straddling the byte cutoff must not panic.
    #[test]
    fn test_truncate_multibyte_near_boundary_no_panic() {
        // 99 ASCII bytes + a 2-byte 'é' => char 100 straddles byte index 100.
        let s = "a".repeat(99) + "é";
        // 100 chars total, so it is not truncated, and must not panic on the byte slice.
        let out = truncate(&s, 100);
        assert_eq!(out, s);

        // Now force truncation right at the multi-byte char (101 chars, max 100).
        let s2 = "a".repeat(100) + "é";
        let out2 = truncate(&s2, 100);
        assert_eq!(out2, format!("{}...", "a".repeat(100)));
    }

    // ----- STORY-641: three-way structured merge (MU-204) -----

    use crate::models::{Comment, FieldChange, HistoryEntry};

    fn hist(author: &str, ts: &str) -> HistoryEntry {
        HistoryEntry {
            id: Uuid::now_v7(),
            author: author.to_string(),
            timestamp: DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Utc),
            changes: vec![FieldChange {
                field_name: "status".to_string(),
                old_value: "draft".to_string(),
                new_value: "approved".to_string(),
            }],
        }
    }

    // trace:STORY-641 — concurrent same-spec history appends union by id.
    #[test]
    fn test_merge_unions_history_by_id_dedupe() {
        let mut base = make_req("Title", "Draft");
        let shared = hist("base", "2026-01-01T00:00:00Z");
        base.history = vec![shared.clone()];

        let mut ours = base.clone();
        let ours_entry = hist("a", "2026-01-02T00:00:00Z");
        ours.history = vec![shared.clone(), ours_entry.clone()];
        ours.set_status_from_str("Approved");
        ours.modified_at = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut theirs = base.clone();
        let theirs_entry = hist("b", "2026-01-03T00:00:00Z");
        theirs.history = vec![shared.clone(), theirs_entry.clone()];
        theirs.set_status_from_str("In Progress");
        theirs.modified_at = DateTime::parse_from_rfc3339("2026-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let merged = merge_spec_three_way(&base, &ours, &theirs);

        // All three unique entries present, deduped (shared appears once).
        assert_eq!(merged.history.len(), 3);
        let ids: std::collections::HashSet<_> = merged.history.iter().map(|h| h.id).collect();
        assert!(ids.contains(&shared.id));
        assert!(ids.contains(&ours_entry.id));
        assert!(ids.contains(&theirs_entry.id));
        // Ordered by timestamp.
        assert_eq!(merged.history[0].id, shared.id);
        assert_eq!(merged.history[1].id, ours_entry.id);
        assert_eq!(merged.history[2].id, theirs_entry.id);
    }

    // trace:STORY-641 — scalar fields resolve last-write-wins by modified_at.
    #[test]
    fn test_merge_scalar_lww_by_modified_at() {
        let base = make_req("Title", "Draft");

        let mut ours = base.clone();
        ours.set_status_from_str("Approved");
        ours.modified_at = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut theirs = base.clone();
        theirs.set_status_from_str("In Progress");
        theirs.modified_at = DateTime::parse_from_rfc3339("2026-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // theirs is newer → its status wins.
        let merged = merge_spec_three_way(&base, &ours, &theirs);
        assert_eq!(merged.effective_status(), "In Progress");

        // Flip: ours newer → ours status wins.
        let mut ours2 = ours.clone();
        ours2.modified_at = DateTime::parse_from_rfc3339("2026-01-04T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let merged2 = merge_spec_three_way(&base, &ours2, &theirs);
        assert_eq!(merged2.effective_status(), "Approved");
    }

    // trace:BUG-578 — EXACT modified_at tie must converge to the SAME winner no
    // matter which clone runs the merge. Two concurrent same-field edits with an
    // identical timestamp: merging (ours=A, theirs=B) and (ours=B, theirs=A)
    // MUST pick the same title. The old "ties → ours" code FAILS this (each side
    // keeps its own edit = divergence / last-puller-wins).
    #[test]
    fn test_merge_scalar_tie_is_order_independent() {
        let base = make_req("Title", "Draft");
        let tie_ts = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut edit_a = base.clone();
        edit_a.title = "Edit A".to_string();
        edit_a.modified_at = tie_ts;

        let mut edit_b = base.clone();
        edit_b.title = "Edit B".to_string();
        edit_b.modified_at = tie_ts;

        // Clone 1 sees A as ours, B as theirs.
        let merged_1 = merge_spec_three_way(&base, &edit_a, &edit_b);
        // Clone 2 sees the SAME pair in the opposite roles.
        let merged_2 = merge_spec_three_way(&base, &edit_b, &edit_a);

        // Order-independence: both clones converge to the same scalar winner.
        assert_eq!(
            merged_1.title, merged_2.title,
            "exact modified_at tie must be resolved by a data-function, not by which side is 'ours'"
        );
        // And the winner is one of the two real edits (not silently the base).
        assert!(merged_1.title == "Edit A" || merged_1.title == "Edit B");
    }

    // trace:BUG-578 — resolve_conflict's LastWriteWins must also be
    // order-independent on an exact timestamp tie.
    #[test]
    fn test_resolve_lww_tie_is_order_independent() {
        let tie_ts = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut a = make_req("Edit A", "Draft");
        a.modified_at = tie_ts;
        let mut b = make_req("Edit B", "Draft");
        b.modified_at = tie_ts;

        let r1 = resolve_conflict(&a, &b, Resolution::LastWriteWins);
        let r2 = resolve_conflict(&b, &a, Resolution::LastWriteWins);
        assert_eq!(
            r1.title, r2.title,
            "LastWriteWins tie must resolve identically regardless of local/remote roles"
        );
        assert!(r1.title == "Edit A" || r1.title == "Edit B");
    }

    // trace:BUG-578 — sanity: a strictly later modified_at still wins (LWW not
    // broken by the deterministic tie-break).
    #[test]
    fn test_merge_scalar_later_wins_not_broken_by_tiebreak() {
        let base = make_req("Title", "Draft");

        let mut older = base.clone();
        older.title = "Older".to_string();
        older.modified_at = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut newer = base.clone();
        newer.title = "Newer".to_string();
        newer.modified_at = DateTime::parse_from_rfc3339("2026-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // newer wins regardless of argument order.
        assert_eq!(merge_spec_three_way(&base, &older, &newer).title, "Newer");
        assert_eq!(merge_spec_three_way(&base, &newer, &older).title, "Newer");
    }

    // trace:STORY-641 — tags union across both sides.
    #[test]
    fn test_merge_tags_union() {
        let mut base = make_req("Title", "Draft");
        base.tags.insert("shared".to_string());

        let mut ours = base.clone();
        ours.tags.insert("ours-only".to_string());
        ours.modified_at = Utc::now();

        let mut theirs = base.clone();
        theirs.tags.insert("theirs-only".to_string());
        theirs.modified_at = Utc::now();

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        assert!(merged.tags.contains("shared"));
        assert!(merged.tags.contains("ours-only"));
        assert!(merged.tags.contains("theirs-only"));
        assert_eq!(merged.tags.len(), 3);
    }

    // trace:STORY-641 — identical-on-both-sides merge is a no-op.
    #[test]
    fn test_merge_identical_is_noop() {
        let mut base = make_req("Title", "Draft");
        base.history = vec![hist("base", "2026-01-01T00:00:00Z")];
        base.tags.insert("t".to_string());
        let ours = base.clone();
        let theirs = base.clone();

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        assert_eq!(merged.title, base.title);
        assert_eq!(merged.effective_status(), base.effective_status());
        assert_eq!(merged.history.len(), 1);
        assert_eq!(merged.history[0].id, base.history[0].id);
        assert_eq!(merged.tags, base.tags);
    }

    // trace:STORY-641 — comments union by id (append-only thread).
    #[test]
    fn test_merge_unions_comments_by_id() {
        let base = make_req("Title", "Draft");

        let mut ours = base.clone();
        let c_ours = Comment::new("a".to_string(), "ours comment".to_string());
        ours.comments = vec![c_ours.clone()];
        ours.modified_at = Utc::now();

        let mut theirs = base.clone();
        let c_theirs = Comment::new("b".to_string(), "theirs comment".to_string());
        theirs.comments = vec![c_theirs.clone()];
        theirs.modified_at = Utc::now();

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        assert_eq!(merged.comments.len(), 2);
        let ids: std::collections::HashSet<_> = merged.comments.iter().map(|c| c.id).collect();
        assert!(ids.contains(&c_ours.id));
        assert!(ids.contains(&c_theirs.id));
    }

    // ----- STORY-645: relationship + dependency set-union on same-spec merge -----

    use crate::models::{Relationship, RelationshipType};

    fn rel(rel_type: RelationshipType, target: Uuid) -> Relationship {
        Relationship {
            rel_type,
            target_id: target,
            created_at: Some(Utc::now()),
            created_by: Some("a".to_string()),
        }
    }

    // trace:STORY-645 — two clones add a DIFFERENT relationship to the same spec;
    // both edges must survive (LWW base would drop the loser's edge).
    #[test]
    fn test_merge_unions_relationships_by_key() {
        let base = make_req("Title", "Draft");
        let shared_target = Uuid::now_v7();
        let ours_target = Uuid::now_v7();
        let theirs_target = Uuid::now_v7();

        let mut ours = base.clone();
        ours.relationships = vec![
            rel(RelationshipType::BlockedBy, shared_target),
            rel(RelationshipType::BlockedBy, ours_target),
        ];
        ours.modified_at = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut theirs = base.clone();
        theirs.relationships = vec![
            rel(RelationshipType::BlockedBy, shared_target),
            rel(RelationshipType::BlockedBy, theirs_target),
        ];
        // theirs is the LWW winner — without the union its relationships would
        // be the only ones kept.
        theirs.modified_at = DateTime::parse_from_rfc3339("2026-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let merged = merge_spec_three_way(&base, &ours, &theirs);

        // shared deduped to one; ours + theirs both survive => 3 edges.
        assert_eq!(merged.relationships.len(), 3);
        let targets: std::collections::HashSet<_> =
            merged.relationships.iter().map(|r| r.target_id).collect();
        assert!(targets.contains(&shared_target));
        assert!(targets.contains(&ours_target));
        assert!(targets.contains(&theirs_target));
    }

    // trace:STORY-645 — the key is (rel_type, target_id): same target, different
    // type is two distinct edges; identical (type,target) dedupes.
    #[test]
    fn test_merge_relationships_key_is_type_and_target() {
        let base = make_req("Title", "Draft");
        let target = Uuid::now_v7();

        let mut ours = base.clone();
        ours.relationships = vec![rel(RelationshipType::Parent, target)];
        ours.modified_at = Utc::now();

        let mut theirs = base.clone();
        // Same target, DIFFERENT rel_type => distinct edge; plus an exact dup of
        // ours that must collapse.
        theirs.relationships = vec![
            rel(RelationshipType::BlockedBy, target),
            rel(RelationshipType::Parent, target),
        ];
        theirs.modified_at = Utc::now();

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        // Parent->target (deduped) + BlockedBy->target = 2 edges.
        assert_eq!(merged.relationships.len(), 2);
    }

    // trace:STORY-645 — dependency uuid lists union as a set.
    #[test]
    fn test_merge_unions_dependencies() {
        let base = make_req("Title", "Draft");
        let shared = Uuid::now_v7();
        let ours_dep = Uuid::now_v7();
        let theirs_dep = Uuid::now_v7();

        let mut ours = base.clone();
        ours.dependencies = vec![shared, ours_dep];
        ours.modified_at = Utc::now();

        let mut theirs = base.clone();
        theirs.dependencies = vec![shared, theirs_dep];
        theirs.modified_at = Utc::now();

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        assert_eq!(merged.dependencies.len(), 3);
        let set: std::collections::HashSet<_> = merged.dependencies.iter().copied().collect();
        assert!(set.contains(&shared));
        assert!(set.contains(&ours_dep));
        assert!(set.contains(&theirs_dep));
    }

    // trace:STORY-645 — comment union dedupes an identical comment by id while
    // keeping two genuinely different comments (extends the STORY-641 test).
    #[test]
    fn test_merge_comments_dedupe_identical_keep_distinct() {
        let base = make_req("Title", "Draft");

        // A comment that exists on BOTH sides (same id) must collapse to one.
        let shared = Comment::new("a".to_string(), "shared".to_string());

        let mut ours = base.clone();
        let c_ours = Comment::new("a".to_string(), "ours".to_string());
        ours.comments = vec![shared.clone(), c_ours.clone()];
        ours.modified_at = Utc::now();

        let mut theirs = base.clone();
        let c_theirs = Comment::new("b".to_string(), "theirs".to_string());
        theirs.comments = vec![shared.clone(), c_theirs.clone()];
        theirs.modified_at = Utc::now();

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        // shared (deduped) + ours + theirs = 3 distinct comment ids.
        assert_eq!(merged.comments.len(), 3);
        let ids: std::collections::HashSet<_> = merged.comments.iter().map(|c| c.id).collect();
        assert!(ids.contains(&shared.id));
        assert!(ids.contains(&c_ours.id));
        assert!(ids.contains(&c_theirs.id));
    }

    // ----- BUG-586: field-level scalar merge (concurrent DIFFERENT-field edits) -----

    // trace:BUG-586 — THE DOGFOOD REPRO. base spec; side A sets priority=High;
    // side B sets owner=bob; three-way merge MUST keep BOTH. Against the old
    // object-level LWW this FAILS (the older-modified_at side's edit is silently
    // dropped); with the field-level merge it PASSES.
    #[test]
    fn test_merge_concurrent_different_scalar_fields_both_survive() {
        let mut base = make_req("Title", "Draft");
        base.priority = crate::models::RequirementPriority::Low;
        base.owner = String::new();
        base.modified_at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // Side A: priority -> High, modified_at = T1 (the OLDER side).
        let mut side_a = base.clone();
        side_a.priority = crate::models::RequirementPriority::High;
        side_a.modified_at = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // Side B: owner -> bob, modified_at = T2 > T1 (the LWW winner under the
        // old policy — so it would have dropped A's priority edit).
        let mut side_b = base.clone();
        side_b.owner = "bob".to_string();
        side_b.modified_at = DateTime::parse_from_rfc3339("2026-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let merged = merge_spec_three_way(&base, &side_a, &side_b);

        assert_eq!(
            merged.effective_priority(),
            "High",
            "side A's priority edit must survive even though it has the older modified_at"
        );
        assert_eq!(merged.owner, "bob", "side B's owner edit must survive");

        // Order-independence: opposite roles converge to the same result.
        let merged_rev = merge_spec_three_way(&base, &side_b, &side_a);
        assert_eq!(merged_rev.effective_priority(), "High");
        assert_eq!(merged_rev.owner, "bob");
    }

    // trace:BUG-586 — many DIFFERENT field pairs each survive a concurrent merge.
    #[test]
    fn test_merge_many_different_field_pairs_survive() {
        let base = make_req("Base Title", "Draft");

        // ours changes title + status; theirs changes owner + description +
        // priority. Disjoint field sets => all five edits must survive.
        let mut ours = base.clone();
        ours.title = "Ours Title".to_string();
        ours.set_status_from_str("Approved");
        ours.modified_at = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut theirs = base.clone();
        theirs.owner = "carol".to_string();
        theirs.description = "Theirs description".to_string();
        theirs.priority = crate::models::RequirementPriority::High;
        theirs.modified_at = DateTime::parse_from_rfc3339("2026-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        for (a, b) in [(&ours, &theirs), (&theirs, &ours)] {
            let merged = merge_spec_three_way(&base, a, b);
            assert_eq!(merged.title, "Ours Title");
            assert_eq!(merged.effective_status(), "Approved");
            assert_eq!(merged.owner, "carol");
            assert_eq!(merged.description, "Theirs description");
            assert_eq!(merged.effective_priority(), "High");
        }
    }

    // trace:BUG-586 — a field changed on only ONE side, regardless of which side
    // is the LWW winner, takes that side's value (the other field stays at base).
    #[test]
    fn test_merge_single_side_field_change_wins_against_base() {
        let mut base = make_req("Title", "Draft");
        base.owner = "original".to_string();

        // Only `ours` touches title; `theirs` is the newer (LWW-winner) snapshot
        // but left title untouched.
        let mut ours = base.clone();
        ours.title = "Ours Only".to_string();
        ours.modified_at = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut theirs = base.clone();
        theirs.modified_at = DateTime::parse_from_rfc3339("2026-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        assert_eq!(
            merged.title, "Ours Only",
            "the only side that changed title must win even though it is not the LWW snapshot"
        );
        assert_eq!(merged.owner, "original");
    }

    // trace:BUG-586 / BUG-578 — a GENUINE same-field-both-sides conflict still
    // resolves deterministically (LWW by modified_at; content-hash on a tie) and
    // is order-independent.
    #[test]
    fn test_merge_same_field_both_sides_deterministic_winner() {
        let base = make_req("Base", "Draft");

        // Both change title to DIFFERENT values; theirs is strictly newer.
        let mut ours = base.clone();
        ours.title = "Ours".to_string();
        ours.modified_at = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut theirs = base.clone();
        theirs.title = "Theirs".to_string();
        theirs.modified_at = DateTime::parse_from_rfc3339("2026-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // Newer side wins, regardless of argument order (no silent loss, no panic).
        assert_eq!(merge_spec_three_way(&base, &ours, &theirs).title, "Theirs");
        assert_eq!(merge_spec_three_way(&base, &theirs, &ours).title, "Theirs");
    }

    // trace:BUG-586 / BUG-578 — same-field conflict on an EXACT modified_at tie:
    // both clones converge to the same winner via the content-hash data-function.
    #[test]
    fn test_merge_same_field_tie_order_independent() {
        let base = make_req("Base", "Draft");
        let tie_ts = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut ours = base.clone();
        ours.owner = "alice".to_string();
        ours.modified_at = tie_ts;

        let mut theirs = base.clone();
        theirs.owner = "bob".to_string();
        theirs.modified_at = tie_ts;

        let m1 = merge_spec_three_way(&base, &ours, &theirs);
        let m2 = merge_spec_three_way(&base, &theirs, &ours);
        assert_eq!(
            m1.owner, m2.owner,
            "exact-tie same-field conflict must converge"
        );
        assert!(m1.owner == "alice" || m1.owner == "bob");
    }

    // trace:BUG-586 — different-field edits survive even when both sides share an
    // EXACT modified_at (the worst case for the old object-LWW: a tie would pick
    // one whole snapshot and drop the other's field).
    #[test]
    fn test_merge_different_fields_on_exact_tie_both_survive() {
        let base = make_req("Title", "Draft");
        let tie_ts = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut ours = base.clone();
        ours.priority = crate::models::RequirementPriority::High;
        ours.modified_at = tie_ts;

        let mut theirs = base.clone();
        theirs.owner = "bob".to_string();
        theirs.modified_at = tie_ts;

        for (a, b) in [(&ours, &theirs), (&theirs, &ours)] {
            let merged = merge_spec_three_way(&base, a, b);
            assert_eq!(merged.effective_priority(), "High");
            assert_eq!(merged.owner, "bob");
        }
    }

    // trace:BUG-586 — concurrent edits to scalar fields AND arrays both survive:
    // the field-level scalar merge must not regress the STORY-641/645 array
    // unions (history/comments/tags/relationships).
    #[test]
    fn test_merge_scalars_and_arrays_compose() {
        let mut base = make_req("Title", "Draft");
        base.tags.insert("shared".to_string());

        let mut ours = base.clone();
        ours.priority = crate::models::RequirementPriority::High; // scalar edit
        ours.tags.insert("ours-tag".to_string()); // array edit
        ours.comments = vec![Comment::new("a".to_string(), "ours".to_string())];
        ours.modified_at = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut theirs = base.clone();
        theirs.owner = "bob".to_string(); // different scalar edit
        theirs.tags.insert("theirs-tag".to_string()); // array edit
        theirs.comments = vec![Comment::new("b".to_string(), "theirs".to_string())];
        theirs.modified_at = DateTime::parse_from_rfc3339("2026-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        // Both scalar edits survive.
        assert_eq!(merged.effective_priority(), "High");
        assert_eq!(merged.owner, "bob");
        // All tags survive (set union).
        assert!(merged.tags.contains("shared"));
        assert!(merged.tags.contains("ours-tag"));
        assert!(merged.tags.contains("theirs-tag"));
        // Both comments survive (id union).
        assert_eq!(merged.comments.len(), 2);
    }

    // trace:BUG-586 — direct unit coverage of the per-field 3-way helper.
    #[test]
    fn test_merge_scalar_helper_rules() {
        // Both equal -> that value.
        assert_eq!(merge_scalar(&"base", &"x", &"x", true), &"x");
        // Only ours changed -> ours (even when ours is NOT the winner).
        assert_eq!(merge_scalar(&"base", &"ours", &"base", false), &"ours");
        // Only theirs changed -> theirs (even when theirs is NOT the winner).
        assert_eq!(merge_scalar(&"base", &"base", &"theirs", true), &"theirs");
        // Both changed differently -> winner decides.
        assert_eq!(merge_scalar(&"base", &"ours", &"theirs", true), &"ours");
        assert_eq!(merge_scalar(&"base", &"ours", &"theirs", false), &"theirs");
    }

    // ===== BUG-602 / stress hardening: tombstones, associativity, idempotence =====
    //
    // These lock down the concurrent-merge correctness properties that the
    // simple 2-field BUG-586 dogfood didn't cover. The niche's most critical
    // surface — never silently lose a committed edit, never resurrect a deleted
    // item, converge deterministically and order-independently.

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    // SCENARIO 1 — 3-way concurrent: three clones each edit a DIFFERENT scalar
    // field; all three edits survive, and the result is the same regardless of
    // the order the pairwise merges run (associativity / commutativity). Each
    // pairwise merge re-anchors against the ORIGINAL base, as the rebase does.
    // trace:BUG-602 | ai:claude
    #[test]
    fn test_merge_three_way_disjoint_scalars_associative() {
        let mut base = make_req("Base", "Draft");
        base.owner = String::new();
        base.priority = crate::models::RequirementPriority::Low;
        base.modified_at = ts("2026-01-01T00:00:00Z");

        let mut a = base.clone();
        a.title = "A title".to_string();
        a.modified_at = ts("2026-01-02T00:00:00Z");

        let mut b = base.clone();
        b.owner = "bob".to_string();
        b.modified_at = ts("2026-01-03T00:00:00Z");

        let mut c = base.clone();
        c.priority = crate::models::RequirementPriority::High;
        c.modified_at = ts("2026-01-04T00:00:00Z");

        let abc = merge_spec_three_way(&base, &merge_spec_three_way(&base, &a, &b), &c);
        let cba = merge_spec_three_way(&base, &merge_spec_three_way(&base, &c, &b), &a);
        let bca = merge_spec_three_way(&base, &merge_spec_three_way(&base, &b, &c), &a);

        // All three disjoint edits survive.
        assert_eq!(abc.title, "A title", "A's title lost in 3-way");
        assert_eq!(abc.owner, "bob", "B's owner lost in 3-way");
        assert_eq!(
            abc.effective_priority(),
            "High",
            "C's priority lost in 3-way"
        );
        // Order-independent: every merge order converges to the same result.
        for other in [&cba, &bca] {
            assert_eq!(abc.title, other.title);
            assert_eq!(abc.owner, other.owner);
            assert_eq!(abc.effective_priority(), other.effective_priority());
        }
    }

    // SCENARIO 4 — TOMBSTONE (tag removal). A removes a tag while B keeps it (and
    // edits an unrelated scalar). The removal MUST win; a 2-way union would
    // resurrect the deleted tag (the classic G-Set "can't remove" problem the
    // P4 red-team flagged). This is the BUG-602 regression. trace:BUG-602
    #[test]
    fn test_merge_tag_removal_wins_over_concurrent_edit() {
        let mut base = make_req("Base", "Draft");
        base.tags.insert("keep".to_string());
        base.tags.insert("doomed".to_string());
        base.modified_at = ts("2026-01-01T00:00:00Z");

        let mut a = base.clone();
        a.tags.remove("doomed");
        a.priority = crate::models::RequirementPriority::High;
        a.modified_at = ts("2026-01-02T00:00:00Z");

        let mut b = base.clone();
        b.owner = "bob".to_string(); // never touched tags
        b.modified_at = ts("2026-01-03T00:00:00Z");

        for (x, y) in [(&a, &b), (&b, &a)] {
            let merged = merge_spec_three_way(&base, x, y);
            assert!(merged.tags.contains("keep"), "untouched tag dropped");
            assert!(
                !merged.tags.contains("doomed"),
                "removed tag resurrected by union (G-Set bug)"
            );
            // The concurrent scalar edits both still survive.
            assert_eq!(merged.effective_priority(), "High");
            assert_eq!(merged.owner, "bob");
        }
    }

    // SCENARIO 4 — concurrent ADD on one side, REMOVE-different on the other: a
    // tag added by A and a different tag removed by B both apply (add survives,
    // remove sticks). trace:BUG-602
    #[test]
    fn test_merge_tag_add_and_remove_compose() {
        let mut base = make_req("Base", "Draft");
        base.tags.insert("old".to_string());
        base.modified_at = ts("2026-01-01T00:00:00Z");

        let mut a = base.clone();
        a.tags.insert("new".to_string()); // ADD new
        a.modified_at = ts("2026-01-02T00:00:00Z");

        let mut b = base.clone();
        b.tags.remove("old"); // REMOVE old
        b.modified_at = ts("2026-01-03T00:00:00Z");

        for (x, y) in [(&a, &b), (&b, &a)] {
            let merged = merge_spec_three_way(&base, x, y);
            assert!(merged.tags.contains("new"), "added tag lost");
            assert!(!merged.tags.contains("old"), "removed tag resurrected");
        }
    }

    // SCENARIO 4b — TOMBSTONE (relationship removal). A removes an edge, B keeps
    // it and edits a scalar. The removal wins; the edge is not resurrected.
    // trace:BUG-602 trace:STORY-645
    #[test]
    fn test_merge_relationship_removal_wins_over_concurrent_edit() {
        let target = Uuid::now_v7();
        let mut base = make_req("Base", "Draft");
        base.relationships = vec![rel(RelationshipType::BlockedBy, target)];
        base.modified_at = ts("2026-01-01T00:00:00Z");

        let mut a = base.clone();
        a.relationships.clear(); // remove the edge
        a.modified_at = ts("2026-01-02T00:00:00Z");

        let mut b = base.clone();
        b.owner = "bob".to_string(); // keeps edge, edits scalar
        b.modified_at = ts("2026-01-03T00:00:00Z");

        for (x, y) in [(&a, &b), (&b, &a)] {
            let merged = merge_spec_three_way(&base, x, y);
            assert_eq!(
                merged.relationships.len(),
                0,
                "removed relationship resurrected by union"
            );
            assert_eq!(merged.owner, "bob");
        }
    }

    // SCENARIO 4b — add-vs-remove on relationships concurrently: A removes the
    // base edge, B adds a new edge; result has only the new edge. trace:BUG-602
    #[test]
    fn test_merge_relationship_add_and_remove_compose() {
        let base_target = Uuid::now_v7();
        let new_target = Uuid::now_v7();
        let mut base = make_req("Base", "Draft");
        base.relationships = vec![rel(RelationshipType::BlockedBy, base_target)];
        base.modified_at = ts("2026-01-01T00:00:00Z");

        let mut a = base.clone();
        a.relationships.clear(); // remove base edge
        a.modified_at = ts("2026-01-02T00:00:00Z");

        let mut b = base.clone();
        b.relationships
            .push(rel(RelationshipType::BlockedBy, new_target)); // add new edge
        b.modified_at = ts("2026-01-03T00:00:00Z");

        for (x, y) in [(&a, &b), (&b, &a)] {
            let merged = merge_spec_three_way(&base, x, y);
            let targets: std::collections::HashSet<_> =
                merged.relationships.iter().map(|r| r.target_id).collect();
            assert!(!targets.contains(&base_target), "removed edge resurrected");
            assert!(targets.contains(&new_target), "added edge lost");
            assert_eq!(merged.relationships.len(), 1);
        }
    }

    // SCENARIO 4 — dependency removal wins over retention. trace:BUG-602
    #[test]
    fn test_merge_dependency_removal_wins() {
        let dep = Uuid::now_v7();
        let kept = Uuid::now_v7();
        let mut base = make_req("Base", "Draft");
        base.dependencies = vec![dep, kept];
        base.modified_at = ts("2026-01-01T00:00:00Z");

        let mut a = base.clone();
        a.dependencies.retain(|d| *d != dep); // remove dep
        a.modified_at = ts("2026-01-02T00:00:00Z");

        let mut b = base.clone();
        b.owner = "bob".to_string();
        b.modified_at = ts("2026-01-03T00:00:00Z");

        for (x, y) in [(&a, &b), (&b, &a)] {
            let merged = merge_spec_three_way(&base, x, y);
            assert!(
                !merged.dependencies.contains(&dep),
                "removed dep resurrected"
            );
            assert!(merged.dependencies.contains(&kept), "kept dep lost");
        }
    }

    // SCENARIO 3 — mixed scalar + array + tag in one merge, with a tag REMOVAL
    // on one side: A edits priority + adds comment + adds tag X; B edits owner +
    // adds a different comment + removes tag Y. After merge: both scalars, BOTH
    // comments, tag X present, tag Y gone. trace:BUG-602
    #[test]
    fn test_merge_mixed_scalar_array_tag_with_removal() {
        let mut base = make_req("Base", "Draft");
        base.tags.insert("Y".to_string());
        base.modified_at = ts("2026-01-01T00:00:00Z");

        let mut a = base.clone();
        a.priority = crate::models::RequirementPriority::High;
        a.tags.insert("X".to_string());
        a.comments = vec![Comment::new("a".to_string(), "from A".to_string())];
        a.modified_at = ts("2026-01-02T00:00:00Z");

        let mut b = base.clone();
        b.owner = "bob".to_string();
        b.tags.remove("Y");
        b.comments = vec![Comment::new("b".to_string(), "from B".to_string())];
        b.modified_at = ts("2026-01-03T00:00:00Z");

        for (x, y) in [(&a, &b), (&b, &a)] {
            let merged = merge_spec_three_way(&base, x, y);
            assert_eq!(merged.effective_priority(), "High", "A priority lost");
            assert_eq!(merged.owner, "bob", "B owner lost");
            assert!(merged.tags.contains("X"), "added tag X lost");
            assert!(!merged.tags.contains("Y"), "removed tag Y resurrected");
            assert_eq!(merged.comments.len(), 2, "a comment was lost");
        }
    }

    // SCENARIO 5 — rapid sequences: many edits on each side before the merge.
    // The LATEST per-field value wins; no intermediate value leaks. Modeled by
    // each side's final snapshot carrying its last value + its full appended
    // history; the union must keep every history row and the final scalars.
    // trace:BUG-602
    #[test]
    fn test_merge_rapid_sequences_latest_value_wins() {
        let mut base = make_req("Base", "Draft");
        base.modified_at = ts("2026-01-01T00:00:00Z");
        base.history = vec![hist("base", "2026-01-01T00:00:00Z")];

        // A: title walked v1->v2->v3, three history rows, final = "A-v3".
        let mut a = base.clone();
        a.title = "A-v3".to_string();
        a.history.push(hist("a", "2026-01-02T00:00:01Z"));
        a.history.push(hist("a", "2026-01-02T00:00:02Z"));
        a.history.push(hist("a", "2026-01-02T00:00:03Z"));
        a.modified_at = ts("2026-01-02T00:00:03Z");

        // B: owner walked through several values, final = "bob"; two history rows.
        let mut b = base.clone();
        b.owner = "bob".to_string();
        b.history.push(hist("b", "2026-01-03T00:00:01Z"));
        b.history.push(hist("b", "2026-01-03T00:00:02Z"));
        b.modified_at = ts("2026-01-03T00:00:02Z");

        let merged = merge_spec_three_way(&base, &a, &b);
        assert_eq!(merged.title, "A-v3", "intermediate title leaked");
        assert_eq!(merged.owner, "bob");
        // Every appended history row survives the union (1 base + 3 A + 2 B = 6).
        assert_eq!(merged.history.len(), 6, "history rows lost in union");
    }

    // SCENARIO 6 — idempotence: merging the same two states twice == once. No
    // duplicated array entries; converged scalars stay stable. trace:BUG-602
    #[test]
    fn test_merge_idempotent() {
        let mut base = make_req("Base", "Draft");
        base.tags.insert("shared".to_string());
        base.history = vec![hist("base", "2026-01-01T00:00:00Z")];
        base.modified_at = ts("2026-01-01T00:00:00Z");

        let mut a = base.clone();
        a.priority = crate::models::RequirementPriority::High;
        a.tags.insert("a-tag".to_string());
        a.comments = vec![Comment::new("a".to_string(), "a".to_string())];
        a.modified_at = ts("2026-01-02T00:00:00Z");

        let mut b = base.clone();
        b.owner = "bob".to_string();
        b.tags.insert("b-tag".to_string());
        b.comments = vec![Comment::new("b".to_string(), "b".to_string())];
        b.modified_at = ts("2026-01-03T00:00:00Z");

        let once = merge_spec_three_way(&base, &a, &b);
        // Feed the merged result back in as `ours` against the same base+theirs.
        let twice = merge_spec_three_way(&base, &once, &b);

        assert_eq!(once.tags, twice.tags, "tags duplicated/changed on re-merge");
        assert_eq!(
            once.comments.len(),
            twice.comments.len(),
            "comments duplicated on re-merge"
        );
        assert_eq!(once.effective_priority(), twice.effective_priority());
        assert_eq!(once.owner, twice.owner);
        assert_eq!(once.history.len(), twice.history.len());
    }

    // SCENARIO 7 — base == ours == theirs is a clean no-op (already covered for
    // content; this asserts arrays/relationships/deps too). trace:BUG-602
    #[test]
    fn test_merge_noop_all_fields() {
        let target = Uuid::now_v7();
        let dep = Uuid::now_v7();
        let mut base = make_req("Base", "Draft");
        base.tags.insert("t".to_string());
        base.relationships = vec![rel(RelationshipType::BlockedBy, target)];
        base.dependencies = vec![dep];
        base.history = vec![hist("base", "2026-01-01T00:00:00Z")];
        let ours = base.clone();
        let theirs = base.clone();

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        assert_eq!(merged.tags, base.tags);
        assert_eq!(merged.relationships.len(), 1);
        assert_eq!(merged.dependencies, vec![dep]);
        assert_eq!(merged.history.len(), 1);
    }

    // SCENARIO 7 — archived/deferred view flags: a flag flipped on only one side
    // (a scalar) survives; the concurrent unrelated scalar edit survives too.
    // trace:BUG-602
    #[test]
    fn test_merge_archived_and_deferred_flags() {
        let mut base = make_req("Base", "Draft");
        base.modified_at = ts("2026-01-01T00:00:00Z");

        let mut a = base.clone();
        a.archived = true;
        a.deferred = true;
        a.modified_at = ts("2026-01-02T00:00:00Z");

        let mut b = base.clone();
        b.owner = "bob".to_string();
        b.modified_at = ts("2026-01-03T00:00:00Z");

        for (x, y) in [(&a, &b), (&b, &a)] {
            let merged = merge_spec_three_way(&base, x, y);
            assert!(merged.archived, "archived flag lost");
            assert!(merged.deferred, "deferred flag lost");
            assert_eq!(merged.owner, "bob");
        }
    }

    // STORY-776 — the advisor's execution_mode set on only one side of a
    // divergent sync survives the merge regardless of which side wins the
    // object-level tie-break; a later concurrent unrelated edit does not
    // clobber it back to None. trace:STORY-776 | ai:claude
    #[test]
    fn test_merge_execution_mode_survives_one_sided_set() {
        let mut base = make_req("Base", "Draft");
        base.modified_at = ts("2026-01-01T00:00:00Z");

        let mut a = base.clone();
        a.execution_mode = Some(crate::ExecutionMode::Guided);
        a.modified_at = ts("2026-01-02T00:00:00Z");

        let mut b = base.clone();
        b.owner = "bob".to_string();
        b.modified_at = ts("2026-01-03T00:00:00Z");

        for (x, y) in [(&a, &b), (&b, &a)] {
            let merged = merge_spec_three_way(&base, x, y);
            assert_eq!(
                merged.execution_mode,
                Some(crate::ExecutionMode::Guided),
                "advisor-set execution_mode lost in sync merge"
            );
            assert_eq!(merged.owner, "bob");
        }
    }

    // Direct unit coverage of the 3-way set helper (the BUG-602 core). trace:BUG-602
    #[test]
    fn test_merge_string_set_three_way_rules() {
        use std::collections::HashSet;
        let set =
            |items: &[&str]| -> HashSet<String> { items.iter().map(|s| s.to_string()).collect() };
        // base={a,b}; ours removes b; theirs keeps both -> {a} (removal wins).
        assert_eq!(
            merge_string_set_three_way(&set(&["a", "b"]), &set(&["a"]), &set(&["a", "b"])),
            set(&["a"])
        );
        // base={a}; ours adds c; theirs adds d -> {a,c,d} (both adds survive).
        assert_eq!(
            merge_string_set_three_way(&set(&["a"]), &set(&["a", "c"]), &set(&["a", "d"])),
            set(&["a", "c", "d"])
        );
        // base={a}; ours removes a; theirs re-adds a (kept) -> removal wins -> {}.
        assert_eq!(
            merge_string_set_three_way(&set(&["a"]), &set(&[]), &set(&["a"])),
            set(&[])
        );
        // base={}; ours adds a; theirs adds a -> {a} (idempotent add).
        assert_eq!(
            merge_string_set_three_way(&set(&[]), &set(&["a"]), &set(&["a"])),
            set(&["a"])
        );
    }

    // trace:BUG-475 — emoji (4-byte) at the cutoff truncates on a char boundary.
    #[test]
    fn test_truncate_emoji_at_boundary() {
        let s = "a".repeat(99) + "😀" + &"b".repeat(10);
        let out = truncate(&s, 100);
        let expected: String = s.chars().take(100).collect();
        assert_eq!(out, format!("{expected}..."));
        // Sanity: the emoji survived intact (no mid-char slice).
        assert!(out.contains('😀'));
    }

    // ---- BUG-725: per-user queue registry three-way merge ----

    fn qe(id: Uuid, position: i64, added_secs: i64) -> crate::models::QueueEntry {
        crate::models::QueueEntry {
            user_id: "joe".to_string(),
            requirement_id: id,
            position,
            added_by: "joe".to_string(),
            note: None,
            added_at: DateTime::from_timestamp(1_750_000_000 + added_secs, 0).unwrap(),
            for_role: Some("implementer".to_string()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        }
    }

    fn ids(entries: &[crate::models::QueueEntry]) -> Vec<Uuid> {
        entries.iter().map(|e| e.requirement_id).collect()
    }

    // The BUG-725 headline case: the same user queues DIFFERENT specs from two
    // machines concurrently. Both adds must survive the merge.
    #[test]
    fn queue_merge_concurrent_adds_from_two_machines_both_survive() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let base = vec![qe(a, 1000, 0)];
        let ours = vec![qe(a, 1000, 0), qe(b, 2000, 10)]; // machine 1 queued b
        let theirs = vec![qe(a, 1000, 0), qe(c, 2000, 20)]; // machine 2 queued c
        let merged = merge_queue_three_way(&base, &ours, &theirs);
        let mut got = ids(&merged);
        got.sort();
        let mut want = vec![a, b, c];
        want.sort();
        assert_eq!(got, want, "concurrent adds must both survive");
    }

    // `aida queue done`/`remove` on one machine must NOT be resurrected by the
    // other machine's mere retention of the entry.
    #[test]
    fn queue_merge_removal_wins_over_retention() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let base = vec![qe(a, 1000, 0), qe(b, 2000, 5)];
        let ours = vec![qe(b, 2000, 5)]; // machine 1 completed a → removed
        let theirs = vec![qe(a, 1000, 0), qe(b, 2000, 5)]; // machine 2 untouched
        let merged = merge_queue_three_way(&base, &ours, &theirs);
        assert_eq!(ids(&merged), vec![b], "removal must win over retention");
    }

    // A reposition on ONE side wins over the other side's unchanged copy
    // (per-key 3-way, mirroring merge_scalar).
    #[test]
    fn queue_merge_single_side_reposition_wins() {
        let a = Uuid::new_v4();
        let base = vec![qe(a, 5000, 0)];
        let ours = vec![qe(a, 5000, 0)]; // unchanged
        let mut moved = qe(a, 1000, 0);
        moved.note = Some("bumped to top".to_string());
        let theirs = vec![moved.clone()]; // machine 2 repositioned
        let merged = merge_queue_three_way(&base, &ours, &theirs);
        assert_eq!(merged, vec![moved], "the changed side's entry must win");
    }

    // Both sides re-added/moved the SAME spec: later added_at wins, and the
    // result is identical with ours/theirs swapped (order-independence).
    #[test]
    fn queue_merge_both_changed_lww_and_order_independent() {
        let a = Uuid::new_v4();
        let base = vec![qe(a, 5000, 0)];
        let ours = vec![qe(a, 1000, 50)];
        let theirs = vec![qe(a, 9000, 100)]; // later add → wins
        let one = merge_queue_three_way(&base, &ours, &theirs);
        let two = merge_queue_three_way(&base, &theirs, &ours);
        assert_eq!(one, vec![qe(a, 9000, 100)], "later added_at must win");
        assert_eq!(one, two, "merge must be order-independent across clones");
    }

    // Exact added_at tie with divergent content: both clones must converge on
    // the SAME winner no matter which side they call ours (BUG-578 precedent).
    #[test]
    fn queue_merge_exact_tie_is_deterministic() {
        let a = Uuid::new_v4();
        let ours = vec![qe(a, 1000, 0)];
        let theirs = vec![qe(a, 2000, 0)]; // same added_at, different position
        let one = merge_queue_three_way(&[], &ours, &theirs);
        let two = merge_queue_three_way(&[], &theirs, &ours);
        assert_eq!(one, two, "tie-break must be a pure data-function");
        assert_eq!(one.len(), 1);
    }

    // add/add with no base (both machines created the file concurrently):
    // everything unions, dedup by requirement_id, sorted by position.
    #[test]
    fn queue_merge_no_base_unions_and_sorts() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let ours = vec![qe(a, 2000, 0)];
        let theirs = vec![qe(b, 1000, 5), qe(a, 2000, 0)];
        let merged = merge_queue_three_way(&[], &ours, &theirs);
        assert_eq!(merged, vec![qe(b, 1000, 5), qe(a, 2000, 0)]);
    }
}

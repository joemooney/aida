//! Canonical pre-registration policy. Compute the entire plan before writing.
// trace:STORY-1675 | ai:codex
use crate::models::{HypothesisLock, RelationshipType, Requirement, RequirementType};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// The pre-registration tag. Matched case-insensitively so `Hypothesis` and
/// `HYPOTHESIS` cannot slip past the lock. trace:STORY-1675 | ai:claude
pub(crate) fn has_tag(r: &Requirement) -> bool {
    r.tags.iter().any(|t| t.eq_ignore_ascii_case("hypothesis"))
}

fn sha256_hex(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Byte-identical row (no `PartialEq` on `Requirement`). Only consulted for
/// stamped rows, which are few, so serializing is cheap.
fn same_row(a: &Requirement, b: &Requirement) -> bool {
    match (serde_json::to_string(a), serde_json::to_string(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Rows with a canonical identity whose `References`/`VerifiedBy` edge joins
/// a hypothesis to a completed result, in either direction.
fn eligible(
    graph: &[Requirement],
    hypotheses: &HashSet<Uuid>,
    completed: &HashSet<Uuid>,
) -> HashSet<Uuid> {
    let identified: HashSet<_> = graph
        .iter()
        .filter(|r| r.spec_id.is_some())
        .map(|r| r.id)
        .collect();
    eligible_edges(&result_edges(graph, &identified), hypotheses, completed)
}

/// Qualifying edges whose endpoints both have a canonical identity in `known`.
fn result_edges<'a>(
    graph: impl IntoIterator<Item = &'a Requirement>,
    known: &HashSet<Uuid>,
) -> HashSet<(Uuid, Uuid)> {
    graph
        .into_iter()
        .filter(|source| source.spec_id.is_some())
        .flat_map(|source| {
            source
                .relationships
                .iter()
                .filter(|edge| {
                    matches!(
                        edge.rel_type,
                        RelationshipType::References | RelationshipType::VerifiedBy
                    ) && known.contains(&edge.target_id)
                })
                .map(move |edge| (source.id, edge.target_id))
        })
        .collect()
}

// trace:BUG-2162 | ai:claude
fn eligible_edges(
    edges: &HashSet<(Uuid, Uuid)>,
    hypotheses: &HashSet<Uuid>,
    completed: &HashSet<Uuid>,
) -> HashSet<Uuid> {
    let mut result = HashSet::new();
    for &(source, target) in edges {
        if hypotheses.contains(&source) && completed.contains(&target) {
            result.insert(source);
        }
        if hypotheses.contains(&target) && completed.contains(&source) {
            result.insert(target);
        }
    }
    result
}

fn is_hypothesis(r: &Requirement) -> bool {
    r.spec_id.is_some()
        && has_tag(r)
        && matches!(
            r.req_type,
            RequirementType::Spike | RequirementType::Decision
        )
}

/// Whether a snapshot holds any stamp or hypothesis-tagged row.
// trace:BUG-2162 | ai:claude
pub(crate) fn holds_evidence(graph: &[Requirement]) -> bool {
    graph
        .iter()
        .any(|r| r.hypothesis_lock.is_some() || is_hypothesis(r))
}

fn is_completed(r: &Requirement) -> bool {
    r.spec_id.is_some() && r.effective_status().eq_ignore_ascii_case("completed")
}

/// Preserve existing stamps and materialize first-result locks on the planned
/// graph. Looking at BOTH graphs prevents unlink/reopen/tag removal from erasing
/// a result that already exists, including data loaded from older binaries.
/// A description change in the same transaction as the first result is refused.
pub(crate) fn enforce(before: &[Requirement], after: &mut [Requirement]) -> Result<()> {
    let mut ids = HashSet::new();
    let mut specs = HashSet::new();
    for row in after.iter() {
        if !ids.insert(row.id)
            || row
                .spec_id
                .as_ref()
                .is_some_and(|sid| !specs.insert(crate::object_store::canonical_spec_id(sid)))
        {
            anyhow::bail!("duplicate requirement in mutation graph; nothing was written");
        }
    }
    let old: HashMap<_, _> = before.iter().map(|r| (r.id, r)).collect();
    // A result boundary cannot be hidden by stripping the tag or edge in
    // that SAME transaction. Cancellation in an earlier transaction remains
    // allowed while there is no completed result.
    let hypotheses: HashSet<_> = before
        .iter()
        .chain(after.iter())
        .filter(|r| is_hypothesis(r))
        .map(|r| r.id)
        .collect();
    let completed: HashSet<_> = before
        .iter()
        .chain(after.iter())
        .filter(|r| is_completed(r))
        .map(|r| r.id)
        .collect();
    let mut locked = eligible(before, &hypotheses, &completed);
    locked.extend(eligible(after, &hypotheses, &completed));
    let by_spec: HashMap<_, _> = before
        .iter()
        .filter_map(|r| {
            r.spec_id
                .as_deref()
                .map(|s| (crate::object_store::canonical_spec_id(s), r))
        })
        .collect();
    let after_ids: HashSet<_> = after.iter().map(|r| r.id).collect();
    for row in before {
        if (row.hypothesis_lock.is_some() || locked.contains(&row.id))
            && !after_ids.contains(&row.id)
        {
            anyhow::bail!("{} has a locked hypothesis description; preserve it and append amendments with aida comment add {} instead", row.display_id(), row.display_id());
        }
    }
    let now = chrono::Utc::now();
    for next in after {
        let previous = old.get(&next.id).copied();
        if previous.is_some_and(|old| {
            (old.hypothesis_lock.is_some() || locked.contains(&old.id))
                && old.spec_id != next.spec_id
        }) {
            anyhow::bail!("a locked hypothesis identity cannot change; use aida comment add {} for amendments", previous.unwrap().display_id());
        }
        if let Some(stored) = next
            .spec_id
            .as_deref()
            .and_then(|s| by_spec.get(&crate::object_store::canonical_spec_id(s)))
        {
            if stored.id != next.id
                && (stored.hypothesis_lock.is_some() || locked.contains(&stored.id))
            {
                anyhow::bail!("{} cannot replace a locked hypothesis identity; use aida comment add {} instead", stored.display_id(), stored.display_id());
            }
        }
        let stamp = previous
            .and_then(|r| r.hypothesis_lock.clone())
            .or_else(|| next.hypothesis_lock.clone())
            .or_else(|| {
                locked.contains(&next.id).then(|| HypothesisLock {
                    locked_at: now,
                    description_sha256: sha256_hex(&previous.unwrap_or(next).description),
                })
            });
        // Hash-check only the write set: an untouched stamped row (even one a
        // text merge corrupted) must not block unrelated writes store-wide.
        // A write whose NEW description matches the frozen hash is accepted,
        // which is how `aida edit` repairs a corrupted body. Every other
        // change to a mismatched row is refused. trace:STORY-1675 | ai:claude
        if let Some(stamp) = &stamp {
            let untouched = previous.is_some_and(|p| same_row(p, next));
            let frozen = |text: &str| sha256_hex(text) == stamp.description_sha256;
            if !untouched && !frozen(&next.description) {
                if previous.is_some_and(|p| frozen(&p.description)) {
                    anyhow::bail!(
                        "{} has a locked hypothesis description; append amendments with aida comment add {} instead",
                        next.spec_id.as_deref().unwrap_or("hypothesis"),
                        next.spec_id.as_deref().unwrap_or("<ID>")
                    );
                }
                anyhow::bail!("{} has inconsistent locked hypothesis evidence; restore the frozen description from store history before using aida comment add {} for amendments", next.display_id(), next.display_id());
            }
        }
        // Incoming copies cannot clear or replace a canonical stamp. Newly
        // imported rows may retain their original pre-registration evidence.
        if previous.is_some() || stamp.is_some() {
            next.hypothesis_lock = stamp;
        }
    }
    Ok(())
}

/// A divergent store merge cannot drop a stamp or replace its frozen body with
/// an offline edit made before the other clone observed the result. Keep the
/// base lock if present; otherwise converge on the first recorded lock, with a
/// deterministic hash tie-break. Other fields retain normal merge semantics.
// trace:STORY-1675 | ai:codex
pub(crate) fn preserve_merge(
    base: Option<&Requirement>,
    a: &Requirement,
    b: &Requirement,
    merged: &mut Requirement,
) {
    let carrier = base.filter(|r| r.hypothesis_lock.is_some()).or_else(|| {
        [a, b]
            .into_iter()
            .filter(|r| r.hypothesis_lock.is_some())
            .min_by_key(|r| {
                let stamp = r.hypothesis_lock.as_ref().unwrap();
                (stamp.locked_at, stamp.description_sha256.as_str())
            })
    });
    if let Some(carrier) = carrier {
        merged.hypothesis_lock = carrier.hypothesis_lock.clone();
        merged.description = carrier.description.clone();
    }
}

/// A store synchronization would expose changed or ambiguous pre-registration
/// evidence, or could not prove it intact after Git finished. Fatal for every
/// caller: never treated as an offline pull, never followed by a publish.
// trace:BUG-2162 | ai:claude
#[derive(Debug)]
pub struct EvidenceRefusal(pub String);

impl std::fmt::Display for EvidenceRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EvidenceRefusal {}

/// Whether `err`, through any added context, is an [`EvidenceRefusal`].
// trace:BUG-2162 | ai:claude
pub fn is_evidence_refusal(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.is::<EvidenceRefusal>())
}

/// Evidence a synchronized graph must carry for one hypothesis row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Frozen {
    pub spec: String,
    pub lock: HypothesisLock,
}

/// Every protected row of one synchronization, keyed by UUID.
pub(crate) type SyncPlan = HashMap<Uuid, Frozen>;

type Indexed<'a> = (HashMap<Uuid, &'a Requirement>, HashMap<String, Uuid>);

fn spec_of(row: &Requirement) -> std::result::Result<String, String> {
    row.spec_id
        .as_deref()
        .map(crate::object_store::canonical_spec_id)
        .ok_or_else(|| format!("hypothesis row {} has no ID", row.id))
}

/// Index one strictly read snapshot: unique UUIDs and IDs, valid stamps.
fn index_snapshot(graph: &[Requirement]) -> std::result::Result<Indexed<'_>, String> {
    let mut by_id = HashMap::new();
    let mut by_spec = HashMap::new();
    for row in graph {
        if by_id.insert(row.id, row).is_some() {
            return Err(format!(
                "{} appears twice in one store snapshot",
                row.display_id()
            ));
        }
        if let Some(sid) = &row.spec_id {
            if by_spec
                .insert(crate::object_store::canonical_spec_id(sid), row.id)
                .is_some()
            {
                return Err(format!("{sid} is used by two rows in one store snapshot"));
            }
        }
        if let Some(stamp) = &row.hypothesis_lock {
            if sha256_hex(&row.description) != stamp.description_sha256 {
                return Err(format!(
                    "{} carries a hypothesis lock that does not match its description; restore the frozen description from store history first",
                    row.display_id()
                ));
            }
        }
    }
    Ok((by_id, by_spec))
}

/// Plan the evidence one store synchronization must preserve, from the merge
/// bases and input tips of the Git operation, before Git runs.
///
/// A stamped base is authoritative. Without one, stamped tips must agree on
/// the frozen body and converge on the earliest stamp, as [`preserve_merge`]
/// does. Every extant tip version must carry the frozen body and identity; a
/// row present in a base but missing from a tip is a deletion. A hypothesis
/// that the union of all inputs links to a completed result is newly locked
/// at `now`, but only when every extant tip agrees on its body: there is no
/// causality guess between divergent pre-result descriptions.
// trace:BUG-2162 | ai:claude
pub(crate) fn plan_sync(
    bases: &[&[Requirement]],
    tips: &[&[Requirement]],
    now: chrono::DateTime<chrono::Utc>,
) -> std::result::Result<SyncPlan, String> {
    let bases = bases
        .iter()
        .map(|g| index_snapshot(g))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let tips = tips
        .iter()
        .map(|g| index_snapshot(g))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let all = || bases.iter().chain(tips.iter());
    let rows = || all().flat_map(|(rows, _)| rows.values().copied());
    fn carriers<'a>(graphs: &[Indexed<'a>], id: &Uuid) -> Vec<&'a Requirement> {
        graphs
            .iter()
            .filter_map(|(rows, _)| rows.get(id).copied())
            .filter(|r| r.hypothesis_lock.is_some())
            .collect()
    }

    let mut plan = SyncPlan::new();
    let stamped: HashSet<Uuid> = rows()
        .filter(|r| r.hypothesis_lock.is_some())
        .map(|r| r.id)
        .collect();
    for id in stamped {
        let from_base = carriers(&bases, &id);
        let carriers = if from_base.is_empty() {
            carriers(&tips, &id)
        } else {
            from_base
        };
        let locks: Vec<&HypothesisLock> = carriers
            .iter()
            .filter_map(|r| r.hypothesis_lock.as_ref())
            .collect();
        if locks
            .iter()
            .any(|l| l.description_sha256 != locks[0].description_sha256)
        {
            return Err(format!(
                "{} was locked with different hypothesis descriptions in the two stores; reconcile the description in one store before syncing",
                carriers[0].display_id()
            ));
        }
        let lock = locks
            .into_iter()
            .min_by_key(|l| l.locked_at)
            .cloned()
            .unwrap();
        plan.insert(
            id,
            Frozen {
                spec: spec_of(carriers[0])?,
                lock,
            },
        );
    }

    // Project result eligibility across every input, so a tag, type, edge or
    // status removed on one side cannot cancel evidence the other side holds.
    let known: HashSet<_> = rows()
        .filter(|r| r.spec_id.is_some())
        .map(|r| r.id)
        .collect();
    let hypotheses: HashSet<_> = rows().filter(|r| is_hypothesis(r)).map(|r| r.id).collect();
    let completed: HashSet<_> = rows().filter(|r| is_completed(r)).map(|r| r.id).collect();
    let mut newly: Vec<Uuid> =
        eligible_edges(&result_edges(rows(), &known), &hypotheses, &completed)
            .into_iter()
            .filter(|id| !plan.contains_key(id))
            .collect();
    newly.sort();
    for id in newly {
        let versions: Vec<&Requirement> = tips
            .iter()
            .filter_map(|(rows, _)| rows.get(&id).copied())
            .collect();
        let Some(first) = versions.first() else {
            let row = rows().find(|r| r.id == id).unwrap();
            return Err(format!(
                "{} links to a completed result but one store deleted it; restore it before syncing",
                row.display_id()
            ));
        };
        if versions.iter().any(|r| r.description != first.description) {
            return Err(format!(
                "{} reached its first completed result while its description differs between the two stores; agree on one pre-result description in one store, then sync",
                first.display_id()
            ));
        }
        plan.insert(
            id,
            Frozen {
                spec: spec_of(first)?,
                lock: HypothesisLock {
                    locked_at: now,
                    description_sha256: sha256_hex(&first.description),
                },
            },
        );
    }

    for (id, frozen) in &plan {
        for (rows, by_spec) in all() {
            if let Some(row) = rows.get(id) {
                if spec_of(row)? != frozen.spec {
                    return Err(format!(
                        "the locked hypothesis {} changed its ID to {}; keep its original ID",
                        frozen.spec,
                        row.display_id()
                    ));
                }
            }
            if by_spec.get(&frozen.spec).is_some_and(|other| other != id) {
                return Err(format!(
                    "{} is a locked hypothesis but another row uses that ID",
                    frozen.spec
                ));
            }
        }
        for (rows, _) in &tips {
            match rows.get(id) {
                Some(row) if sha256_hex(&row.description) != frozen.lock.description_sha256 => {
                    return Err(format!(
                        "{} has a locked hypothesis description that one store changed; restore the frozen description there and append amendments with aida comment add {} instead",
                        frozen.spec, frozen.spec
                    ));
                }
                None if bases.iter().any(|(rows, _)| rows.contains_key(id)) => {
                    return Err(format!(
                        "{} is a locked hypothesis that one store deleted; restore it before syncing",
                        frozen.spec
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(plan)
}

/// Check a synchronized graph against the evidence it must carry. The
/// candidate joins the inputs as one more tip, so eligibility only the
/// combined result reveals meets the same agreement rule. Returns the rows
/// whose lock alone must be written; any other difference is a refusal.
// trace:BUG-2162 | ai:claude
pub(crate) fn finalize_sync(
    bases: &[&[Requirement]],
    tips: &[&[Requirement]],
    candidate: &[Requirement],
    now: chrono::DateTime<chrono::Utc>,
) -> std::result::Result<Vec<Requirement>, String> {
    let mut with_candidate = tips.to_vec();
    with_candidate.push(candidate);
    let plan = plan_sync(bases, &with_candidate, now)?;
    let rows: HashMap<_, _> = candidate.iter().map(|r| (r.id, r)).collect();
    let mut writes = Vec::new();
    for (id, frozen) in &plan {
        let Some(row) = rows.get(id) else {
            return Err(format!(
                "the synchronized store lost the locked hypothesis {}",
                frozen.spec
            ));
        };
        if row.hypothesis_lock.as_ref() != Some(&frozen.lock) {
            let mut row = (*row).clone();
            row.hypothesis_lock = Some(frozen.lock.clone());
            writes.push(row);
        }
    }
    writes.sort_by(|a, b| a.spec_id.cmp(&b.spec_id));
    Ok(writes)
}

//! Canonical pre-registration policy. Compute the entire plan before writing.
// trace:STORY-1675 | ai:codex
use crate::models::{HypothesisLock, RelationshipType, Requirement, RequirementType};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

fn eligible(
    graph: &[Requirement],
    hypotheses: &HashSet<Uuid>,
    completed: &HashSet<Uuid>,
) -> HashSet<Uuid> {
    let by_id: HashMap<_, _> = graph.iter().map(|r| (r.id, r)).collect();
    let mut result = HashSet::new();
    for source in graph {
        if source.spec_id.is_none() {
            continue;
        }
        for edge in &source.relationships {
            if !matches!(
                edge.rel_type,
                RelationshipType::References | RelationshipType::VerifiedBy
            ) {
                continue;
            }
            let Some(target) = by_id.get(&edge.target_id) else {
                continue;
            };
            if target.spec_id.is_none() {
                continue;
            }
            if hypotheses.contains(&source.id) && completed.contains(&target.id) {
                result.insert(source.id);
            }
            if hypotheses.contains(&target.id) && completed.contains(&source.id) {
                result.insert(target.id);
            }
        }
    }
    result
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
                .is_some_and(|sid| !specs.insert(sid.to_ascii_uppercase()))
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
        .filter(|r| {
            r.spec_id.is_some()
                && r.tags.contains("hypothesis")
                && matches!(
                    r.req_type,
                    RequirementType::Spike | RequirementType::Decision
                )
        })
        .map(|r| r.id)
        .collect();
    let completed: HashSet<_> = before
        .iter()
        .chain(after.iter())
        .filter(|r| r.spec_id.is_some() && r.effective_status().eq_ignore_ascii_case("completed"))
        .map(|r| r.id)
        .collect();
    let mut locked = eligible(before, &hypotheses, &completed);
    locked.extend(eligible(after, &hypotheses, &completed));
    let by_spec: HashMap<_, _> = before
        .iter()
        .filter_map(|r| r.spec_id.as_ref().map(|s| (s, r)))
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
        if let Some(stored) = next.spec_id.as_ref().and_then(|s| by_spec.get(s)) {
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
                    description_sha256: format!(
                        "{:x}",
                        Sha256::digest(previous.unwrap_or(next).description.as_bytes())
                    ),
                })
            });
        if let Some(stamp) = &stamp {
            if stamp.description_sha256
                != format!(
                    "{:x}",
                    Sha256::digest(previous.unwrap_or(next).description.as_bytes())
                )
            {
                anyhow::bail!("{} has inconsistent locked hypothesis evidence; restore the frozen description from store history before using aida comment add {} for amendments", next.display_id(), next.display_id());
            }
        }
        if stamp.is_some() && previous.is_some_and(|r| r.description != next.description) {
            anyhow::bail!(
                "{} has a locked hypothesis description; append amendments with aida comment add {} instead",
                next.spec_id.as_deref().unwrap_or("hypothesis"),
                next.spec_id.as_deref().unwrap_or("<ID>")
            );
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

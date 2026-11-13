// trace:TASK-3428 | ai:codex
use anyhow::Result;
/// TASK-589: assemble the discipline-pack glossary from the binary's embedded
/// templates — the product glossary, the machinery glossary and/or the
/// lifecycle vocabulary. Reads the embedded copy (not the project's scaffolded
/// files), so it is correct even when a project's `.aida/discipline/` is
/// missing or stale. With no flag it returns every section in the order
/// product, machinery, lifecycle, joined by a blank line; any flag selects only
/// the sections it names, in that same order.
/// trace:TASK-589 | ai:claude
/// trace:STORY-2219 | ai:claude
pub(crate) fn render_discipline_glossary(
    product: bool,
    machinery: bool,
    lifecycle: bool,
) -> Result<String> {
    let templates = aida_core::get_embedded_templates();
    let embedded = |key: &str| -> Result<String> {
        templates
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.content.clone())
            .ok_or_else(|| anyhow::anyhow!("embedded glossary `{key}` not found in this binary"))
    };
    let all = !(product || machinery || lifecycle);
    let sections = [
        (product, ".aida/discipline/product-glossary.md"),
        (machinery, ".aida/discipline/machinery-glossary.md"),
        (lifecycle, ".aida/discipline/lifecycle-vocabulary.md"),
    ];
    let mut parts = Vec::new();
    for (selected, key) in sections {
        if all || selected {
            parts.push(embedded(key)?);
        }
    }
    Ok(parts.join("\n\n"))
}

/// Pure selection for the release-time doc-coverage gate (TASK-680).
///
/// Returns the requirements that **reached Completed at or after `cutoff`** and
/// have **no Doc entry referencing them** (no `Doc`-typed requirement carries a
/// `References` edge pointing at them). When `cutoff` is `None`, every spec that
/// ever reached Completed is considered (full-history scan — used when the repo
/// has no tags yet).
///
/// "Reached Completed" is read from the spec's transition `history`: any
/// `HistoryEntry` whose `changes` include a `status` field whose `new_value`
/// is `Completed`. As a fallback for specs that are currently Completed but
/// carry no such history row (e.g. imported/legacy data), the spec's
/// `modified_at` is used as the transition time.
///
/// Doc-typed requirements and archived specs are never themselves reported as
/// gaps (a doc doesn't need a doc about it).
///
/// trace:TASK-680 | ai:claude
pub(crate) fn find_uncovered_completed_specs(
    requirements: &[aida_core::models::Requirement],
    cutoff: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<aida_core::models::Requirement> {
    use aida_core::models::{RelationshipType, RequirementStatus, RequirementType};

    // Set of spec uuids that at least one Doc References.
    let mut documented: std::collections::HashSet<uuid::Uuid> = std::collections::HashSet::new();
    for doc in requirements
        .iter()
        .filter(|r| r.req_type == RequirementType::Doc)
    {
        for rel in &doc.relationships {
            if rel.rel_type == RelationshipType::References {
                documented.insert(rel.target_id);
            }
        }
    }

    let completed_str = RequirementStatus::Completed.to_string();

    let mut gaps: Vec<aida_core::models::Requirement> = Vec::new();
    for req in requirements {
        // Docs and archived specs are out of scope.
        if req.req_type == RequirementType::Doc || req.archived {
            continue;
        }

        // When did this spec reach Completed (if ever)?
        let completed_at: Option<chrono::DateTime<chrono::Utc>> = req
            .history
            .iter()
            .filter(|h| {
                h.changes
                    .iter()
                    .any(|c| c.field_name == "status" && c.new_value == completed_str)
            })
            .map(|h| h.timestamp)
            .max()
            .or_else(|| {
                // Fallback: currently Completed but no history row recording it.
                if req.status == RequirementStatus::Completed {
                    Some(req.modified_at)
                } else {
                    None
                }
            });

        let Some(reached_at) = completed_at else {
            continue;
        };

        // Inside the release window?
        if let Some(c) = cutoff {
            if reached_at < c {
                continue;
            }
        }

        // Already documented?
        if documented.contains(&req.id) {
            continue;
        }

        gaps.push(req.clone());
    }

    gaps.sort_by_key(|a| a.display_id());
    gaps
}

#[cfg(test)]
#[path = "tests/glossary_render_tests.rs"]
mod glossary_render_tests;

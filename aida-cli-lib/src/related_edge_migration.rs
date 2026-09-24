//! `aida db migrate-related-edges` — repair the graph-inert custom
//! "related" edges written before `rel add --type related` was aliased to the
//! standard `References` edge.
//!
//! Three legacy spellings are in scope: `related`, `related-to` and
//! `relates-to` (case-insensitive). Every other custom edge (`implements`,
//! `implemented-by`, `sprint_*`, ...) is left untouched.
//!
//! Per (source, target) pair holding at least one legacy edge:
//!   * a `References` edge to the same target already exists (a "twin"):
//!     every legacy edge to that target is deleted;
//!   * otherwise (an "orphan"): the first legacy edge is converted to
//!     `References` in place (keeping its created_at / created_by) and any
//!     further legacy edges to the same target are deleted, so the
//!     conversion never produces two `References` edges.
//!
//! The plan is computed from the live store on every run (nothing is
//! hardcoded), checked for duplicate edges before anything is written, then
//! applied one spec at a time through the backend's single-spec update path
//! (one targeted store commit per spec, never a full-store rewrite), and the
//! written specs are re-read and checked again afterwards. A second run finds
//! nothing to do.
// trace:TASK-1426 | ai:claude

use std::collections::{BTreeMap, HashMap};

use aida_core::models::{Relationship, RelationshipType, Requirement};
use aida_core::DatabaseBackend;
use anyhow::Result;
use serde::Serialize;
use uuid::Uuid;

/// Legacy custom spellings that mean "references" and that no graph traversal
/// follows.
pub(crate) const LEGACY_RELATED_SPELLINGS: [&str; 3] = ["related", "related-to", "relates-to"];

/// True when `rel_type` is one of the legacy custom "related" spellings.
// trace:TASK-1426 | ai:claude
pub(crate) fn is_legacy_related(rel_type: &RelationshipType) -> bool {
    match rel_type {
        RelationshipType::Custom(name) => {
            let name = name.trim().to_ascii_lowercase();
            LEGACY_RELATED_SPELLINGS.contains(&name.as_str())
        }
        _ => false,
    }
}

/// What happens to one legacy edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EdgeAction {
    /// Orphan: rewritten in place as a `References` edge.
    Convert,
    /// A `References` edge to the same target already exists.
    DeleteTwin,
    /// A second legacy edge to a target whose first legacy edge is converted.
    DeleteExtra,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PlannedEdge {
    pub action: EdgeAction,
    /// The stored custom spelling, e.g. `related-to`.
    pub from_type: String,
    pub target_id: Uuid,
    /// Display id of the target, when it resolves in the store.
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SpecPlan {
    pub spec_id: String,
    #[serde(skip)]
    pub uuid: Uuid,
    pub edges: Vec<PlannedEdge>,
    /// The spec's relationship list after the migration.
    #[serde(skip)]
    pub new_relationships: Vec<Relationship>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct MigrationPlan {
    pub specs_scanned: usize,
    pub legacy_edges: usize,
    pub converted: usize,
    pub twins_deleted: usize,
    pub extras_deleted: usize,
    /// Legacy edge count per stored spelling.
    pub by_spelling: BTreeMap<String, usize>,
    pub specs: Vec<SpecPlan>,
}

impl MigrationPlan {
    pub fn is_noop(&self) -> bool {
        self.specs.is_empty()
    }
}

fn display_id(req: &Requirement) -> String {
    req.agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| req.id.to_string())
}

/// One legacy edge's fate: action, stored spelling, target.
type EdgeDecision = (EdgeAction, String, Uuid);

/// Compute the migration for one spec's relationship list. Returns `None`
/// when the spec holds no legacy edge.
// trace:TASK-1426 | ai:claude
fn plan_relationships(
    relationships: &[Relationship],
) -> Option<(Vec<Relationship>, Vec<EdgeDecision>)> {
    if !relationships.iter().any(|r| is_legacy_related(&r.rel_type)) {
        return None;
    }
    let mut out = Vec::with_capacity(relationships.len());
    let mut actions = Vec::new();
    // Targets that already carry a References edge (before or after a
    // conversion earlier in this pass).
    let mut has_references: std::collections::HashSet<Uuid> = relationships
        .iter()
        .filter(|r| r.rel_type == RelationshipType::References)
        .map(|r| r.target_id)
        .collect();
    let preexisting_references = has_references.clone();
    for rel in relationships {
        let RelationshipType::Custom(name) = &rel.rel_type else {
            out.push(rel.clone());
            continue;
        };
        if !is_legacy_related(&rel.rel_type) {
            out.push(rel.clone());
            continue;
        }
        if preexisting_references.contains(&rel.target_id) {
            actions.push((EdgeAction::DeleteTwin, name.clone(), rel.target_id));
        } else if has_references.contains(&rel.target_id) {
            actions.push((EdgeAction::DeleteExtra, name.clone(), rel.target_id));
        } else {
            has_references.insert(rel.target_id);
            actions.push((EdgeAction::Convert, name.clone(), rel.target_id));
            out.push(Relationship {
                rel_type: RelationshipType::References,
                ..rel.clone()
            });
        }
    }
    Some((out, actions))
}

/// Build the migration plan from the current store contents.
// trace:TASK-1426 | ai:claude
pub(crate) fn plan_migration(requirements: &[Requirement]) -> MigrationPlan {
    let names: HashMap<Uuid, String> = requirements.iter().map(|r| (r.id, display_id(r))).collect();
    let mut plan = MigrationPlan {
        specs_scanned: requirements.len(),
        ..Default::default()
    };
    for req in requirements {
        let Some((new_relationships, actions)) = plan_relationships(&req.relationships) else {
            continue;
        };
        let mut edges = Vec::with_capacity(actions.len());
        for (action, from_type, target_id) in actions {
            plan.legacy_edges += 1;
            *plan.by_spelling.entry(from_type.clone()).or_default() += 1;
            match action {
                EdgeAction::Convert => plan.converted += 1,
                EdgeAction::DeleteTwin => plan.twins_deleted += 1,
                EdgeAction::DeleteExtra => plan.extras_deleted += 1,
            }
            edges.push(PlannedEdge {
                action,
                from_type,
                target_id,
                target: names.get(&target_id).cloned(),
            });
        }
        plan.specs.push(SpecPlan {
            spec_id: display_id(req),
            uuid: req.id,
            edges,
            new_relationships,
        });
    }
    plan.specs.sort_by(|a, b| a.spec_id.cmp(&b.spec_id));
    plan
}

/// Edge kinds that a source holds more than once toward the same target.
// trace:TASK-1426 | ai:claude
pub(crate) fn duplicate_edges(relationships: &[Relationship]) -> Vec<(RelationshipType, Uuid)> {
    let mut seen: HashMap<(&RelationshipType, Uuid), usize> = HashMap::new();
    for r in relationships {
        *seen.entry((&r.rel_type, r.target_id)).or_default() += 1;
    }
    let mut dups: Vec<(RelationshipType, Uuid)> = seen
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|((t, id), _)| (t.clone(), id))
        .collect();
    dups.sort_by_key(|(t, id)| (format!("{:?}", t), *id));
    dups
}

/// The post-migration invariant for one spec: no legacy edge left, and no
/// target reached twice by the same edge kind.
// trace:TASK-1426 | ai:claude
pub(crate) fn check_migrated(spec_id: &str, relationships: &[Relationship]) -> Result<()> {
    if relationships.iter().any(|r| is_legacy_related(&r.rel_type)) {
        anyhow::bail!("{spec_id} still holds a custom related edge after migration");
    }
    let dups = duplicate_edges(relationships);
    if let Some((kind, target)) = dups.first() {
        anyhow::bail!(
            "{spec_id} holds more than one {kind} edge to {target}; refusing to leave duplicate edges"
        );
    }
    Ok(())
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct MigrationOutcome {
    pub dry_run: bool,
    #[serde(flatten)]
    pub plan: MigrationPlan,
    pub specs_written: usize,
}

/// Plan, check, and (unless `dry_run`) apply the migration against `backend`.
/// Each affected spec is written with its own `update_requirement` call.
// trace:TASK-1426 | ai:claude
pub(crate) fn run_migration(
    backend: &dyn DatabaseBackend,
    dry_run: bool,
) -> Result<MigrationOutcome> {
    let store = backend.load()?;
    let plan = plan_migration(&store.requirements);

    // Pre-flight: refuse before any write when the planned result would
    // violate the invariant (e.g. a spec that already holds a duplicate edge).
    for spec in &plan.specs {
        check_migrated(&spec.spec_id, &spec.new_relationships)?;
    }

    let mut outcome = MigrationOutcome {
        dry_run,
        plan,
        specs_written: 0,
    };
    if dry_run || outcome.plan.is_noop() {
        return Ok(outcome);
    }

    for spec in &outcome.plan.specs {
        // Re-read right before the write so a concurrent edit to another
        // field of this spec is not clobbered by the snapshot above.
        let Some(mut req) = backend.get_requirement(&spec.uuid)? else {
            anyhow::bail!("{} disappeared during migration", spec.spec_id);
        };
        let Some((new_relationships, _)) = plan_relationships(&req.relationships) else {
            continue;
        };
        check_migrated(&spec.spec_id, &new_relationships)?;
        req.relationships = new_relationships;
        req.modified_at = chrono::Utc::now();
        backend.update_requirement(&req)?;
        outcome.specs_written += 1;

        let Some(written) = backend.get_requirement(&spec.uuid)? else {
            anyhow::bail!("{} could not be re-read after migration", spec.spec_id);
        };
        check_migrated(&spec.spec_id, &written.relationships)?;
    }
    Ok(outcome)
}

/// Human-readable report.
// trace:TASK-1426 | ai:claude
pub(crate) fn render_outcome(outcome: &MigrationOutcome) -> String {
    use std::fmt::Write as _;
    let plan = &outcome.plan;
    let mut s = String::new();
    if plan.is_noop() {
        let _ = writeln!(
            s,
            "No custom related edges to migrate ({} specs scanned).",
            plan.specs_scanned
        );
        return s;
    }
    let spellings = plan
        .by_spelling
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        s,
        "{} custom related edge(s) on {} spec(s) ({}); {} specs scanned.",
        plan.legacy_edges,
        plan.specs.len(),
        spellings,
        plan.specs_scanned
    );
    let _ = writeln!(
        s,
        "  convert to references: {}\n  delete (references edge already present): {}\n  delete (repeat of a converted edge): {}",
        plan.converted, plan.twins_deleted, plan.extras_deleted
    );
    let _ = writeln!(s);
    for spec in &plan.specs {
        let _ = writeln!(s, "{}", spec.spec_id);
        for e in &spec.edges {
            let target = e.target.clone().unwrap_or_else(|| e.target_id.to_string());
            let verb = match e.action {
                EdgeAction::Convert => "convert",
                EdgeAction::DeleteTwin => "delete ",
                EdgeAction::DeleteExtra => "delete ",
            };
            let why = match e.action {
                EdgeAction::Convert => "-> references",
                EdgeAction::DeleteTwin => "(references edge already present)",
                EdgeAction::DeleteExtra => "(repeat of the converted edge)",
            };
            let _ = writeln!(s, "  {verb} {} -> {target} {why}", e.from_type);
        }
    }
    let _ = writeln!(s);
    if outcome.dry_run {
        let _ = writeln!(
            s,
            "Dry run: nothing written. Re-run without --dry-run to apply."
        );
    } else {
        let _ = writeln!(
            s,
            "Migrated {} spec(s), one store commit each.",
            outcome.specs_written
        );
    }
    s
}

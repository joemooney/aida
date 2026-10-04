//! `aida db migrate-related-edges` — repair graph-inert custom edges written
//! before the shared relationship-type parser existed.
//!
//! Scope: every `Custom(name)` edge whose stored spelling parses to a
//! *standard* [`RelationshipType`] via
//! [`RelationshipType::parse_relationship_type`] (TASK-1488), plus the
//! original TASK-1426 `related-to` family (see [`standard_type_for`] for why
//! that one spelling needs a carve-out ahead of the shared parser). This is
//! a strict superset of the original TASK-1426 scope (`related` /
//! `related-to` / `relates-to` → `references`): it also covers spellings
//! like `depends-on` → `blocked_by`, `verified_by` → `verified_by`,
//! `replaced_by` → `superseded_by`, `duplicate-of` → `duplicate` (BUG-1604),
//! and any other spelling the shared parser resolves to a standard type.
//! `implements` / `implemented-by`, `sprint_*`, and any other spelling the
//! parser does not recognize stay `Custom`.
//!
//! Per (source, target, standard-type) triple holding at least one such
//! edge:
//!   * a native edge of that standard type to the same target already
//!     exists (a "twin"): every matching custom edge to that target is
//!     deleted;
//!   * otherwise (an "orphan"): the first matching custom edge is converted
//!     to the standard type in place (keeping its created_at / created_by)
//!     and any further matching custom edges to the same target are
//!     deleted, so the conversion never produces two edges of the same type
//!     to the same target.
//!
//! Reciprocal edges: NOT written. `rel add` only auto-writes a reciprocal
//! edge on the target for `Parent`/`Child` (`rel_should_write_inverse`);
//! every other paired kind (`Supersedes`/`SupersededBy`, `Verifies`/
//! `VerifiedBy`, `Blocks`/`BlockedBy`, `Duplicate`) needs an explicit
//! `--bidirectional`. This migration mirrors that default (non-bidirectional)
//! behavior for every kind, including `Parent`/`Child`: the input was a
//! single, one-directional legacy edge, and the job here is type repair
//! (Custom → correctly-typed), not topology repair (inventing an edge the
//! store never had). A spec that wants the canonical bidirectional
//! parent/child pair after migration gets it the normal way — `aida rel add
//! --type child` (or `-b`) — which already dedups against an edge this
//! migration just created. See `parent_child_orphan_does_not_touch_target`
//! for the fixture that pins this choice.
//!
//! The plan is computed from the live store on every run (nothing is
//! hardcoded), checked for duplicate edges before anything is written, then
//! applied one spec at a time through the backend's single-spec update path
//! (one targeted store commit per spec, never a full-store rewrite), and the
//! written specs are re-read and checked again afterwards. A second run finds
//! nothing to do.
// trace:TASK-1426 | ai:claude
// trace:TASK-1488 | ai:claude
// trace:BUG-1604 | ai:claude

use std::collections::{BTreeMap, HashMap, HashSet};

use aida_core::models::{Relationship, RelationshipType, Requirement};
use aida_core::DatabaseBackend;
use anyhow::Result;
use serde::Serialize;
use uuid::Uuid;

/// Legacy custom spellings that mean "references" and that no graph traversal
/// follows. Kept for `rel_remove_matches`'s References-family fuzzy match
/// (TASK-1426); the migration itself no longer special-cases this list — see
/// [`standard_type_for`].
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

/// The standard type a stored `Custom(name)` edge should migrate to, or
/// `None` when it isn't a migration candidate: `rel_type` isn't `Custom`, or
/// its stored spelling doesn't parse to a standard type (it stays `Custom`
/// forever — e.g. `implements`, `implemented-by`, `sprint_*`).
///
/// This is the single selection predicate for the whole migration (TASK-1488)
/// — the shared parser, so the set of migratable spellings is (with one
/// exception) exactly whatever `parse_relationship_type` resolves, never a
/// second hand-maintained list that can drift from it.
///
/// The exception: `is_legacy_related` is checked first and takes priority.
/// TASK-1426's original hardcoded family was `related` / `related-to` /
/// `relates-to`, but the shared parser's own References aliases (added later,
/// BUG-1602) are `related` / `relates-to` / `relates_to` / `relatesto` — note
/// `relates-to`, not `related-to`. Falling through to the parser alone would
/// silently *narrow* this migration's coverage, dropping `related-to` edges
/// it used to convert. Checking `is_legacy_related` first keeps this
/// migration a strict superset of TASK-1426's behavior regardless of that
/// gap in the parser's own alias table (a candidate follow-up for whoever
/// owns BUG-1602's alias list, not fixed here).
// trace:TASK-1488 | ai:claude
pub(crate) fn standard_type_for(rel_type: &RelationshipType) -> Option<RelationshipType> {
    let RelationshipType::Custom(name) = rel_type else {
        return None;
    };
    if is_legacy_related(rel_type) {
        return Some(RelationshipType::References);
    }
    match RelationshipType::parse_relationship_type(name) {
        RelationshipType::Custom(_) => None,
        standard => Some(standard),
    }
}

/// What happens to one legacy edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EdgeAction {
    /// Orphan: rewritten in place as the standard type.
    Convert,
    /// A native edge of the standard type to the same target already exists.
    DeleteTwin,
    /// A second matching custom edge to a target whose first one is converted.
    DeleteExtra,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PlannedEdge {
    pub action: EdgeAction,
    /// The stored custom spelling, e.g. `depends-on`.
    pub from_type: String,
    /// The standard type it resolves to, e.g. `blocked_by`.
    pub to_type: String,
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
    /// Legacy edge count per resolved standard type.
    pub by_standard_type: BTreeMap<String, usize>,
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

/// One legacy edge's fate: action, stored spelling, target, resolved type.
type EdgeDecision = (EdgeAction, String, Uuid, RelationshipType);

/// Compute the migration for one spec's relationship list. Returns `None`
/// when the spec holds no migratable edge.
// trace:TASK-1426 | ai:claude
// trace:TASK-1488 | ai:claude
fn plan_relationships(
    relationships: &[Relationship],
) -> Option<(Vec<Relationship>, Vec<EdgeDecision>)> {
    if !relationships
        .iter()
        .any(|r| standard_type_for(&r.rel_type).is_some())
    {
        return None;
    }
    let mut out = Vec::with_capacity(relationships.len());
    let mut actions = Vec::new();
    // (standard type, target) pairs already carrying a NATIVE edge of that
    // type — before this pass, or converted earlier in this same pass.
    let mut has_type: HashSet<(RelationshipType, Uuid)> = relationships
        .iter()
        .filter(|r| !matches!(r.rel_type, RelationshipType::Custom(_)))
        .map(|r| (r.rel_type.clone(), r.target_id))
        .collect();
    let preexisting = has_type.clone();
    for rel in relationships {
        let Some(standard) = standard_type_for(&rel.rel_type) else {
            out.push(rel.clone());
            continue;
        };
        let RelationshipType::Custom(name) = &rel.rel_type else {
            unreachable!("standard_type_for only returns Some for Custom edges")
        };
        let key = (standard.clone(), rel.target_id);
        if preexisting.contains(&key) {
            actions.push((
                EdgeAction::DeleteTwin,
                name.clone(),
                rel.target_id,
                standard,
            ));
        } else if has_type.contains(&key) {
            actions.push((
                EdgeAction::DeleteExtra,
                name.clone(),
                rel.target_id,
                standard,
            ));
        } else {
            has_type.insert(key);
            actions.push((
                EdgeAction::Convert,
                name.clone(),
                rel.target_id,
                standard.clone(),
            ));
            out.push(Relationship {
                rel_type: standard,
                ..rel.clone()
            });
        }
    }
    Some((out, actions))
}

/// Build the migration plan from the current store contents.
// trace:TASK-1426 | ai:claude
// trace:TASK-1488 | ai:claude
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
        for (action, from_type, target_id, to_type) in actions {
            plan.legacy_edges += 1;
            *plan.by_spelling.entry(from_type.clone()).or_default() += 1;
            *plan.by_standard_type.entry(to_type.name()).or_default() += 1;
            match action {
                EdgeAction::Convert => plan.converted += 1,
                EdgeAction::DeleteTwin => plan.twins_deleted += 1,
                EdgeAction::DeleteExtra => plan.extras_deleted += 1,
            }
            edges.push(PlannedEdge {
                action,
                from_type,
                to_type: to_type.name(),
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

/// The post-migration invariant for one spec: no migratable edge left, and no
/// target reached twice by the same edge kind.
// trace:TASK-1426 | ai:claude
// trace:TASK-1488 | ai:claude
pub(crate) fn check_migrated(spec_id: &str, relationships: &[Relationship]) -> Result<()> {
    if relationships
        .iter()
        .any(|r| standard_type_for(&r.rel_type).is_some())
    {
        anyhow::bail!(
            "{spec_id} still holds a custom edge that parses to a standard type after migration"
        );
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
// trace:TASK-1488 | ai:claude
pub(crate) fn render_outcome(outcome: &MigrationOutcome) -> String {
    use std::fmt::Write as _;
    let plan = &outcome.plan;
    let mut s = String::new();
    if plan.is_noop() {
        let _ = writeln!(
            s,
            "No custom edges to migrate ({} specs scanned).",
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
    let types = plan
        .by_standard_type
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        s,
        "{} custom edge(s) on {} spec(s) ({}); {} specs scanned.",
        plan.legacy_edges,
        plan.specs.len(),
        spellings,
        plan.specs_scanned
    );
    let _ = writeln!(s, "  resolved type: {types}");
    let _ = writeln!(
        s,
        "  convert to the resolved type: {}\n  delete (that type's edge already present): {}\n  delete (repeat of a converted edge): {}",
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
                EdgeAction::Convert => format!("-> {}", e.to_type),
                EdgeAction::DeleteTwin => format!("({} edge already present)", e.to_type),
                EdgeAction::DeleteExtra => "(repeat of the converted edge)".to_string(),
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

/// The canonical first-class route target for "a human is required" (gate-owned
/// with [`canonical_role_name`]).
// trace:TASK-747 | ai:claude
pub(crate) const HUMAN_ROUTE: &str = crate::gate::dispatch_deps::HUMAN_ROUTE;

/// Is this `--for <role>` value the `human` route target? Case-insensitive.
// trace:TASK-747 | ai:claude
pub(crate) fn is_human_route(raw: &str) -> bool {
    crate::gate::dispatch_deps::is_human_route(raw)
}

/// Collect the spec IDs (uppercased, stable order) of every OPEN requirement
/// explicitly routed `--for human` across all per-user queues. Work-routing is
/// a project-global axis, so we union across all queue YAML files directly
/// (mirrors [`all_queued_requirement_ids`]). Terminal (Completed/Rejected) and
/// archived specs are skipped — a routed item that already shipped is no longer
/// a human bottleneck. Returns an empty set when the queue dir is absent.
/// trace:TASK-747 | ai:claude
pub(crate) fn human_routed_spec_ids(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let dir = project_root.join(".aida-store/registry/queues");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(items) = serde_yaml::from_str::<Vec<aida_core::QueueEntry>>(&content) else {
            continue;
        };
        for item in items {
            // Route match is case-insensitive (canonical_role_name lowercases on
            // add, but legacy/hand-edited entries may carry mixed casing).
            if !item
                .for_role
                .as_deref()
                .map(is_human_route)
                .unwrap_or(false)
            {
                continue;
            }
            let Some(req) = store
                .requirements
                .iter()
                .find(|r| r.id == item.requirement_id)
            else {
                continue;
            };
            if !human_route_is_open(req.archived, &req.status) {
                continue;
            }
            let id = req
                .agreed_id
                .clone()
                .or_else(|| req.spec_id.clone())
                .unwrap_or_else(|| req.id.to_string());
            out.insert(id.to_ascii_uppercase());
        }
    }
    out
}

/// A `--for human` routed spec counts toward the human-attention view only
/// while it is OPEN — an archived or terminal (Completed/Rejected) spec that
/// once carried the route is no longer a bottleneck. Pure over its inputs so
/// the open-test is directly unit-testable. trace:TASK-747 | ai:claude
pub(crate) fn human_route_is_open(archived: bool, status: &aida_core::RequirementStatus) -> bool {
    !archived
        && !matches!(
            status,
            aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
        )
}

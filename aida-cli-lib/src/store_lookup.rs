// trace:TASK-3428 | ai:codex

// ============================================================================
// TASK-96 — plan Followups extraction. When a spec reaches Done (`aida queue
// done`) or Completed (the STORY-86 auto-bump), parse the `## Followups`
// section of any matching docs/plans/ file and offer to file each bullet as
// a child TASK so out-of-scope items don't get forgotten.
// ============================================================================

// BUG-656: BUG-655's per-parent `(parent, title)` dedup was not enough. The
// auto-followups path keys child TASKs to the *completing spec*, so the same
// plan's bullets get re-filed when a DIFFERENT spec completes — either because
// the plan's header lists several specs (each "owns" the plan and re-extracts
// it) or because the same followup text was copied into a sibling plan. On
// EPIC-0428 this re-filed ~14 cross-plan duplicate TASKs (TASK-1019-1032). The
// stable signature is the PLAN PATH, not the completing spec: a plan's followups
// must be extracted exactly once across the whole store. The marker comment
// already records `extracted from <relative plan paths>`; we parse that back out
// of every spec's comments to build the already-extracted set, then skip any
// plan already in it. A re-completion, a sibling-spec completion, or a slice PR
// landing the plan twice all become no-ops at the plan granularity.

/// Best-effort, `project_root`-pure load of the requirements store for
/// read-only lookups; `None` on any error. Gate-owned, since dispatch
/// admission validates specs against it.
// trace:STORY-98 trace:BUG-1732 trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
pub(crate) fn load_store_for_lookup(
    project_root: &std::path::Path,
) -> Option<aida_core::RequirementsStore> {
    crate::gate::dispatch_deps::load_store_for_lookup(project_root)
}

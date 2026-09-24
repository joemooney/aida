//! STORY-333: the pre-pickup gate — a centralized helper that decides whether
//! a spec is *pickable* by `aida queue work` (head pickup), `aida queue next`,
//! batch drains, and cluster drains. Replaces ad-hoc "is the spec In Progress"
//! checks scattered across the queue layer with one truth, so every pickup
//! site applies the same rules.
//!
//! Three un-pickable categories:
//!
//! - **Blocked** — the spec has a `BlockedBy` relationship to a target that
//!   has not reached `Completed`. Cleared automatically when the blocker
//!   ships. If the blocker is `Rejected`, the block is *permanent* — the
//!   target will never ship, so the dependent needs re-scoping (a UX state
//!   distinct from a normal in-flight blocker).
//! - **Human-only** — the spec carries the `human_only: bool` marker, meaning
//!   it is work no agent can do (a sign-off, a physical task, a moderated
//!   user test). Never auto-unblocks; the human marks it complete by normal
//!   `aida edit --status` when they finish.
//! - **Needs triage** — the spec's status is [`RequirementStatus::NeedsAttention`].
//!   A punt parked it mid-work; an advisor or human must resolve the fork
//!   before the spec can re-enter the pickable head. Without this gate the
//!   spec still sat at the top of `aida queue list` with a `⚠` badge — visible
//!   but rendered alongside actionable items, so a dispatcher reading the
//!   head still misfired drains on it. trace:TASK-131
//!
//! Precedence when multiple apply: `HumanOnly` > `NeedsTriage` > `BlockedBy`.
//! Human-only is the durable reason (still human-only after a punt resolves
//! or a blocker clears); needs-triage outranks BlockedBy because the punt
//! itself needs deciding before the dependency math matters.
//!
//! The helper takes a `&RequirementsStore` so it can resolve `BlockedBy`
//! target uuids back to the target spec's status + display id. A dangling
//! `BlockedBy` (target not in the store) defensively reports as blocked
//! — never as accidentally pickable — and `aida doctor verify-relationships`
//! catches the dangling edge separately.
//!
//! trace:STORY-333 | ai:claude

use crate::models::{Relationship, RelationshipType, Requirement, RequirementStatus};
use crate::RequirementsStore;

/// Why a spec is un-pickable. Kept distinct from `Pickability` itself so a
/// `match Pickability::Blocked(reason)` arm has the structured detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockedReason {
    /// Has a `BlockedBy` edge to a target whose status is not yet
    /// `Completed` (but is not `Rejected` either — that's the permanent
    /// case below). Carries the target's display id for surfacing.
    UnsatisfiedBlocker {
        target_spec: String,
        target_status: RequirementStatus,
    },
    /// Has a `BlockedBy` edge to a target whose status is `Rejected` —
    /// the blocker will never ship, so the dependent needs re-scoping.
    /// A distinct variant so the UI can shout this state rather than
    /// silently skipping it. trace:STORY-333
    PermanentlyBlocked { target_spec: String },
    /// The `human_only: bool` flag is set on the spec. Never auto-clears.
    HumanOnly,
    /// Status is [`RequirementStatus::NeedsAttention`] — paused mid-work by
    /// a punt. An advisor or human must resolve the fork before the spec
    /// re-enters the pickable head. Distinct from `HumanOnly`: this is a
    /// transient state (resumes after triage), whereas `HumanOnly` is the
    /// durable nature of the work. trace:TASK-131 | ai:claude
    NeedsTriage,
}

/// The result of a pickability check. `Pickable` is the only state in which
/// the orchestrator/queue may start a phase on the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pickability {
    Pickable,
    Blocked(BlockedReason),
}

impl Pickability {
    /// True iff the spec is pickable. Convenience for the common boolean
    /// filter check at consumer sites.
    pub fn is_pickable(&self) -> bool {
        matches!(self, Pickability::Pickable)
    }
}

/// Decide whether `req` is currently pickable by the orchestrator/queue.
///
/// Order of checks (precedence matters for the surfaced reason):
///
/// 1. `human_only` first — a human-only spec stays un-pickable even when
///    its blocker clears, so we report that as the durable reason.
/// 2. `NeedsAttention` status — a punted spec is paused awaiting triage.
///    Gated here (not just at the `aida queue next` site) so `queue list`
///    surfaces it in its Blocked section instead of inline at the head,
///    where a dispatcher misreads it as drainable. trace:TASK-131 | ai:claude
/// 3. `BlockedBy` edges — walked once; first `PermanentlyBlocked`
///    (Rejected target) wins over `UnsatisfiedBlocker` (still-in-flight
///    target) so the UI surfaces the louder failure mode.
/// 4. Otherwise pickable.
pub fn pickability(req: &Requirement, store: &RequirementsStore) -> Pickability {
    // A spike defaults to human-only at creation time, but once the advisor
    // explicitly grooms it into an agent harness the mode is authoritative.
    // `drain` runs the full research/report lifecycle; `drive` runs the same
    // work but holds the merge for advisor review. trace:TASK-1276 | ai:codex
    let groomed_spike = matches!(req.req_type, crate::RequirementType::Spike)
        && matches!(
            req.execution_mode,
            Some(crate::ExecutionMode::Drain | crate::ExecutionMode::Drive)
        );
    if req.human_only && !groomed_spike {
        return Pickability::Blocked(BlockedReason::HumanOnly);
    }

    if matches!(req.status, RequirementStatus::NeedsAttention) {
        return Pickability::Blocked(BlockedReason::NeedsTriage);
    }

    let blocked_by_edges: Vec<&Relationship> = req
        .relationships
        .iter()
        .filter(|r| matches!(r.rel_type, RelationshipType::BlockedBy))
        .collect();

    if blocked_by_edges.is_empty() {
        return Pickability::Pickable;
    }

    // First pass: any Rejected target wins — surface the loudest signal.
    for rel in &blocked_by_edges {
        match store.requirements.iter().find(|r| r.id == rel.target_id) {
            Some(target) if matches!(target.status, RequirementStatus::Rejected) => {
                return Pickability::Blocked(BlockedReason::PermanentlyBlocked {
                    target_spec: target_display_id(target),
                });
            }
            _ => {}
        }
    }

    // Second pass: any non-Completed target → still blocked. A dangling
    // edge (target not in the store) reports as a blocker too — defensive:
    // an unresolvable blocker is not accidentally pickable. The dangling
    // case is also surfaced by `aida doctor verify-relationships`.
    for rel in &blocked_by_edges {
        match store.requirements.iter().find(|r| r.id == rel.target_id) {
            Some(target) if !matches!(target.status, RequirementStatus::Completed) => {
                return Pickability::Blocked(BlockedReason::UnsatisfiedBlocker {
                    target_spec: target_display_id(target),
                    target_status: target.status.clone(),
                });
            }
            None => {
                return Pickability::Blocked(BlockedReason::UnsatisfiedBlocker {
                    target_spec: format!("(unknown:{})", rel.target_id),
                    target_status: RequirementStatus::Draft,
                });
            }
            _ => {} // Completed target — this edge is satisfied.
        }
    }

    // All BlockedBy targets are Completed → unblocked.
    Pickability::Pickable
}

/// TASK-670: is `req` blocked purely by the dependency graph — i.e. it carries
/// a `BlockedBy` edge to a target that is NOT Completed (an in-progress, draft,
/// rejected, or dangling/unknown blocker)?
///
/// This is the *work-routing* "blocked behind a blocker" axis used by the
/// `aida list --blocked` leading ⊘ glyph. Unlike [`pickability`], it
/// deliberately ignores `human_only` and `NeedsAttention` — those are already
/// surfaced on the status axis (the status glyph `⚠`), and TASK-670's overlay
/// must not duplicate status. trace:TASK-670 | ai:claude
pub fn blocked_by_incomplete(req: &Requirement, store: &RequirementsStore) -> bool {
    req.relationships
        .iter()
        .filter(|r| matches!(r.rel_type, RelationshipType::BlockedBy))
        .any(|rel| {
            match store.requirements.iter().find(|r| r.id == rel.target_id) {
                // Resolvable blocker that hasn't reached Completed → blocked.
                Some(target) => !matches!(target.status, RequirementStatus::Completed),
                // Dangling edge: a blocker we can't resolve is treated as
                // unsatisfied (defensive — don't silently un-block).
                None => true,
            }
        })
}

/// BUG-1551: an unresolved `BlockedBy` predecessor that holds a spec's
/// CLOSURE (Done → Completed), as opposed to its pickup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureBlocker {
    /// Display id of the blocker (agreed > spec > internal), or
    /// `(unknown:<uuid>)` for a dangling edge.
    pub id: String,
    /// The blocker's status; `None` for a dangling edge (target not in the
    /// store — treated as unresolved, never silently satisfied).
    pub status: Option<RequirementStatus>,
}

/// BUG-1551: is a `BlockedBy` target resolved for CLOSURE purposes? Reuses
/// the one "closed" predicate the open lens and the epic rollup read
/// ([`crate::lifecycle::status_is_closed_for_type`]): terminal (Completed,
/// Rejected, Superseded) or an accepted ADR (Decision at Approved). An EPIC is
/// judged by its read-only child rollup ([`crate::rollup::derive_epic_status`])
/// rather than its stored status, which is not hand-maintained and reads a
/// stale Draft. Deliberately wider than the pickup rule
/// (`blocked_by_incomplete`, which only accepts Completed): a Rejected blocker
/// parks new work for re-scoping, but it must not strand already-merged work at
/// Done forever.
// trace:BUG-1551 | ai:claude
pub fn closure_blocker_resolved(target: &Requirement, store: &RequirementsStore) -> bool {
    let status = effective_closure_status(target, store);
    crate::lifecycle::status_is_closed_for_type(
        &target.req_type,
        crate::lifecycle::State::from_status(&status),
    )
}

/// BUG-1551: the status a blocker is judged by — the derived rollup for an
/// epic, the stored status otherwise.
fn effective_closure_status(target: &Requirement, store: &RequirementsStore) -> RequirementStatus {
    if matches!(target.req_type, crate::RequirementType::Epic) {
        if let Some(derived) = crate::rollup::derive_epic_status(store, target.id) {
            return derived;
        }
    }
    target.status.clone()
}

/// BUG-1551: the unresolved `BlockedBy` predecessors that hold `req`'s closure.
/// `BlockedBy` gates BOTH pickup (see [`pickability`]) and closure: the
/// merge-driven Done → Completed auto-bump consults this and keeps the spec at
/// Done (with a note recording the merge) while it returns non-empty. Empty =
/// closure is free to proceed.
// trace:BUG-1551 | ai:claude
pub fn unresolved_closure_blockers(
    req: &Requirement,
    store: &RequirementsStore,
) -> Vec<ClosureBlocker> {
    req.relationships
        .iter()
        .filter(|r| matches!(r.rel_type, RelationshipType::BlockedBy))
        .filter_map(
            |rel| match store.requirements.iter().find(|r| r.id == rel.target_id) {
                Some(target) if closure_blocker_resolved(target, store) => None,
                Some(target) => Some(ClosureBlocker {
                    id: target_display_id(target),
                    status: Some(effective_closure_status(target, store)),
                }),
                None => Some(ClosureBlocker {
                    id: format!("(unknown:{})", rel.target_id),
                    status: None,
                }),
            },
        )
        .collect()
}

/// BUG-1551: one-line rendering of a closure-blocker set, e.g.
/// `STORY-12 (InProgress), BUG-3 (Draft)`.
// trace:BUG-1551 | ai:claude
pub fn closure_blockers_label(blockers: &[ClosureBlocker]) -> String {
    blockers
        .iter()
        .map(|b| match &b.status {
            Some(s) => format!("{} ({:?})", b.id, s),
            None => format!("{} (missing)", b.id),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// STORY-1430: the tag a spec carries to declare "my own closure criteria are
/// not met yet" — the merge-driven auto-bump holds it at Done while the tag is
/// present. Removing the tag is the resolution.
pub const CLOSURE_PENDING_TAG: &str = "closure:pending";

/// STORY-1430: the spec's OWN declared closure criteria that are still unmet —
/// the explicit, machine-readable counterpart to prose like "this spec does not
/// close until X". Two declared forms, no free-text parsing:
///
/// - the tag [`CLOSURE_PENDING_TAG`] (a whole-spec "not ready" flag), and
/// - unchecked `- [ ]` items under a `## Closure` heading (or a bare
///   `Closure:` line) in the description; `- [x]` items are met. The section
///   ends only at the next markdown heading; fenced code blocks are ignored.
///
/// STORY-1385: a criterion marked STRETCH never holds closure — see
/// [`unmet_stretch_closure_criteria`]. Only REQUIRED unmet items appear here.
///
/// Empty = nothing declared unmet, and the auto-bump behaves exactly as before.
/// Each entry is the human text of one unmet criterion.
// trace:STORY-1430 | ai:claude
pub fn unmet_declared_closure_criteria(req: &Requirement) -> Vec<String> {
    let mut unmet = Vec::new();
    if req
        .tags
        .iter()
        .any(|t| t.trim().eq_ignore_ascii_case(CLOSURE_PENDING_TAG))
    {
        unmet.push(format!("tag `{CLOSURE_PENDING_TAG}` is set"));
    }
    unmet.extend(
        unchecked_closure_items(&req.description)
            .into_iter()
            .filter(|(_, stretch)| !stretch)
            .map(|(text, _)| text),
    );
    unmet
}

/// STORY-1385: unchecked closure items the author marked STRETCH ahead of
/// time — "I want this, I am not sure it is reachable, I will not block on
/// it". They never hold completion; when the spec completes with any still
/// unchecked, the auto-bump records them as debt on the spec. Marking forms:
///
/// - a `(stretch)` suffix on the item: `- [ ] p95 under 1s (stretch)`, or
/// - any item under a `### Stretch` (or `Stretch criteria`) subheading nested
///   inside the Closure section, or under a `Stretch:` label line in it.
///
/// The returned text has the `(stretch)` suffix stripped.
// trace:STORY-1385 | ai:claude
pub fn unmet_stretch_closure_criteria(req: &Requirement) -> Vec<String> {
    unchecked_closure_items(&req.description)
        .into_iter()
        .filter(|(_, stretch)| *stretch)
        .map(|(text, _)| text)
        .collect()
}

/// STORY-1430 / STORY-1385: every unchecked item in the Closure section, with
/// whether it is marked stretch.
// trace:STORY-1385 | ai:claude
fn unchecked_closure_items(description: &str) -> Vec<(String, bool)> {
    let mut items = Vec::new();
    // `Some(level)` while inside the Closure section; the bare `Closure:` label
    // form opens at level 0, so any heading nested under it counts as deeper.
    let mut closure_level: Option<usize> = None;
    let mut in_stretch = false;
    let mut in_fence = false;
    for line in description.lines() {
        let trimmed = line.trim();
        // Fenced code is quoted text, never a declaration: `## Closure` or
        // `- [ ]` inside a fence is ignored.
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Only a markdown heading ends (or opens) a section; a `Label:` line
        // inside the Closure section is part of it.
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            let text = trimmed.trim_start_matches('#');
            match closure_level {
                // STORY-1385: a deeper `### Stretch` heading inside the
                // section is its stretch subsection. Any other heading ends
                // the section exactly as STORY-1430 always did.
                Some(cl) if level > cl && is_stretch_heading(text) => in_stretch = true,
                _ => {
                    closure_level = is_closure_heading(text).then_some(level);
                    in_stretch = false;
                }
            }
            continue;
        }
        let unindented = !line.starts_with(char::is_whitespace);
        let is_label = unindented && trimmed.ends_with(':') && !is_list_item(trimmed);
        if is_label {
            let label = trimmed.trim_end_matches(':');
            if closure_level.is_none() {
                if is_closure_heading(label) {
                    closure_level = Some(0);
                    in_stretch = false;
                }
                continue;
            }
            if is_stretch_heading(label) {
                in_stretch = true;
                continue;
            }
        }
        if closure_level.is_none() {
            continue;
        }
        let item = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .map(str::trim_start);
        if let Some(rest) = item.and_then(|i| i.strip_prefix("[ ]")) {
            let (text, suffix_stretch) = strip_stretch_suffix(rest.trim());
            items.push((
                if text.is_empty() {
                    "an unchecked closure item".to_string()
                } else {
                    text.to_string()
                },
                in_stretch || suffix_stretch,
            ));
        }
    }
    items
}

/// STORY-1385: `Stretch` / `Stretch criteria` (case-insensitive) names the
/// stretch subsection of a Closure section.
fn is_stretch_heading(text: &str) -> bool {
    let t = text.trim().to_ascii_lowercase();
    t == "stretch" || t == "stretch criteria"
}

/// STORY-1385: split a trailing `(stretch)` marker (case-insensitive) off an
/// item's text. Returns the remaining text and whether the marker was present.
fn strip_stretch_suffix(text: &str) -> (&str, bool) {
    const MARK: &str = "(stretch)";
    let n = text.len();
    if n >= MARK.len()
        && text.is_char_boundary(n - MARK.len())
        && text[n - MARK.len()..].eq_ignore_ascii_case(MARK)
    {
        (text[..n - MARK.len()].trim_end(), true)
    } else {
        (text, false)
    }
}

/// STORY-1430: `Closure` / `Closure criteria` (case-insensitive) opens the
/// declared-criteria section.
fn is_closure_heading(text: &str) -> bool {
    let t = text.trim().to_ascii_lowercase();
    t == "closure" || t == "closure criteria"
}

fn is_list_item(trimmed: &str) -> bool {
    trimmed.starts_with("- ") || trimmed.starts_with("* ")
}

/// Render a `BlockedReason` as a single line suitable for the
/// `aida queue list` Blocked section, `aida queue next` skip hints, and
/// the head-pickup banner. The label leads with the *reason kind*, then
/// the relevant target detail.
pub fn pickability_reason_label(reason: &BlockedReason) -> String {
    match reason {
        BlockedReason::HumanOnly => "human-only".to_string(),
        BlockedReason::NeedsTriage => "needs-triage".to_string(),
        BlockedReason::PermanentlyBlocked { target_spec } => {
            format!("blocked-by {} (REJECTED — needs re-scoping)", target_spec)
        }
        BlockedReason::UnsatisfiedBlocker {
            target_spec,
            target_status,
        } => format!("blocked-by {} ({})", target_spec, target_status),
    }
}

fn target_display_id(target: &Requirement) -> String {
    target
        .agreed_id
        .clone()
        .or_else(|| target.spec_id.clone())
        .unwrap_or_else(|| target.id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Relationship, Requirement, RequirementStatus};
    use crate::RequirementsStore;
    use uuid::Uuid;

    fn make_req(spec_id: &str, status: RequirementStatus) -> Requirement {
        let mut r = Requirement::new(format!("title for {spec_id}"), String::new());
        r.spec_id = Some(spec_id.to_string());
        r.status = status;
        r
    }

    fn store_with(reqs: Vec<Requirement>) -> RequirementsStore {
        RequirementsStore {
            requirements: reqs,
            ..Default::default()
        }
    }

    fn add_blocked_by(req: &mut Requirement, target_id: Uuid) {
        req.relationships.push(Relationship {
            rel_type: RelationshipType::BlockedBy,
            target_id,
            created_at: None,
            created_by: None,
        });
    }

    #[test]
    fn pickability_pickable_when_no_blockers_no_human_only() {
        let a = make_req("STORY-1", RequirementStatus::Approved);
        let store = store_with(vec![a.clone()]);
        assert_eq!(pickability(&a, &store), Pickability::Pickable);
    }

    #[test]
    fn pickability_blocked_by_in_progress_target_reports_unsatisfied() {
        let blocker = make_req("STORY-B", RequirementStatus::InProgress);
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dependent, blocker.id);
        let store = store_with(vec![blocker, dependent.clone()]);
        match pickability(&dependent, &store) {
            Pickability::Blocked(BlockedReason::UnsatisfiedBlocker {
                target_spec,
                target_status,
            }) => {
                assert_eq!(target_spec, "STORY-B");
                assert_eq!(target_status, RequirementStatus::InProgress);
            }
            other => panic!("expected UnsatisfiedBlocker, got {other:?}"),
        }
    }

    #[test]
    fn pickability_unblocks_when_blocker_reaches_completed() {
        let blocker = make_req("STORY-B", RequirementStatus::Completed);
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dependent, blocker.id);
        let store = store_with(vec![blocker, dependent.clone()]);
        assert_eq!(pickability(&dependent, &store), Pickability::Pickable);
    }

    /// TASK-670: `blocked_by_incomplete` is the graph-only blocked axis for the
    /// `aida list --blocked` ⊘ glyph — true for an incomplete/dangling blocker,
    /// false once every blocker is Completed, and (unlike `pickability`) it
    /// ignores human_only / NeedsAttention so it never duplicates the status
    /// axis. trace:TASK-670 | ai:claude
    #[test]
    fn blocked_by_incomplete_axis() {
        // No edges → not blocked.
        let lone = make_req("STORY-X", RequirementStatus::Approved);
        let store = store_with(vec![lone.clone()]);
        assert!(!blocked_by_incomplete(&lone, &store));

        // Incomplete blocker → blocked.
        let blocker = make_req("STORY-B", RequirementStatus::InProgress);
        let mut dep = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dep, blocker.id);
        let store = store_with(vec![blocker, dep.clone()]);
        assert!(blocked_by_incomplete(&dep, &store));

        // Blocker Completed → not blocked.
        let done = make_req("STORY-B", RequirementStatus::Completed);
        let mut dep2 = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dep2, done.id);
        let store = store_with(vec![done, dep2.clone()]);
        assert!(!blocked_by_incomplete(&dep2, &store));

        // Dangling blocker (target absent) → blocked (defensive).
        let mut dep3 = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dep3, Uuid::new_v4());
        let store = store_with(vec![dep3.clone()]);
        assert!(blocked_by_incomplete(&dep3, &store));

        // human_only / NeedsAttention with no BlockedBy edge → NOT graph-blocked
        // (those belong to the status axis, not this overlay).
        let mut na = make_req("STORY-N", RequirementStatus::NeedsAttention);
        na.human_only = true;
        let store = store_with(vec![na.clone()]);
        assert!(!blocked_by_incomplete(&na, &store));
    }

    #[test]
    fn pickability_blocked_by_rejected_target_reports_permanent() {
        let blocker = make_req("STORY-B", RequirementStatus::Rejected);
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dependent, blocker.id);
        let store = store_with(vec![blocker, dependent.clone()]);
        match pickability(&dependent, &store) {
            Pickability::Blocked(BlockedReason::PermanentlyBlocked { target_spec }) => {
                assert_eq!(target_spec, "STORY-B");
            }
            other => panic!("expected PermanentlyBlocked, got {other:?}"),
        }
    }

    #[test]
    fn pickability_human_only_reports_human_only() {
        let mut h = make_req("TASK-H", RequirementStatus::Approved);
        h.human_only = true;
        let store = store_with(vec![h.clone()]);
        assert_eq!(
            pickability(&h, &store),
            Pickability::Blocked(BlockedReason::HumanOnly)
        );
    }

    #[test]
    fn groomed_spike_mode_overrides_type_default_human_only_in_both_directions() {
        for mode in [crate::ExecutionMode::Drain, crate::ExecutionMode::Drive] {
            let mut spike = make_req("SPIKE-1", RequirementStatus::Approved);
            spike.req_type = crate::RequirementType::Spike;
            spike.human_only = true;
            spike.execution_mode = Some(mode);
            let store = store_with(vec![spike.clone()]);
            assert_eq!(pickability(&spike, &store), Pickability::Pickable);
        }

        for mode in [None, Some(crate::ExecutionMode::Operator)] {
            let mut spike = make_req("SPIKE-2", RequirementStatus::Approved);
            spike.req_type = crate::RequirementType::Spike;
            spike.human_only = true;
            spike.execution_mode = mode;
            let store = store_with(vec![spike.clone()]);
            assert_eq!(
                pickability(&spike, &store),
                Pickability::Blocked(BlockedReason::HumanOnly)
            );
        }
    }

    #[test]
    fn pickability_human_only_takes_precedence_over_blocked() {
        // Human-only + blocked → reported as human-only (the durable reason).
        let blocker = make_req("STORY-B", RequirementStatus::InProgress);
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        dependent.human_only = true;
        add_blocked_by(&mut dependent, blocker.id);
        let store = store_with(vec![blocker, dependent.clone()]);
        assert_eq!(
            pickability(&dependent, &store),
            Pickability::Blocked(BlockedReason::HumanOnly)
        );
    }

    #[test]
    fn pickability_dangling_blocked_by_target_treated_as_blocked() {
        // Defensive: an unresolvable blocker must not accidentally be pickable.
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dependent, Uuid::now_v7());
        let store = store_with(vec![dependent.clone()]);
        match pickability(&dependent, &store) {
            Pickability::Blocked(BlockedReason::UnsatisfiedBlocker { target_spec, .. }) => {
                assert!(target_spec.starts_with("(unknown:"));
            }
            other => panic!("expected UnsatisfiedBlocker (dangling), got {other:?}"),
        }
    }

    #[test]
    fn pickability_rejected_blocker_wins_over_in_progress_blocker() {
        // Two BlockedBy edges, one Rejected + one InProgress → surface the
        // louder Rejected signal.
        let b_rejected = make_req("STORY-R", RequirementStatus::Rejected);
        let b_inprog = make_req("STORY-I", RequirementStatus::InProgress);
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dependent, b_rejected.id);
        add_blocked_by(&mut dependent, b_inprog.id);
        let store = store_with(vec![b_rejected, b_inprog, dependent.clone()]);
        match pickability(&dependent, &store) {
            Pickability::Blocked(BlockedReason::PermanentlyBlocked { target_spec }) => {
                assert_eq!(target_spec, "STORY-R");
            }
            other => panic!("expected PermanentlyBlocked, got {other:?}"),
        }
    }

    #[test]
    fn pickability_label_renders_each_reason() {
        assert_eq!(
            pickability_reason_label(&BlockedReason::HumanOnly),
            "human-only"
        );
        assert_eq!(
            pickability_reason_label(&BlockedReason::NeedsTriage),
            "needs-triage"
        );
        assert_eq!(
            pickability_reason_label(&BlockedReason::PermanentlyBlocked {
                target_spec: "STORY-B".into()
            }),
            "blocked-by STORY-B (REJECTED — needs re-scoping)"
        );
        assert_eq!(
            pickability_reason_label(&BlockedReason::UnsatisfiedBlocker {
                target_spec: "STORY-B".into(),
                target_status: RequirementStatus::InProgress,
            }),
            "blocked-by STORY-B (In Progress)"
        );
    }

    // TASK-131: NeedsAttention specs must surface in the Blocked section,
    // not inline at the head of `aida queue list`. Until this gate moved
    // into pickability, a dispatcher reading the numbered head still
    // saw shelved specs at positions #1-3 and misfired drains on them.
    // trace:TASK-131 | ai:claude
    #[test]
    fn pickability_needs_attention_reports_needs_triage() {
        let r = make_req("TASK-X", RequirementStatus::NeedsAttention);
        let store = store_with(vec![r.clone()]);
        assert_eq!(
            pickability(&r, &store),
            Pickability::Blocked(BlockedReason::NeedsTriage)
        );
    }

    #[test]
    fn pickability_needs_attention_precedes_blocked_by() {
        // A NeedsAttention spec with an unsatisfied blocker still reports
        // NeedsTriage — the punt itself needs deciding before the
        // dependency math matters. trace:TASK-131
        let blocker = make_req("STORY-B", RequirementStatus::InProgress);
        let mut dependent = make_req("STORY-A", RequirementStatus::NeedsAttention);
        add_blocked_by(&mut dependent, blocker.id);
        let store = store_with(vec![blocker, dependent.clone()]);
        assert_eq!(
            pickability(&dependent, &store),
            Pickability::Blocked(BlockedReason::NeedsTriage)
        );
    }

    #[test]
    fn pickability_human_only_takes_precedence_over_needs_attention() {
        // Human-only is the durable reason — even if the spec is also
        // NeedsAttention (e.g. a human punted on a human-only spec),
        // the surfaced reason stays human-only. trace:TASK-131
        let mut r = make_req("TASK-H", RequirementStatus::NeedsAttention);
        r.human_only = true;
        let store = store_with(vec![r.clone()]);
        assert_eq!(
            pickability(&r, &store),
            Pickability::Blocked(BlockedReason::HumanOnly)
        );
    }

    // BUG-1551: BlockedBy gates CLOSURE too. A blocker holds closure until it
    // is terminal (Completed / Rejected / Superseded); anything else — or a
    // dangling edge — keeps it held. trace:BUG-1551 | ai:claude
    #[test]
    fn closure_blockers_hold_until_blocker_terminal() {
        for (status, held) in [
            (RequirementStatus::Draft, true),
            (RequirementStatus::Approved, true),
            (RequirementStatus::InProgress, true),
            (RequirementStatus::Done, true),
            (RequirementStatus::NeedsAttention, true),
            (RequirementStatus::Completed, false),
            (RequirementStatus::Rejected, false),
            (RequirementStatus::Superseded, false),
        ] {
            let blocker = make_req("STORY-B", status.clone());
            let mut spec = make_req("BUG-A", RequirementStatus::Done);
            add_blocked_by(&mut spec, blocker.id);
            let store = store_with(vec![blocker, spec.clone()]);
            let got = unresolved_closure_blockers(&spec, &store);
            assert_eq!(!got.is_empty(), held, "blocker status {status:?}");
            if held {
                assert_eq!(got[0].id, "STORY-B");
                assert_eq!(
                    closure_blockers_label(&got),
                    format!("STORY-B ({status:?})")
                );
            }
        }
    }

    #[test]
    fn closure_blockers_dangling_edge_is_unresolved_and_no_edge_is_free() {
        let mut spec = make_req("BUG-A", RequirementStatus::Done);
        let store = store_with(vec![spec.clone()]);
        assert!(unresolved_closure_blockers(&spec, &store).is_empty());
        add_blocked_by(&mut spec, Uuid::new_v4());
        let got = unresolved_closure_blockers(&spec, &store_with(vec![spec.clone()]));
        assert_eq!(got.len(), 1);
        assert!(got[0].status.is_none());
        assert!(closure_blockers_label(&got).ends_with("(missing)"));
    }

    // ── STORY-1430: declared closure criteria ───────────────────────────

    // trace:STORY-1430 | ai:claude
    #[test]
    fn no_declared_criterion_means_nothing_unmet() {
        let mut r = make_req("BUG-1", RequirementStatus::Done);
        r.description = "Acceptance:\n- [ ] prose checklist outside a Closure section\n\
                         This spec does not close until X."
            .to_string();
        assert!(unmet_declared_closure_criteria(&r).is_empty());
    }

    // trace:STORY-1430 | ai:claude
    #[test]
    fn closure_pending_tag_is_unmet() {
        let mut r = make_req("BUG-1", RequirementStatus::Done);
        r.tags.insert("Closure:Pending".to_string());
        assert_eq!(
            unmet_declared_closure_criteria(&r),
            vec!["tag `closure:pending` is set".to_string()]
        );
    }

    // trace:STORY-1430 | ai:claude
    #[test]
    fn closure_section_unchecked_items_are_unmet_checked_are_met() {
        let mut r = make_req("BUG-1480", RequirementStatus::Done);
        r.description =
            "Fix it.\n\n## Closure\n- [ ] STORY-1423 criterion 1 re-measured after the fix\n\
                         - [x] guard enabled\n  * [ ] indented second item\n\n\
                         ## Notes\n- [ ] not a closure item\nAcceptance:\n- [ ] nor this"
                .to_string();
        assert_eq!(
            unmet_declared_closure_criteria(&r),
            vec![
                "STORY-1423 criterion 1 re-measured after the fix".to_string(),
                "indented second item".to_string()
            ]
        );
        // A bare `Closure:` label opens the section too; checking every box
        // clears it.
        r.description = "Closure:\n- [x] done\n- [X] also done".to_string();
        assert!(unmet_declared_closure_criteria(&r).is_empty());
        r.description = "Closure criteria:\n- [ ] BUG-1288 headline under 26s".to_string();
        assert_eq!(unmet_declared_closure_criteria(&r).len(), 1);
    }

    // trace:STORY-1430 | ai:claude
    #[test]
    fn closure_markers_inside_code_fences_are_ignored() {
        let mut r = make_req("BUG-1", RequirementStatus::Done);
        for body in [
            "```\n## Closure\n- [ ] quoted example\n```",
            "~~~md\n# Closure\n- [ ] quoted example\n~~~",
        ] {
            r.description = body.to_string();
            assert!(unmet_declared_closure_criteria(&r).is_empty(), "{body}");
        }
        // `- [ ]` fenced inside a real Closure section is ignored too.
        r.description = "## Closure\n```\n- [ ] quoted\n```\n- [ ] real".to_string();
        assert_eq!(
            unmet_declared_closure_criteria(&r),
            vec!["real".to_string()]
        );
    }

    // trace:STORY-1430 | ai:claude
    #[test]
    fn label_line_inside_closure_section_does_not_end_it() {
        let mut r = make_req("BUG-1", RequirementStatus::Done);
        r.description = "## Closure\n- [x] done\nRemaining items:\n- [ ] still open\n\
                         ## Next\n- [ ] outside"
            .to_string();
        assert_eq!(
            unmet_declared_closure_criteria(&r),
            vec!["still open".to_string()]
        );
    }

    // ── STORY-1385: stretch closure criteria ────────────────────────────

    // trace:STORY-1385 | ai:claude
    #[test]
    fn stretch_suffix_item_does_not_hold_closure() {
        let mut r = make_req("STORY-1", RequirementStatus::Done);
        r.description = "## Closure\n- [ ] required one\n- [ ] p95 under 1s (Stretch)\n\
                         - [x] met stretch (stretch)"
            .to_string();
        assert_eq!(
            unmet_declared_closure_criteria(&r),
            vec!["required one".to_string()]
        );
        assert_eq!(
            unmet_stretch_closure_criteria(&r),
            vec!["p95 under 1s".to_string()]
        );
    }

    // trace:STORY-1385 | ai:claude
    #[test]
    fn stretch_subsection_items_do_not_hold_closure() {
        let mut r = make_req("STORY-1", RequirementStatus::Done);
        r.description = "## Closure\n- [ ] required\n### Stretch\n- [ ] ambitious\n\
                         ### Notes\n- [ ] a non-stretch heading ends the section, as before"
            .to_string();
        assert_eq!(
            unmet_declared_closure_criteria(&r),
            vec!["required".to_string()]
        );
        assert_eq!(
            unmet_stretch_closure_criteria(&r),
            vec!["ambitious".to_string()]
        );
        // A same-level `## Stretch` is NOT nested under Closure: it ends the
        // section, so its items are neither required nor stretch.
        r.description = "## Closure\n- [x] done\n## Stretch\n- [ ] outside".to_string();
        assert!(unmet_declared_closure_criteria(&r).is_empty());
        assert!(unmet_stretch_closure_criteria(&r).is_empty());
        // The bare-label forms compose too.
        r.description = "Closure:\n- [x] done\nStretch:\n- [ ] reach".to_string();
        assert!(unmet_declared_closure_criteria(&r).is_empty());
        assert_eq!(
            unmet_stretch_closure_criteria(&r),
            vec!["reach".to_string()]
        );
    }

    // trace:STORY-1385 | ai:claude
    #[test]
    fn stretch_outside_closure_section_is_ignored_and_tag_still_holds() {
        let mut r = make_req("STORY-1", RequirementStatus::Done);
        r.description = "### Stretch\n- [ ] not closure\n- [ ] also not (stretch)".to_string();
        assert!(unmet_stretch_closure_criteria(&r).is_empty());
        r.tags.insert(CLOSURE_PENDING_TAG.to_string());
        assert_eq!(unmet_declared_closure_criteria(&r).len(), 1);
    }
}

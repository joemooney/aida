use super::*;
use aida_core::RequirementStatus;

// A status-change INTO Completed renders the felt crescendo — names the
// spec + title, shows the arc terminating, points forward — and is NOT
// the generic Updated line. trace:STORY-738
#[test]
fn crescendo_has_felt_elements_not_updated() {
    let rendered =
        completion_crescendo_lines("STORY-700", "First-run experience: feel the core loop")
            .join("\n");
    // Names the spec + title.
    assert!(rendered.contains("STORY-700"), "names the spec: {rendered}");
    assert!(
        rendered.contains("First-run experience"),
        "names the title: {rendered}"
    );
    // Felt close: reaches Completed, the loop closed.
    assert!(
        rendered.contains("Completed"),
        "names Completed: {rendered}"
    );
    assert!(
        rendered.contains("the loop closed"),
        "felt close: {rendered}"
    );
    // The arc terminating.
    assert!(rendered.contains("filed"), "arc start: {rendered}");
    assert!(rendered.contains("merged"), "arc merge: {rendered}");
    // Points forward to the linked commit.
    assert!(
        rendered.contains("aida show STORY-700"),
        "forward breadcrumb: {rendered}"
    );
    assert!(
        rendered.contains("commit that landed it"),
        "points at the commit: {rendered}"
    );
    // It is emphatically NOT the flat generic line.
    assert!(
        !rendered.contains("Updated:"),
        "must not be the generic Updated: line: {rendered}"
    );
}

/// A title-only edit (no title) still renders a coherent crescendo and
/// skips the empty title line.
#[test]
fn crescendo_omits_empty_title_line() {
    let rendered = completion_crescendo_lines("TASK-1", "   ");
    // 3 lines: header, arc, breadcrumb (no title line).
    assert_eq!(rendered.len(), 3, "no blank title line: {rendered:?}");
}

// Only a genuine transition INTO Completed counts — a no-op re-set of an
// already-Completed spec does not. trace:STORY-738
#[test]
fn into_completed_transition_detection() {
    assert!(is_into_completed_transition(
        &RequirementStatus::Done,
        "Completed"
    ));
    assert!(is_into_completed_transition(
        &RequirementStatus::InProgress,
        "Completed"
    ));
    // Already Completed -> Completed is not a crescendo moment.
    assert!(!is_into_completed_transition(
        &RequirementStatus::Completed,
        "Completed"
    ));
    // A non-completion target never crescendos.
    assert!(!is_into_completed_transition(
        &RequirementStatus::InProgress,
        "Done"
    ));
}

// The `aida edit` render gate: crescendo fires ONLY on a true
// into-Completed transition on the HUMAN surface. A non-completion edit
// keeps Updated, and the agent/TOON surface is unaffected (keeps Updated).
// trace:STORY-738
#[test]
fn edit_render_gate() {
    // Human + into-Completed -> crescendo.
    assert_eq!(
        edit_completion_render(true, false),
        EditCompletionRender::Crescendo
    );
    // Non-completion human edit -> Updated.
    assert_eq!(
        edit_completion_render(false, false),
        EditCompletionRender::Updated
    );
    // Agent/TOON path unaffected even on into-Completed -> Updated.
    assert_eq!(
        edit_completion_render(true, true),
        EditCompletionRender::Updated
    );
    assert_eq!(
        edit_completion_render(false, true),
        EditCompletionRender::Updated
    );
}

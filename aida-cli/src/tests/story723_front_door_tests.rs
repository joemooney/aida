use super::*;

// STORY-723: the front-door `next` nudge must NEVER point at an archived /
// completed / done / draft corpse — only live, workable specs.
#[test]
fn next_filter_excludes_archived_and_completed() {
    // Actionable statuses (case/spelling-tolerant) pass.
    for s in [
        "approved",
        "Approved",
        "in-progress",
        "InProgress",
        "needs-attention",
    ] {
        assert!(queue_row_actionable(s, 0, 0, 0), "{s} should be actionable");
    }
    // Non-workable statuses are excluded even when otherwise live.
    for s in [
        "completed",
        "Completed",
        "done",
        "Done",
        "rejected",
        "draft",
        "released",
    ] {
        assert!(
            !queue_row_actionable(s, 0, 0, 0),
            "{s} must not be surfaced as actionable"
        );
    }
    // An archived / deferred / blocked row is excluded regardless of status.
    assert!(
        !queue_row_actionable("approved", 1, 0, 0),
        "archived excluded"
    );
    assert!(
        !queue_row_actionable("approved", 0, 1, 0),
        "deferred excluded"
    );
    assert!(
        !queue_row_actionable("in-progress", 0, 0, 1),
        "blocked excluded"
    );
}

// STORY-723: bare `aida list` defaults to the OPEN lens; any explicit status
// filter or a widened view (--all / --archived / --deferred) opts out.
#[test]
fn bare_list_defaults_to_open_lens() {
    // Bare list (no status, no widening) -> open lens on.
    assert!(list_default_open_lens(false, false, false, false));
    // An explicit status filter opts out (e.g. `aida list completed`).
    assert!(!list_default_open_lens(true, false, false, false));
    // Each widening flag opts out so closed rows surface.
    assert!(!list_default_open_lens(false, true, false, false)); // --all
    assert!(!list_default_open_lens(false, false, true, false)); // --archived
    assert!(!list_default_open_lens(false, false, false, true)); // --deferred

    // The open lens's status set is exactly the non-terminal states — it
    // must hide done/completed/rejected.
    let open: Vec<&str> = aida_core::RequirementStatus::open_statuses()
        .iter()
        .map(|s| s.cache_key())
        .collect();
    for closed in ["Completed", "Done", "Rejected"] {
        assert!(
            !open.contains(&closed),
            "open lens must not include {closed}"
        );
    }
}

// STORY-723: the agent `aida list` count denominator is labelled so it
// reconciles with `aida status` instead of reading as a bare mismatched int.
#[test]
fn count_denominator_is_labelled() {
    assert_eq!(list_count_denom_label(true), "open");
    assert_eq!(list_count_denom_label(false), "matched");
}

// STORY-723: the agent-path `aida why` headline has NO leading glyph; the
// human path keeps the decorative arrow.
#[test]
fn why_headline_has_no_leading_glyph_in_agent_mode() {
    assert_eq!(why_headline_prefix_for(true), "");
    assert!(
        !why_headline_prefix_for(false).is_empty(),
        "human path keeps the leading arrow marker"
    );
}

// STORY-729 (FIX 7): `aida why` on a Completed spec names the merged
// reality ("merged to the default branch"), and reads DIFFERENTLY from a
// Rejected spec — so a user can tell Completed from Done (which says
// "awaiting merge", covered in burndown.rs) by the words, not the colour.
#[test]
fn terminal_why_text_names_merged_for_completed_and_differs_for_rejected() {
    let (completed_reason, completed_human) =
        terminal_why_text(aida_core::RequirementStatus::Completed, "Completed");
    assert!(
        completed_reason.contains("merged"),
        "completed reason must say 'merged': {completed_reason}"
    );
    assert!(
        completed_human.contains("merged to the default branch"),
        "completed human text must name the merge: {completed_human}"
    );
    // Crucially NOT the old generic "it's done, nothing keeping it open".
    assert!(
        !completed_human.contains("it's done"),
        "completed must not read as the old generic 'it's done': {completed_human}"
    );

    let (rejected_reason, rejected_human) =
        terminal_why_text(aida_core::RequirementStatus::Rejected, "Rejected");
    assert!(
        rejected_human.contains("rejected"),
        "rejected human text must name the drop: {rejected_human}"
    );
    assert!(
        !rejected_reason.contains("merged"),
        "rejected must not claim it was merged: {rejected_reason}"
    );
    assert_ne!(
        completed_human, rejected_human,
        "Completed and Rejected must read differently"
    );
}

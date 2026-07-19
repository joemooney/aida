use super::*;

fn tagset(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn capturing_a_decision_clears_needs_design_and_keeps_other_tags() {
    // A `needs-design`-parked spec: capturing a real decision (non-blank note)
    // clears just `needs-design`; every sibling tag survives.
    let tags = tagset(&["needs-design", "batch:auth", "parent:EPIC-9"]);
    let cleared = tags_to_clear_on_note("Use JWT with 15m expiry", &tags);
    assert_eq!(cleared, vec!["needs-design".to_string()]);
}

#[test]
fn cancelled_capture_leaves_the_tag() {
    // A blank / whitespace-only note = cancel/abort → nothing is cleared, so
    // the spec stays parked with `needs-design` intact.
    let tags = tagset(&["needs-design", "batch:auth"]);
    assert!(tags_to_clear_on_note("", &tags).is_empty());
    assert!(tags_to_clear_on_note("   ", &tags).is_empty());
}

#[test]
fn no_needs_design_tag_is_an_idempotent_no_op() {
    // Capturing a decision on a spec that never carried `needs-design` clears
    // nothing — safe and idempotent.
    let tags = tagset(&["batch:auth", "parent:EPIC-9"]);
    assert!(tags_to_clear_on_note("Ship the default", &tags).is_empty());
    // And an empty tag set is likewise a no-op.
    assert!(tags_to_clear_on_note("decide", &HashSet::new()).is_empty());
}

#[test]
fn needs_design_match_is_case_insensitive() {
    let tags = tagset(&["Needs-Design", "keep"]);
    let cleared = tags_to_clear_on_note("captured", &tags);
    assert_eq!(cleared, vec!["Needs-Design".to_string()]);
}

#[test]
fn related_needs_design_signoff_tag_is_not_cleared() {
    // TASK-1086 clears only `needs-design`; the distinct `needs-design-signoff`
    // parking tag is left for its own resolution path.
    let tags = tagset(&["needs-design-signoff", "keep"]);
    assert!(tags_to_clear_on_note("captured", &tags).is_empty());
}

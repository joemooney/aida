use super::*;
use std::collections::HashSet;

fn tags(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn no_tags_yields_no_chip() {
    assert_eq!(format_tag_chip(&HashSet::new()), None);
}

#[test]
fn single_tag_chip() {
    assert_eq!(format_tag_chip(&tags(&["ux"])), Some("ux".to_string()));
}

#[test]
fn multi_tag_chip_caps_plain_tags_and_keeps_all_batch() {
    // 1 batch tag + 5 plain → batch shown, 3 plain shown, "+2".
    let chip = format_tag_chip(&tags(&[
        "batch:display-polish",
        "ux",
        "queue",
        "tags",
        "visibility",
        "display",
    ]))
    .unwrap();
    assert!(
        chip.starts_with("batch:display-polish"),
        "batch tag first: {chip}"
    );
    assert!(chip.ends_with("+2"), "overflow marker: {chip}");
    // batch + 3 plain + overflow = 5 comma-separated segments.
    assert_eq!(chip.split(", ").count(), 5, "{chip}");
}

#[test]
fn tag_chip_hoists_lifecycle_tags_after_batch_tags() {
    let chip = format_tag_chip(&tags(&[
        "ux",
        "lifecycle:no-review",
        "batch:overnight",
        "lifecycle:no-build",
    ]))
    .unwrap();
    assert_eq!(
        chip,
        "batch:overnight, lifecycle:no-build, lifecycle:no-review, ux"
    );
}

#[test]
fn tag_chip_always_shows_lifecycle_tags_before_plain_overflow() {
    let chip = format_tag_chip(&tags(&["lifecycle:trivial", "a", "b", "c", "d", "e"])).unwrap();
    assert!(
        chip.starts_with("lifecycle:trivial"),
        "lifecycle tag first: {chip}"
    );
    assert!(chip.ends_with("+2"), "plain overflow marker: {chip}");
    assert_eq!(chip.split(", ").count(), 5, "{chip}");
}

#[test]
fn batch_tag_of_finds_the_batch_tag() {
    assert_eq!(
        batch_tag_of(&tags(&["ux", "batch:plan-tooling", "queue"])),
        Some("batch:plan-tooling")
    );
    assert_eq!(batch_tag_of(&tags(&["ux", "queue"])), None);
    assert_eq!(batch_tag_of(&HashSet::new()), None);
}

#[test]
fn tag_exact_match_is_case_insensitive() {
    let t = tags(&["batch:plan-tooling", "ux"]);
    assert!(tag_matches_exact(&t, "ux"));
    assert!(tag_matches_exact(&t, "UX"));
    assert!(tag_matches_exact(&t, "batch:plan-tooling"));
    // Exact, not substring: a prefix is not an exact match.
    assert!(!tag_matches_exact(&t, "batch:plan"));
    assert!(!tag_matches_exact(&t, "queue"));
    assert!(!tag_matches_exact(&HashSet::new(), "ux"));
}

#[test]
fn tag_prefix_match_is_case_insensitive() {
    let t = tags(&["batch:plan-tooling", "ux"]);
    assert!(tag_matches_prefix(&t, "batch:"));
    assert!(tag_matches_prefix(&t, "BATCH:"));
    assert!(tag_matches_prefix(&t, "batch:plan"));
    assert!(tag_matches_prefix(&t, "u"));
    assert!(!tag_matches_prefix(&t, "integration:"));
    assert!(!tag_matches_prefix(&HashSet::new(), "batch:"));
}

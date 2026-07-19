use super::apply_tag_deltas;
use std::collections::HashSet;

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn vec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

// trace:TASK-351 | ai:claude
#[test]
fn add_inserts_new_tag_and_preserves_others() {
    let mut tags = set(&["a", "b", "c"]);
    let changed = apply_tag_deltas(&mut tags, &vec(&["x"]), &[]);
    assert!(changed);
    assert_eq!(tags, set(&["a", "b", "c", "x"]));
}

// trace:TASK-351 | ai:claude
#[test]
fn remove_drops_named_tag_and_preserves_others() {
    let mut tags = set(&["a", "b", "c"]);
    let changed = apply_tag_deltas(&mut tags, &[], &vec(&["b"]));
    assert!(changed);
    assert_eq!(tags, set(&["a", "c"]));
}

// trace:TASK-351 | ai:claude
#[test]
fn add_and_remove_compose_in_one_call() {
    let mut tags = set(&["a", "b"]);
    let changed = apply_tag_deltas(&mut tags, &vec(&["x", "y"]), &vec(&["a"]));
    assert!(changed);
    assert_eq!(tags, set(&["b", "x", "y"]));
}

// trace:TASK-351 | ai:claude
#[test]
fn adding_present_tag_is_noop() {
    let mut tags = set(&["a", "b"]);
    let changed = apply_tag_deltas(&mut tags, &vec(&["a"]), &[]);
    assert!(!changed);
    assert_eq!(tags, set(&["a", "b"]));
}

// trace:TASK-351 | ai:claude
#[test]
fn removing_absent_tag_is_noop() {
    let mut tags = set(&["a", "b"]);
    let changed = apply_tag_deltas(&mut tags, &[], &vec(&["z"]));
    assert!(!changed);
    assert_eq!(tags, set(&["a", "b"]));
}

// trace:TASK-351 | ai:claude
#[test]
fn whitespace_entries_are_ignored() {
    let mut tags = set(&["a"]);
    let changed = apply_tag_deltas(&mut tags, &vec(&["", "  "]), &vec(&[" "]));
    assert!(!changed);
    assert_eq!(tags, set(&["a"]));
}

// trace:TASK-351 | ai:claude
#[test]
fn entries_are_trimmed() {
    let mut tags = set(&["a"]);
    let changed = apply_tag_deltas(&mut tags, &vec(&["  x  "]), &vec(&[" a "]));
    assert!(changed);
    assert_eq!(tags, set(&["x"]));
}

// trace:TASK-351 | ai:claude
#[test]
fn empty_inputs_make_no_change() {
    let mut tags = set(&["a", "b"]);
    let changed = apply_tag_deltas(&mut tags, &[], &[]);
    assert!(!changed);
    assert_eq!(tags, set(&["a", "b"]));
}

// trace:TASK-351 | ai:claude
#[test]
fn add_then_remove_same_tag_in_one_call_is_net_zero_if_present() {
    // remove runs after add, so add x + remove x with x absent yields {} change net
    let mut tags = set(&["a"]);
    let changed = apply_tag_deltas(&mut tags, &vec(&["x"]), &vec(&["x"]));
    assert!(changed); // x was inserted then removed — both ops registered changes
    assert_eq!(tags, set(&["a"]));
}

// BUG-545: `--add-tag` preserves the existing set (the regression the bug
// describes: a single-tag edit must NOT clobber provenance/routing tags).
// trace:BUG-545 | ai:claude
#[test]
fn add_tag_preserves_existing_provenance_tags() {
    let mut tags = set(&["from-friction", "papercut", "safety", "aida:edit"]);
    let changed = apply_tag_deltas(&mut tags, &vec(&["supervised"]), &[]);
    assert!(changed);
    // All five present — the original four survive the add.
    assert_eq!(
        tags,
        set(&[
            "from-friction",
            "papercut",
            "safety",
            "aida:edit",
            "supervised"
        ])
    );
}

// BUG-545: `--remove-tag` drops only the named tag, leaving the rest intact.
// trace:BUG-545 | ai:claude
#[test]
fn remove_tag_drops_only_named_tag() {
    let mut tags = set(&["from-friction", "papercut", "safety", "supervised"]);
    let changed = apply_tag_deltas(&mut tags, &[], &vec(&["supervised"]));
    assert!(changed);
    assert_eq!(tags, set(&["from-friction", "papercut", "safety"]));
}

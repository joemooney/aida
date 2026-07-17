use super::tags_replace_warning;
use std::collections::HashSet;

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

// BUG-545: the loud-on-clobber warning fires when `--tags` shrinks a
// multi-tag set down to one, and shows the old→new diff (sorted).
// trace:BUG-545 | ai:claude
#[test]
fn warns_with_sorted_old_and_new_on_clobber() {
    let old = set(&["papercut", "from-friction", "safety", "aida:edit"]);
    let new = set(&["supervised"]);
    let msg = tags_replace_warning(&old, &new).expect("clobber should warn");
    assert!(
        msg.contains("was: aida:edit,from-friction,papercut,safety"),
        "old set should be listed sorted: {msg}"
    );
    assert!(
        msg.contains("now: supervised"),
        "new set should be listed: {msg}"
    );
    assert!(
        msg.contains("--add-tag/--remove-tag"),
        "warning should point at the incremental flags: {msg}"
    );
}

// BUG-545: no warning when the replace is a true no-op (same set).
// trace:BUG-545 | ai:claude
#[test]
fn no_warning_when_set_unchanged() {
    let old = set(&["a", "b"]);
    let new = set(&["b", "a"]);
    assert!(tags_replace_warning(&old, &new).is_none());
}

// BUG-545: clearing all tags renders the new set as `(none)`.
// trace:BUG-545 | ai:claude
#[test]
fn empty_new_set_renders_none() {
    let old = set(&["a"]);
    let new: HashSet<String> = HashSet::new();
    let msg = tags_replace_warning(&old, &new).expect("clearing should warn");
    assert!(
        msg.contains("now: (none)"),
        "empty new set is (none): {msg}"
    );
}

use super::*;
use std::collections::HashSet;

fn tags(v: &[&str]) -> HashSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// TASK-527: `prefix:*` matches the surface; exact still works; neither
/// over-matches a sibling.
#[test]
fn prefix_glob_and_exact_filtering() {
    let t = tags(&["aida:queue:work", "aida:status"]);
    assert!(
        tag_filter_matches("aida:queue:*", &t),
        "prefix glob matches leaf"
    );
    assert!(tag_filter_matches("aida:status", &t), "exact still matches");
    assert!(!tag_filter_matches("aida:db:*", &t), "non-matching surface");
    assert!(
        !tag_filter_matches("aida:stat", &t),
        "exact is not substring"
    );

    // `aida:queue:*` also matches an exact bare `aida:queue` tag…
    assert!(tag_filter_matches("aida:queue:*", &tags(&["aida:queue"])));
    // …but the `:*` form does not over-match a sibling surface.
    assert!(!tag_filter_matches(
        "aida:queue:*",
        &tags(&["aida:queuewatch"])
    ));
}

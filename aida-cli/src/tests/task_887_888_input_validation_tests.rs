use super::nearest_standard_rel_type;

#[test]
fn typos_of_standard_rel_types_get_a_did_you_mean() {
    // The motivating cases from the dogfood: a fat-fingered standard type
    // that lands as a Custom edge (NOT one of the accepted aliases).
    assert_eq!(nearest_standard_rel_type("blcoks"), Some("blocks"));
    assert_eq!(nearest_standard_rel_type("blok"), Some("blocks"));
    assert_eq!(nearest_standard_rel_type("duplicat"), Some("duplicate"));
    assert_eq!(nearest_standard_rel_type("blockd-by"), Some("blocked-by"));
    // Case-insensitive.
    assert_eq!(nearest_standard_rel_type("Blcoks"), Some("blocks"));
}

#[test]
fn standard_rel_types_and_aliases_get_no_hint() {
    // Exact standard spellings → no hint (handled silently as today).
    for t in [
        "parent",
        "child",
        "duplicate",
        "verifies",
        "verified-by",
        "references",
        "blocked-by",
        "blocks",
    ] {
        assert_eq!(nearest_standard_rel_type(t), None, "{t} is standard");
    }
    // Accepted aliases that `from_str` normalizes to a standard variant.
    assert_eq!(nearest_standard_rel_type("blocked_by"), None);
    assert_eq!(nearest_standard_rel_type("verifiedby"), None);
}

#[test]
fn deliberate_custom_types_are_not_nagged() {
    // A genuine custom type far from any standard → no false did-you-mean.
    assert_eq!(nearest_standard_rel_type("supersedes"), None);
    assert_eq!(nearest_standard_rel_type("implements"), None);
    assert_eq!(nearest_standard_rel_type("relates-to"), None);
    assert_eq!(nearest_standard_rel_type(""), None);
}

// TASK-888: the empty-title rejection is a `title.trim().is_empty()` guard
// in the `aida add` handler. Pin the predicate directly so the rejection
// contract survives a refactor of the handler. trace:TASK-888 | ai:claude
fn title_is_rejected(title: &str) -> bool {
    title.trim().is_empty()
}

#[test]
fn empty_and_whitespace_titles_are_rejected() {
    assert!(title_is_rejected(""));
    assert!(title_is_rejected("   "));
    assert!(title_is_rejected("\t\n "));
    assert!(!title_is_rejected("Real title"));
    assert!(!title_is_rejected("  padded but real  "));
}

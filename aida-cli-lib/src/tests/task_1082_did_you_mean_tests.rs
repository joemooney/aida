use super::{nearest_spec_id, not_found_requested_id};

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn near_miss_typo_suggests_the_real_id() {
    let known = ids(&["TASK-1", "TASK-2", "STORY-3", "BUG-9"]);
    // The motivating case: TASK-11 is a fat-fingered TASK-1.
    assert_eq!(
        nearest_spec_id("TASK-11", &known).as_deref(),
        Some("TASK-1")
    );
    // A transposed prefix (one edit inside the type token).
    assert_eq!(nearest_spec_id("TSAK-2", &known).as_deref(), Some("TASK-2"));
    // Case-insensitive match: a mixed-case transposition typo still resolves
    // to the canonical-cased id. (A pure case-only difference like `story-3`
    // is NOT a typo — `canonical_spec_id` uppercases, so it resolves on the
    // found path — and is exercised in `case_only_difference_is_not_a_typo`.)
    assert_eq!(
        nearest_spec_id("Stroy-3", &known).as_deref(),
        Some("STORY-3")
    );
}

#[test]
fn far_off_or_empty_suggests_nothing() {
    let known = ids(&["TASK-1", "STORY-3", "BUG-9"]);
    // Nothing within the edit budget → no nagging suggestion.
    assert_eq!(nearest_spec_id("EPIC-42", &known), None);
    assert_eq!(nearest_spec_id("QQQQ-9999", &known), None);
    // Empty / whitespace input never suggests.
    assert_eq!(nearest_spec_id("", &known), None);
    assert_eq!(nearest_spec_id("   ", &known), None);
    // Empty id set never suggests.
    assert_eq!(nearest_spec_id("TASK-1", &[]), None);
}

#[test]
fn exact_match_is_never_suggested_back() {
    // An exact hit would have resolved on the found path; the did-you-mean
    // lens must not echo the request back (nor divert to a NEAR neighbour
    // like TASK-2).
    let known = ids(&["TASK-1", "TASK-2"]);
    assert_eq!(nearest_spec_id("TASK-1", &known), None);
}

#[test]
fn case_only_difference_is_not_a_typo() {
    // `canonical_spec_id` uppercases, so `task-1` resolves to TASK-1 on the
    // found path — a case-only difference is not a typo and suggests nothing.
    let known = ids(&["TASK-1", "TASK-2"]);
    assert_eq!(nearest_spec_id("task-1", &known), None);
}

#[test]
fn agreed_id_typos_resolve_too() {
    // Agreed short ids (FR-1, BUG-7) are folded into the known set, so a
    // typo of one is suggested just like a spec id.
    let known = ids(&["FR-1", "FR-2", "BUG-7"]);
    assert_eq!(nearest_spec_id("FR-11", &known).as_deref(), Some("FR-1"));
}

#[test]
fn prefix_related_wins_the_distance_tie() {
    // TASK-1, TASK-10 and TASK-12 are all one edit from TASK-11, but
    // TASK-1 (a prefix of the request) is the most likely intent.
    let known = ids(&["TASK-10", "TASK-12", "TASK-1"]);
    assert_eq!(
        nearest_spec_id("TASK-11", &known).as_deref(),
        Some("TASK-1")
    );
}

#[test]
fn requested_id_parsed_from_not_found_message() {
    // The plain not-found message.
    assert_eq!(
        not_found_requested_id("Requirement not found: TASK-11\n  Hint: ...").as_deref(),
        Some("TASK-11")
    );
    // The invalid-format variant appends a parenthetical — strip it.
    assert_eq!(
        not_found_requested_id("Requirement not found: zzz (not a valid spec ID)\n  Expected ...")
            .as_deref(),
        Some("zzz")
    );
    // A legacy `.context("Requirement not found")` chain carries no id.
    assert_eq!(not_found_requested_id("Requirement not found"), None);
    // Unrelated errors are ignored.
    assert_eq!(not_found_requested_id("some other error"), None);
}

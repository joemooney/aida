use super::*;

/// `next` with no count → a single head pickup (N=1).
#[test]
fn bare_next_is_count_one() {
    assert_eq!(
        parse_next_keyword(Some("next"), None).unwrap(),
        NextKeyword::Count(1)
    );
}

/// `next1` and `next 1` both resolve to N=1 — equivalent to bare `next`.
#[test]
fn next_one_compact_and_spaced_are_count_one() {
    assert_eq!(
        parse_next_keyword(Some("next1"), None).unwrap(),
        NextKeyword::Count(1)
    );
    assert_eq!(
        parse_next_keyword(Some("next"), Some("1")).unwrap(),
        NextKeyword::Count(1)
    );
}

/// Acceptance: `next3` (compact) and `next 3` (spaced) both parse to N=3.
#[test]
fn next_three_compact_and_spaced_are_count_three() {
    assert_eq!(
        parse_next_keyword(Some("next3"), None).unwrap(),
        NextKeyword::Count(3)
    );
    assert_eq!(
        parse_next_keyword(Some("next"), Some("3")).unwrap(),
        NextKeyword::Count(3)
    );
}

/// The keyword matches case-insensitively — a real spec-id carries a
/// `TYPE-N` hyphen, so there is no collision.
#[test]
fn next_keyword_is_case_insensitive() {
    assert_eq!(
        parse_next_keyword(Some("Next3"), None).unwrap(),
        NextKeyword::Count(3)
    );
    assert_eq!(
        parse_next_keyword(Some("NEXT"), None).unwrap(),
        NextKeyword::Count(1)
    );
}

/// Acceptance: SPEC-ID positionals still parse as `NotNext` — they are
/// unambiguous because of the `TYPE-N` hyphen.
#[test]
fn spec_ids_and_no_arg_are_not_next() {
    assert_eq!(
        parse_next_keyword(Some("TASK-293"), None).unwrap(),
        NextKeyword::NotNext
    );
    assert_eq!(
        parse_next_keyword(Some("BUG-7"), None).unwrap(),
        NextKeyword::NotNext
    );
    assert_eq!(
        parse_next_keyword(None, None).unwrap(),
        NextKeyword::NotNext
    );
}

/// `next0` drains nothing — rejected with a redirect to `next` / `next1`.
#[test]
fn next_zero_is_rejected() {
    assert!(parse_next_keyword(Some("next0"), None).is_err());
    assert!(parse_next_keyword(Some("next"), Some("0")).is_err());
}

/// `nextfoo` starts with `next` but is not a valid form.
#[test]
fn next_with_non_numeric_suffix_is_rejected() {
    assert!(parse_next_keyword(Some("nextfoo"), None).is_err());
    assert!(parse_next_keyword(Some("next"), Some("foo")).is_err());
}

/// The compact `nextN` already carries its count — a separate count
/// positional alongside it is contradictory.
#[test]
fn compact_next_n_plus_separate_count_is_rejected() {
    assert!(parse_next_keyword(Some("next3"), Some("5")).is_err());
}

/// A trailing count only follows the `next` keyword — not a spec-id.
#[test]
fn count_after_a_non_next_positional_is_rejected() {
    assert!(parse_next_keyword(Some("TASK-1"), Some("3")).is_err());
}

/// `parse_next_count` accepts whitespace-padded and zero-padded counts.
#[test]
fn parse_next_count_trims_and_accepts_zero_padding() {
    assert_eq!(parse_next_count("3").unwrap(), 3);
    assert_eq!(parse_next_count(" 3 ").unwrap(), 3);
    assert_eq!(parse_next_count("03").unwrap(), 3);
    assert!(parse_next_count("0").is_err());
    assert!(parse_next_count("-1").is_err());
}

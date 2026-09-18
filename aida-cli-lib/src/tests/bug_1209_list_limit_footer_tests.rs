//! Regression coverage for explicit `aida list --limit N` human footers.
//!
//! trace:BUG-1209 | ai:codex

use super::*;

#[test]
fn truncated_open_lens_names_count_total_and_limit() {
    assert_eq!(
        list_human_count_footer(5, 10, Some(5), true),
        "5 of 10 open requirements (--limit 5; drop it or raise N to see the rest)"
    );
}

#[test]
fn truncated_explicit_filter_uses_toon_matched_label() {
    assert_eq!(
        list_human_count_footer(5, 10, Some(5), false),
        "5 of 10 matched requirements (--limit 5; drop it or raise N to see the rest)"
    );
}

#[test]
fn untrimmed_explicit_limit_preserves_historic_footer() {
    assert_eq!(
        list_human_count_footer(10, 10, Some(20), true),
        "10 requirements"
    );
    assert_eq!(
        list_human_count_footer(10, 10, Some(10), false),
        "10 requirements"
    );
}

#[test]
fn implicit_agent_cap_does_not_change_human_footer() {
    assert_eq!(
        list_human_count_footer(50, 100, None, true),
        "50 requirements"
    );
}

#[test]
fn zero_limit_discloses_all_hidden_matches() {
    assert_eq!(
        list_human_count_footer(0, 13, Some(0), true),
        "0 of 13 open requirements (--limit 0; drop it or raise N to see the rest)"
    );
    assert_eq!(
        list_human_count_footer(0, 13, Some(0), false),
        "0 of 13 matched requirements (--limit 0; drop it or raise N to see the rest)"
    );
}

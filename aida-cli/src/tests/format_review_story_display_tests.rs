use super::*;

/// TASK-91: a synthetic `Review PR-15: ...` title produces a
/// PR-prominent display tuple, with the STORY-NNN as parenthetical.
#[test]
fn rewrites_synthetic_review_story_title() {
    let result = format_review_story_display("STORY-108", "Review PR-15: EPIC-23 batch wrap-up");
    assert_eq!(
        result,
        Some((
            "PR-15 (STORY-108)".to_string(),
            "EPIC-23 batch wrap-up".to_string()
        ))
    );
}

/// agreed_id wins over spec_id at the call site; this helper is
/// id-agnostic — whatever display id the caller passes ends up in
/// the parenthetical.
#[test]
fn passes_through_agreed_id_when_present() {
    let result = format_review_story_display("STORY-7", "Review PR-2: tiny fix");
    assert_eq!(
        result,
        Some(("PR-2 (STORY-7)".to_string(), "tiny fix".to_string()))
    );
}

/// Non-review titles return None so the caller falls back to the
/// untransformed (display_id, title) pair.
#[test]
fn returns_none_for_non_review_title() {
    assert_eq!(
        format_review_story_display("STORY-50", "Add OAuth provider"),
        None
    );
}

/// BUG-525: a non-ASCII title with a multibyte char straddling the
/// `Review PR-` prefix boundary (byte 10) must return None, not panic on a
/// byte-slice that lands mid-char.
#[test]
fn non_ascii_title_near_prefix_boundary_does_not_panic() {
    // '—' (em-dash) occupies bytes 9..12; a raw `title[..10]` slice panics.
    let result = format_review_story_display("TASK-788", "TASK-782 — intent markers + policy");
    assert_eq!(result, None);
    // A leading multibyte char, and an exactly-9-byte (sub-prefix) title.
    assert_eq!(format_review_story_display("X", "→ go"), None);
    assert_eq!(format_review_story_display("X", "Review P"), None);
}

/// Defensive: `Review PR-` followed by non-digits isn't the
/// auto-queue pattern; don't claim it.
#[test]
fn returns_none_when_pr_number_not_digits() {
    assert_eq!(
        format_review_story_display("STORY-99", "Review PR-abc: malformed title"),
        None
    );
}

/// A title that mentions PR-N but doesn't use the auto-queue
/// prefix shouldn't be rewritten.
#[test]
fn returns_none_for_partial_match() {
    assert_eq!(
        format_review_story_display("STORY-99", "Reviewer should look at PR-7 carefully"),
        None
    );
}

/// Edge case: empty after-colon (theoretical — auto-queue always
/// includes a title). Should still produce a tuple, just with an
/// empty trimmed-title.
#[test]
fn handles_empty_title_after_colon() {
    let result = format_review_story_display("STORY-1", "Review PR-99:");
    assert_eq!(result, Some(("PR-99 (STORY-1)".to_string(), String::new())));
}

/// BUG-91: the `Review PR-` prefix matches case-insensitively (aligning
/// with the case-insensitive `review_title_matches` router), and the
/// after-colon title keeps its original case.
#[test]
fn matches_review_prefix_case_insensitively() {
    let result = format_review_story_display("STORY-15", "review pr-15: Foo Bar");
    assert_eq!(
        result,
        Some(("PR-15 (STORY-15)".to_string(), "Foo Bar".to_string()))
    );
    let result = format_review_story_display("STORY-3", "REVIEW PR-3: keep CASE");
    assert_eq!(
        result,
        Some(("PR-3 (STORY-3)".to_string(), "keep CASE".to_string()))
    );
}

use super::*;

#[test]
fn squash_merge_subject_yields_pr_number() {
    assert_eq!(
        parse_squash_pr_number("EPIC-24 batch:plan-tooling — plan lifecycle (7 specs) (#29)"),
        Some(29)
    );
    assert_eq!(parse_squash_pr_number("feat: thing (#1234)"), Some(1234));
    // Trailing whitespace is tolerated.
    assert_eq!(parse_squash_pr_number("fix: y (#7)  "), Some(7));
}

#[test]
fn aida_format_subject_has_no_pr_number() {
    // The `(SPEC-ID)` AIDA commit format must NOT be mistaken for a
    // PR pointer — only `(#NN)` counts.
    assert_eq!(
        parse_squash_pr_number("[AI:claude] feat(cli): aida rebase (TASK-103)"),
        None
    );
    assert_eq!(parse_squash_pr_number("chore: bump deps"), None);
}

#[test]
fn malformed_pr_suffix_is_rejected() {
    assert_eq!(parse_squash_pr_number("feat: x (#notanumber)"), None);
    assert_eq!(parse_squash_pr_number("feat: x (#12"), None);
    assert_eq!(parse_squash_pr_number(""), None);
}

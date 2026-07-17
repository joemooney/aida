use super::*;

/// BUG-88: a well-formed `gh pr list -q` line parses into a Found PR.
// trace:BUG-88 | ai:claude
#[test]
fn parses_single_well_formed_line() {
    let stdout = "42\tFix the thing\thttps://github.com/o/r/pull/42\n";
    match parse_gh_pr_line(stdout) {
        PrLookup::Found(p) => {
            assert_eq!(p.number, 42);
            assert_eq!(p.title, "Fix the thing");
            assert_eq!(p.url, "https://github.com/o/r/pull/42");
        }
        other => panic!("expected Found, got {:?}", std::mem::discriminant(&other)),
    }
}

/// BUG-88: empty stdout means no PR matched the query — distinct from
// gh failing. trace:BUG-88 | ai:claude
#[test]
fn empty_stdout_is_no_open_pr() {
    assert!(matches!(parse_gh_pr_line(""), PrLookup::NoOpenPr));
    assert!(matches!(parse_gh_pr_line("\n"), PrLookup::NoOpenPr));
    assert!(matches!(parse_gh_pr_line("   \n"), PrLookup::NoOpenPr));
}

/// BUG-88: malformed lines (too few fields, non-numeric PR number)
/// surface as GhFailed so the caller doesn't silently swallow them.
// trace:BUG-88 | ai:claude
#[test]
fn malformed_line_is_gh_failed() {
    // Only one field.
    assert!(matches!(
        parse_gh_pr_line("just-one-field\n"),
        PrLookup::GhFailed(_)
    ));
    // Two fields (missing url).
    assert!(matches!(
        parse_gh_pr_line("42\ttitle\n"),
        PrLookup::GhFailed(_)
    ));
    // Non-numeric PR number.
    assert!(matches!(
        parse_gh_pr_line("abc\ttitle\turl\n"),
        PrLookup::GhFailed(_)
    ));
}

/// BUG-88: only the first line is considered (we pass --limit 1 to gh
/// but defensively the parser shouldn't blow up if more arrive).
// trace:BUG-88 | ai:claude
#[test]
fn only_first_line_consumed() {
    let stdout = "11\tfirst\thttps://x/1\n22\tsecond\thttps://x/2\n";
    match parse_gh_pr_line(stdout) {
        PrLookup::Found(p) => assert_eq!(p.number, 11),
        other => panic!("expected Found, got {:?}", std::mem::discriminant(&other)),
    }
}

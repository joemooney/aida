use super::*;
use std::collections::HashMap;

// ── Detector (1): aida pull no-op with a pending review ──────────────
#[test]
fn pull_noop_warns_loudly_when_reviewer_lease_on_unmerged_pr() {
    let msg =
        pull_noop_warning(true, Some("PR-27")).expect("a no-op with a pending review should warn");
    assert!(msg.contains("PR-27"), "warning should name the PR: {msg}");
    assert!(
        msg.to_lowercase().contains("wait"),
        "warning should suggest waiting: {msg}"
    );
}

#[test]
fn pull_noop_quiet_note_when_no_pending_review() {
    let msg = pull_noop_warning(true, None).expect("a no-op still gets a one-liner");
    assert!(
        msg.to_lowercase().contains("up to date"),
        "plain note should say up to date: {msg}"
    );
    // No PR alarm when there's no reviewer lease.
    assert!(!msg.contains("PR-"));
}

#[test]
fn pull_that_advanced_never_warns() {
    assert!(pull_noop_warning(false, Some("PR-27")).is_none());
    assert!(pull_noop_warning(false, None).is_none());
}

// ── Detector (2): git pull no-op (local main == origin/main) ─────────
#[test]
fn local_main_at_origin_is_true_only_when_shas_match() {
    assert!(local_main_already_at_origin(Some("abc123"), Some("abc123")));
    assert!(!local_main_already_at_origin(
        Some("abc123"),
        Some("def456")
    ));
}

#[test]
fn local_main_at_origin_is_false_when_a_sha_is_unknown_or_empty() {
    assert!(!local_main_already_at_origin(None, Some("abc123")));
    assert!(!local_main_already_at_origin(Some("abc123"), None));
    assert!(!local_main_already_at_origin(None, None));
    assert!(!local_main_already_at_origin(Some(""), Some("")));
}

// ── Detector (3): release with unmerged PRs ──────────────────────────
#[test]
fn release_warns_for_a_single_unmerged_pr() {
    let msg = release_unmerged_pr_warning(&[27]).expect("one open PR should warn");
    assert!(msg.contains("PR-27"), "should name the PR: {msg}");
    assert!(msg.contains("1 open PR "), "singular phrasing: {msg}");
    assert!(msg.contains(" is "), "singular verb: {msg}");
}

#[test]
fn release_warns_for_multiple_unmerged_prs_sorted_and_deduped() {
    let msg = release_unmerged_pr_warning(&[31, 27, 27, 5]).expect("open PRs should warn");
    // sorted ascending + deduped
    assert!(msg.contains("PR-5, PR-27, PR-31"), "sorted+deduped: {msg}");
    assert!(msg.contains("3 open PRs"), "plural count: {msg}");
    assert!(msg.contains(" are "), "plural verb: {msg}");
}

#[test]
fn release_does_not_warn_with_no_open_prs() {
    assert!(release_unmerged_pr_warning(&[]).is_none());
}

// ── Detector (4): session end with cross-role queue waiting ──────────
fn counts(pairs: &[(&str, usize)]) -> HashMap<String, usize> {
    pairs.iter().map(|(r, c)| (r.to_string(), *c)).collect()
}

#[test]
fn cross_role_lists_other_roles_with_waiting_work() {
    let c = counts(&[("reviewer", 3), ("implementer", 2), ("advisor", 0)]);
    let waiting = cross_role_queue_waiting(Some("reviewer"), &c);
    // reviewer (the ending role) and advisor (zero) excluded; implementer left.
    assert_eq!(waiting, vec![("implementer".to_string(), 2)]);
}

#[test]
fn cross_role_excludes_only_the_ending_role() {
    let c = counts(&[("reviewer", 1), ("implementer", 4)]);
    let waiting = cross_role_queue_waiting(Some("implementer"), &c);
    assert_eq!(waiting, vec![("reviewer".to_string(), 1)]);
}

#[test]
fn cross_role_is_sorted_by_role_name() {
    let c = counts(&[("reviewer", 1), ("advisor", 2)]);
    let waiting = cross_role_queue_waiting(Some("implementer"), &c);
    assert_eq!(
        waiting,
        vec![("advisor".to_string(), 2), ("reviewer".to_string(), 1)]
    );
}

#[test]
fn cross_role_with_unknown_ending_role_includes_all_waiting() {
    let c = counts(&[("reviewer", 1), ("implementer", 2)]);
    let waiting = cross_role_queue_waiting(None, &c);
    assert_eq!(
        waiting,
        vec![("implementer".to_string(), 2), ("reviewer".to_string(), 1)]
    );
}

#[test]
fn session_end_warning_renders_singular_and_plural() {
    let one = session_end_cross_role_warning(&[("reviewer".to_string(), 1)]).unwrap();
    assert!(one.contains("reviewer has 1 item"), "singular: {one}");
    assert!(!one.contains("1 items"));
    let many = session_end_cross_role_warning(&[("implementer".to_string(), 3)]).unwrap();
    assert!(many.contains("implementer has 3 items"), "plural: {many}");
}

#[test]
fn session_end_warning_is_none_when_nothing_waiting() {
    assert!(session_end_cross_role_warning(&[]).is_none());
    let empty = cross_role_queue_waiting(Some("reviewer"), &counts(&[("reviewer", 5)]));
    assert!(session_end_cross_role_warning(&empty).is_none());
}

// ── PR-scope parsing for the reviewer-lease query ────────────────────
#[test]
fn pr_number_from_scope_parses_pr_and_mr_forms() {
    assert_eq!(pr_number_from_scope("PR-27"), Some(27));
    assert_eq!(pr_number_from_scope("pr-3"), Some(3));
    assert_eq!(pr_number_from_scope("MR-12"), Some(12));
    assert_eq!(pr_number_from_scope(" PR-99 "), Some(99));
    assert_eq!(pr_number_from_scope("STORY-127"), None);
    assert_eq!(pr_number_from_scope("PR-abc"), None);
    assert_eq!(pr_number_from_scope(""), None);
}

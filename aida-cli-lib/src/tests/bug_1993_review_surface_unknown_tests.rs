//! BUG-1993: a linkage scan that timed out leaves the review surface
//! `Unknown`, never `Local` ("never pushed"), so `aida human` keeps the spec
//! listed and `aida review` does not misreport it.
// trace:BUG-1993 | ai:claude
use super::*;

fn timed_out_linkage(commits_scan_incomplete: bool, shipped_unknown: bool) -> GitLinkage {
    GitLinkage {
        commits: Vec::new(),
        files: Vec::new(),
        shipped: false,
        branch: None,
        other_branches: Vec::new(),
        branch_scan_truncated: None,
        files_scan_incomplete: false,
        commits_scan_incomplete,
        shipped_unknown,
        worktree: None,
        shipped_pr: None,
        repo: None,
    }
}

#[test]
fn commit_scan_timeout_is_unknown_not_local() {
    let l = timed_out_linkage(true, true);
    match classify_review_surface(&l, None) {
        ReviewSurface::Unknown { reason } => {
            assert_eq!(reason, LINKAGE_UNKNOWN_REASON_COMMIT_SCAN)
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn merge_check_timeout_is_unknown_not_local() {
    let mut l = timed_out_linkage(false, true);
    l.commits = vec![("sha".into(), "s".into(), "fix (BUG-9)".into())];
    match classify_review_surface(&l, None) {
        ReviewSurface::Unknown { reason } => {
            assert_eq!(reason, LINKAGE_UNKNOWN_REASON_MERGE_CHECK)
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn truncated_branch_scan_without_a_branch_is_unknown() {
    let mut l = timed_out_linkage(false, false);
    l.commits = vec![("sha".into(), "s".into(), "fix (BUG-9)".into())];
    l.branch_scan_truncated = Some((5, 40));
    assert!(matches!(
        classify_review_surface(&l, None),
        ReviewSurface::Unknown {
            reason: LINKAGE_UNKNOWN_REASON_BRANCH_SCAN
        }
    ));
    // A branch found despite the truncated walk is still a real surface.
    l.branch = Some("bug-9".into());
    assert!(matches!(
        classify_review_surface(&l, None),
        ReviewSurface::BranchNoChange { .. }
    ));
}

#[test]
fn complete_linkage_with_no_branch_is_still_local() {
    let l = timed_out_linkage(false, false);
    assert!(matches!(
        classify_review_surface(&l, None),
        ReviewSurface::Local
    ));
}

#[test]
fn forge_found_change_wins_over_a_timed_out_scan() {
    let l = timed_out_linkage(true, true);
    let found = crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
        id: 77,
        url: "https://example/pr/77".to_string(),
        branch: "bug-9".to_string(),
        base: "main".to_string(),
        title: None,
    });
    match classify_review_surface_forge_first(&l, Some(found), None) {
        ReviewSurface::OpenChange { number, branch, .. } => {
            assert_eq!(number, 77);
            assert_eq!(branch, "bug-9");
        }
        other => panic!("expected OpenChange, got {other:?}"),
    }
}

#[test]
fn unknown_surface_stays_on_the_human_review_seat() {
    let surface = ReviewSurface::Unknown {
        reason: LINKAGE_UNKNOWN_REASON_COMMIT_SCAN,
    };
    assert_eq!(review_surface_branch(&surface), None);
    for status in [
        aida_core::RequirementStatus::InProgress,
        aida_core::RequirementStatus::Done,
    ] {
        assert_eq!(
            classify_human_review_bucket(status, false, false, &surface),
            HumanReviewBucket::ReviewsAwaiting,
            "an unknown surface is listed, not dropped as never-pushed"
        );
    }
    assert_eq!(
        classify_human_review_bucket(aida_core::RequirementStatus::Draft, false, false, &surface),
        HumanReviewBucket::WipBranch
    );
    // The status/view-state gates still win.
    assert_eq!(
        classify_human_review_bucket(
            aida_core::RequirementStatus::Completed,
            false,
            false,
            &surface
        ),
        HumanReviewBucket::Excluded
    );
    assert_eq!(
        classify_human_review_bucket(aida_core::RequirementStatus::Done, true, false, &surface),
        HumanReviewBucket::Excluded
    );
    assert_eq!(REVIEW_SURFACE_UNKNOWN_LABEL, "unknown (scan timed out)");
}

#[test]
fn unknown_reason_is_none_once_shipped_or_complete() {
    let mut l = timed_out_linkage(false, false);
    assert_eq!(l.unknown_reason(), None);
    l.shipped = true;
    l.branch_scan_truncated = Some((1, 2));
    assert_eq!(l.unknown_reason(), None);
}

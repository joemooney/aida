use super::*;

fn approved_decision(login: Option<&str>) -> PrReviewDecision {
    PrReviewDecision {
        decision: "APPROVED".to_string(),
        approver: login.map(|s| s.to_string()),
    }
}

/// PR open, CI still running, no review started → CI-running stage,
/// so the row says "wait for CI" instead of a premature "start
/// review" (the orchestrator's CI-wait window).
#[test]
fn open_pr_ci_running() {
    let d = classify_open_pr_display(
        None,
        LocalReviewState::NotStarted,
        &CiProbe::InProgress { pr_number: 71 },
    );
    assert_eq!(d, InFlightPrDisplay::CiRunning);
}

/// PR open, CI finished (green / no-checks / no-signal), no review
/// started → ready for a reviewer to pick up.
#[test]
fn open_pr_ci_done_awaits_review() {
    for ci in [
        CiProbe::Green { pr_number: 71 },
        CiProbe::PrNoChecks { pr_number: 71 },
        CiProbe::NoSignal(String::new()),
    ] {
        assert_eq!(
            classify_open_pr_display(None, LocalReviewState::NotStarted, &ci),
            InFlightPrDisplay::AwaitingReview,
        );
    }
}

/// PR open, a reviewer session lease owns it → under-review stage.
/// A still-running CI probe does not override an active lease.
#[test]
fn open_pr_under_review_by_lease() {
    let d = classify_open_pr_display(
        None,
        LocalReviewState::InProgressLease {
            session_id: "abc12345".to_string(),
            since: "30m ago".to_string(),
        },
        &CiProbe::InProgress { pr_number: 71 },
    );
    assert_eq!(
        d,
        InFlightPrDisplay::UnderReviewLease {
            session_id: "abc12345".to_string(),
            since: "30m ago".to_string(),
        }
    );
}

/// PR open, an In-Progress `Review PR-N:` story (no PR-scoped
/// session) → under-review stage.
#[test]
fn open_pr_under_review_by_story() {
    let d = classify_open_pr_display(
        None,
        LocalReviewState::InProgressStory {
            story_id: "STORY-9".to_string(),
        },
        &CiProbe::NoSignal(String::new()),
    );
    assert_eq!(
        d,
        InFlightPrDisplay::UnderReviewStory {
            story_id: "STORY-9".to_string()
        }
    );
}

/// PR open, review complete (gh `reviewDecision` APPROVED) →
/// review-complete stage, carrying the approver login.
#[test]
fn open_pr_review_approved() {
    let d = classify_open_pr_display(
        Some(approved_decision(Some("octocat"))),
        LocalReviewState::NotStarted,
        &CiProbe::NoSignal(String::new()),
    );
    assert_eq!(
        d,
        InFlightPrDisplay::ReviewApproved {
            approver: Some("octocat".to_string())
        }
    );
}

/// An APPROVED decision is authoritative — it supersedes a reviewer
/// lease still on disk (a lingering session after approval).
#[test]
fn approved_supersedes_lingering_lease() {
    let d = classify_open_pr_display(
        Some(approved_decision(None)),
        LocalReviewState::InProgressLease {
            session_id: "abc12345".to_string(),
            since: "1h ago".to_string(),
        },
        &CiProbe::NoSignal(String::new()),
    );
    assert_eq!(d, InFlightPrDisplay::ReviewApproved { approver: None });
}

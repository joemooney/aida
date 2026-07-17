use super::*;

fn write_lease(project_root: &std::path::Path, id: &str, scope: &str) {
    let dir = project_root.join(".aida").join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let lease = SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: project_root.to_path_buf(),
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("reviewer".into()),
        creator_pid: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
    };
    std::fs::write(
        dir.join(format!("{}.toml", id)),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();
}

fn review_story(pr: u64, status: RequirementStatus) -> aida_core::Requirement {
    let mut r = aida_core::Requirement::new(format!("Review PR-{}: some batch", pr), String::new());
    r.spec_id = Some(format!("STORY-{}", 900 + pr));
    r.status = status;
    r
}

/// State 1: no lease, no In-Progress review story → NotStarted.
#[test]
fn local_state_not_started_with_no_signals() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(
        detect_local_review_state(tmp.path(), &[], 42),
        LocalReviewState::NotStarted
    );
}

/// State 1: a `Review PR-42:` story still at Approved (queued, not
/// picked up) does NOT count as in-progress — the status the
/// auto-queue files it at is "ready to work", not a verdict.
#[test]
fn local_state_approved_review_story_is_not_in_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let reqs = [review_story(42, RequirementStatus::Approved)];
    assert_eq!(
        detect_local_review_state(tmp.path(), &reqs, 42),
        LocalReviewState::NotStarted
    );
}

/// State 2: an active session lease scoped to PR-42 → InProgressLease.
#[test]
fn local_state_in_progress_via_lease() {
    let tmp = tempfile::tempdir().unwrap();
    write_lease(tmp.path(), "abc123def456", "PR-42");
    match detect_local_review_state(tmp.path(), &[], 42) {
        LocalReviewState::InProgressLease { session_id, .. } => {
            assert_eq!(session_id, "abc123def456");
        }
        other => panic!("expected InProgressLease, got {:?}", other),
    }
}

/// State 2: a lease scoped to a DIFFERENT PR is ignored.
#[test]
fn local_state_lease_for_other_pr_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    write_lease(tmp.path(), "abc123def456", "PR-99");
    assert_eq!(
        detect_local_review_state(tmp.path(), &[], 42),
        LocalReviewState::NotStarted
    );
}

/// State 2: an In-Progress `Review PR-42:` story (no scoped session)
/// → InProgressStory carrying the story id.
#[test]
fn local_state_in_progress_via_review_story() {
    let tmp = tempfile::tempdir().unwrap();
    let reqs = [review_story(42, RequirementStatus::InProgress)];
    match detect_local_review_state(tmp.path(), &reqs, 42) {
        LocalReviewState::InProgressStory { story_id } => {
            assert_eq!(story_id, "STORY-942");
        }
        other => panic!("expected InProgressStory, got {:?}", other),
    }
}

/// State 2: the lease leg wins over the review-story leg when both
/// fire — a live scoped session is the stronger signal.
#[test]
fn local_state_lease_takes_precedence_over_story() {
    let tmp = tempfile::tempdir().unwrap();
    write_lease(tmp.path(), "abc123def456", "PR-42");
    let reqs = [review_story(42, RequirementStatus::InProgress)];
    assert!(matches!(
        detect_local_review_state(tmp.path(), &reqs, 42),
        LocalReviewState::InProgressLease { .. }
    ));
}

/// State 3: gh `reviewDecision` APPROVED with a latestReviews entry
/// → decision + approver login parsed.
#[test]
fn parse_review_decision_approved_with_reviewer() {
    let json = r#"{
            "reviewDecision": "APPROVED",
            "latestReviews": [
                {"author": {"login": "alice"}, "state": "APPROVED"}
            ]
        }"#;
    let d = parse_review_decision_json(json).expect("parses");
    assert_eq!(d.decision, "APPROVED");
    assert_eq!(d.approver.as_deref(), Some("alice"));
}

/// State 1/edge: a PR with no reviews → empty decision, no approver.
#[test]
fn parse_review_decision_no_reviews() {
    let json = r#"{"reviewDecision": "", "latestReviews": []}"#;
    let d = parse_review_decision_json(json).expect("parses");
    assert_eq!(d.decision, "");
    assert!(d.approver.is_none());
}

/// CHANGES_REQUESTED parses but is not APPROVED — the render path
/// treats it as "not State 3" and falls back to the local state.
#[test]
fn parse_review_decision_changes_requested() {
    let json = r#"{
            "reviewDecision": "CHANGES_REQUESTED",
            "latestReviews": [
                {"author": {"login": "bob"}, "state": "CHANGES_REQUESTED"}
            ]
        }"#;
    let d = parse_review_decision_json(json).expect("parses");
    assert_eq!(d.decision, "CHANGES_REQUESTED");
    assert!(d.approver.is_none(), "no APPROVED review → no approver");
}

/// State 4: a merged PR with a `mergedAt` timestamp → number + a
/// humanized merge time.
#[test]
fn parse_merged_pr_with_timestamp() {
    let json = r#"[{"number": 43, "mergedAt": "2026-05-15T10:00:00Z"}]"#;
    let (number, merged) = parse_merged_pr_json(json).expect("parses");
    assert_eq!(number, 43);
    assert!(merged.is_some(), "mergedAt → humanized time");
}

/// State 4: a merged PR row missing `mergedAt` still yields the
/// number — the hint just drops the time.
#[test]
fn parse_merged_pr_without_timestamp() {
    let json = r#"[{"number": 43}]"#;
    let (number, merged) = parse_merged_pr_json(json).expect("parses");
    assert_eq!(number, 43);
    assert!(merged.is_none());
}

/// No merged PR (empty gh array) → None, so the render path keeps
/// the "no PR opened yet" hint.
#[test]
fn parse_merged_pr_empty_is_none() {
    assert!(parse_merged_pr_json("[]").is_none());
}

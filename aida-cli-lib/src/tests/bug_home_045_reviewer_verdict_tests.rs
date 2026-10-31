//! BUG-HOME-045: a standalone reviewer that recorded no verdict must not end
//! under another review's PASS, and its brief must keep `aida review record`
//! in its own review worktree.
//!
//!   * a failed collection shows no verdict body, even when the PR's verdict
//!     file still holds an earlier round's approval, so the summary carries
//!     no PASS / approve line;
//!   * a reviewer that recorded no verdict gets no "manual continuation" hint
//!     (it is a result, not a setup to finish by hand);
//!   * the standalone reviewer's verdict anchor names its review worktree as
//!     the directory for `aida review record`, never the primary checkout.
//!
//! The contained-launch refusal is covered beside the keyring tests
//! (`gate/tests/bug_1934_keyring_tests.rs`).
//!
//! trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
use super::*;

const PRIOR_APPROVAL: &str = r#"{"verdict":"Approved","summary":"Base-only update from main; prior approval carried","reviewed_sha":"5a24edaf00000000000000000000000000000000","recorded_by":"reviewer-other"}"#;

fn prior_round_verdict(root: &std::path::Path, pr: u64) -> std::path::PathBuf {
    let path = root.join(format!(".aida/review-verdicts/PR-{pr}.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, PRIOR_APPROVAL).unwrap();
    path
}

#[test]
fn no_collected_verdict_prints_no_pass_or_approve_line() {
    let root = tempfile::tempdir().unwrap();
    let path = prior_round_verdict(root.path(), 3142);

    // The fixture is live: a collected run would show that approval.
    let shown = standalone_summary_verdict_body(true, &path);
    let collected = crate::reviewer_summary::format_reviewer_summary(
        3142,
        shown.as_deref(),
        None,
        &path,
        None,
        Some(0),
    );
    assert!(collected.contains("PASS"), "{collected}");

    let body = standalone_summary_verdict_body(false, &path);
    assert_eq!(body, None, "a failed collection reads no earlier round");
    let summary = crate::reviewer_summary::format_reviewer_summary(
        3142,
        body.as_deref(),
        None,
        &path,
        None,
        Some(0),
    );
    for line in summary.lines() {
        let lower = line.to_ascii_lowercase();
        assert!(
            !line.contains("PASS") && !lower.contains("approve"),
            "no-verdict summary carries a verdict line: {line:?}\n{summary}"
        );
    }
    assert!(!summary.contains("prior approval carried"), "{summary}");
}

#[test]
fn reviewer_without_a_verdict_gets_no_manual_continuation_hint() {
    let no_verdict = anyhow::Error::new(NoReviewVerdict {
        message: "the reviewer for PR-3142 exited without recording a verifiable verdict".into(),
    });
    assert!(!pickup_failure_offers_manual_continuation(&no_verdict));
    let vendor_exit = anyhow::Error::new(ReviewerVendorExit {
        code: 2,
        message: "no verdict".into(),
    });
    assert!(!pickup_failure_offers_manual_continuation(&vendor_exit));
    let setup = anyhow::anyhow!("worktree setup failed");
    assert!(pickup_failure_offers_manual_continuation(&setup));
}

#[test]
fn standalone_anchor_names_the_review_worktree_as_the_record_cwd() {
    let tree = std::path::Path::new("/home/u/aida-worktrees/pr-3142-review");
    let anchor =
        standalone_reviewer_record_anchor("/home/u/proj/.aida/review-verdicts/PR-3142.json", tree);
    assert!(
        anchor.contains("run from your review worktree `/home/u/aida-worktrees/pr-3142-review`"),
        "{anchor}"
    );
    assert!(
        anchor.contains("Do not cd to the primary checkout `/home/u/proj`"),
        "{anchor}"
    );
    assert!(
        !anchor.contains("run it from `/home/u/proj`"),
        "the orchestrator anchor's cd-to-root instruction must not reach a standalone reviewer: {anchor}"
    );
}

#[test]
fn standalone_prompt_carries_the_worktree_anchor_only() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        (
            "AIDA_REVIEW_VERDICT_FILE",
            Some("/home/u/proj/.aida/review-verdicts/PR-3142.json"),
        ),
        ("AIDA_FROM_PR_REVIEW", None),
        ("AIDA_GRADED_REVIEW_CONTEXT", None),
    ]);
    let tree = std::path::Path::new("/home/u/aida-worktrees/pr-3142-review");
    let mut prompt = "/aida-review --pr 3142".to_string();
    append_standalone_reviewer_prompt_suffixes(&mut prompt, tree);
    assert!(
        prompt.contains("from your review worktree `/home/u/aida-worktrees/pr-3142-review`"),
        "{prompt}"
    );
    assert_eq!(
        prompt.matches("Verdict anchor").count(),
        1,
        "exactly one anchor: {prompt}"
    );
    assert!(!prompt.contains("run it from `/home/u/proj`"), "{prompt}");
}

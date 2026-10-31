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
//!   * a contained reviewer whose grant key the launcher cannot hold in a
//!     session keyring is refused by the launcher before any reviewer runs,
//!     on the fresh and the resume launch alike, and before the prior
//!     round's verdict file is reset;
//!   * every launch path mints its child grant through the containment-aware
//!     issuer (the issuer itself is covered beside the keyring tests,
//!     `gate/tests/bug_1934_keyring_tests.rs`).
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

/// Run the REAL standalone reviewer launcher, contained, with this thread's
/// next keyring install forced to fail the way a launcher inside another
/// agent's sandbox does (`ELOOP`: it cannot hold the previous session
/// keyring). The mock reviewer would leave a marker if it ever ran.
#[cfg(unix)]
fn contained_launch_without_a_keyring_key(launch: QueueWorkLaunch, no_human: bool) {
    if !crate::grant_keyring::available() {
        eprintln!("SKIP: the session keyring is unavailable here");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    let worktree = tmp.path().join("review-worktree");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    let verdict_path = prior_round_verdict(&project, 3157);
    let marker = tmp.path().join("reviewer-ran");
    let mock = tmp.path().join("mock-reviewer.sh");
    crate::test_exec::write_executable(
        &mock,
        &format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
    );
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_AGENT_CMD", Some(mock.to_str().unwrap())),
        ("AIDA_CONTAINED", None),
    ]);
    crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|e| e.set(Some(libc::ELOOP)));
    let result = run_standalone_reviewer(
        &project,
        3157,
        launch,
        "/aida-review --pr 3157",
        None,
        "PR-3157",
        "pr-3157",
        "reviewer",
        &worktree,
        no_human,
        true,
        &verdict_path,
        true,
        session::HeadlessVendor::Claude,
    );
    crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|e| e.set(None));
    let err = result.expect_err("a contained reviewer without a keyring key must be refused");
    assert!(
        crate::gate::seat_authority::ChildGrantRefused::is_in(&err),
        "the refusal is a child-grant refusal (it releases the review lease): {err:#}"
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("cannot launch the contained `reviewer` worker"),
        "{msg}"
    );
    assert!(!marker.exists(), "the reviewer ran before the refusal");
    assert_eq!(
        std::fs::read_to_string(&verdict_path).unwrap(),
        PRIOR_APPROVAL,
        "a refused launch keeps the prior round's verdict"
    );
}

#[cfg(unix)]
#[test]
fn bug_045_contained_fresh_headless_launch_is_refused_before_the_review() {
    contained_launch_without_a_keyring_key(
        QueueWorkLaunch::Fresh("019e0000-0000-7000-8000-000000003157".to_string()),
        true,
    );
}

#[cfg(unix)]
#[test]
fn bug_045_contained_fresh_interactive_launch_is_refused_before_the_review() {
    contained_launch_without_a_keyring_key(
        QueueWorkLaunch::Fresh("019e0000-0000-7000-8000-000000003158".to_string()),
        false,
    );
}

#[cfg(unix)]
#[test]
fn bug_045_contained_headless_resume_is_refused_before_the_review() {
    contained_launch_without_a_keyring_key(
        QueueWorkLaunch::Resume("019e0000-0000-7000-8000-000000003159".to_string()),
        true,
    );
}

#[cfg(unix)]
#[test]
fn bug_045_contained_interactive_resume_is_refused_before_the_review() {
    contained_launch_without_a_keyring_key(
        QueueWorkLaunch::Resume("019e0000-0000-7000-8000-000000003160".to_string()),
        false,
    );
}

/// An uncontained launcher is not probed: the forced keyring failure is still
/// pending afterwards, so an uncontained reviewer keeps today's warn-only
/// proc-only fallback.
#[test]
fn bug_045_uncontained_reviewer_is_not_probed() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|e| e.set(Some(libc::ELOOP)));
    refuse_unpresentable_contained_reviewer(false).expect("uncontained is never refused here");
    let pending = crate::grant_keyring::FORCE_INSTALL_ERRNO.with(std::cell::Cell::take);
    assert_eq!(pending, Some(libc::ELOOP));
}

/// The refusal runs before `handle_queue_work` resets the PR's verdict file,
/// so a refused contained launch keeps the prior round's verdict.
#[test]
fn bug_045_contained_refusal_precedes_the_verdict_reset() {
    let source = include_str!("../queue_cmd.rs");
    let refuse = source
        .find("refuse_unpresentable_contained_reviewer(contained)?;")
        .expect("contained refusal in handle_queue_work");
    let clear = source
        .find("clear_standalone_reviewer_verdict(verdict_path);")
        .expect("verdict reset in handle_queue_work");
    assert!(refuse < clear, "the verdict is reset before the refusal");
}

/// Every launch path mints its child grant through the containment-aware
/// issuer, so a fresh, resumed, headless, interactive or role launch cannot
/// hand a contained worker a grant its sandbox could not present. A revert of
/// any launch site to the bare issuer fails here.
#[test]
fn bug_045_launch_paths_mint_through_the_containment_aware_issuer() {
    for (file, source) in [
        ("session.rs", include_str!("../session.rs")),
        ("queue_cmd.rs", include_str!("../queue_cmd.rs")),
        ("agent_launch.rs", include_str!("../agent_launch.rs")),
    ] {
        assert!(
            !source.contains("seat_authority::issue_child("),
            "{file} mints a launch child grant without its containment"
        );
    }
    let session = include_str!("../session.rs");
    assert!(
        session
            .matches("seat_authority::issue_launch_child(")
            .count()
            >= 3
    );
}

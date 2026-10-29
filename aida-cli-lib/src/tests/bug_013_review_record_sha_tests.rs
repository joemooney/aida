//! BUG-b6b1278890774ef993d2ebc327862470-013 regression: `aida review record
//! --sha` takes the reviewed commit from git, not from the typed string. A
//! prefix that resolves to the head under review records the full sha; a
//! mistyped sha is refused at record time and nothing is written.
// trace:BUG-b6b1278890774ef993d2ebc327862470-013 | ai:claude

use crate::*;

/// A tempdir project whose root holds one commit; returns it and that sha.
fn project() -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    let head = crate::test_git::commit_fixture(root.path());
    (root, head)
}

/// `aida review record BUG-1301 --pr 1301 --verdict approved --sha <sha>`
/// from an independent reviewer session, every ambient root pinned to `root`.
fn record(root: &std::path::Path, sha: &str) -> Result<()> {
    let root_str = root.to_str().expect("tempdir path is utf-8");
    let _pinned = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_PROJECT_ROOT", Some(root_str)),
        ("AIDA_DRIVE_ROOT", Some(root_str)),
        ("AIDA_REVIEW_VERDICT_FILE", None),
        ("AIDA_FROM_PR_HEAD_SHA", None),
        (crate::gate::seat_authority::GRANT_ENV, None),
    ]);
    let subject = ReviewPrSubject {
        head_ref: "bug-1301-work".into(),
        commit_subjects: vec![],
    };
    let reviewer = crate::gate::review_authority::IdentitySource {
        session_ids: vec!["independent-reviewer".into()],
        ..Default::default()
    };
    with_review_pr_subject(subject, || {
        crate::gate::review_authority::with_identity_source(reviewer, || {
            handle_review_record_at(
                root.to_path_buf(),
                "BUG-1301",
                "approved",
                Some(sha),
                Some("bug-1301-work"),
                Some("looks right"),
                &[],
                &[],
                Some(1301),
            )
        })
    })
}

#[test]
fn a_prefix_of_head_records_the_full_sha() {
    let (root, head) = project();
    record(root.path(), &head[..8]).expect("a prefix of HEAD must record");
    let recorded = crate::gate::review_verdict::read_recorded_verdict(root.path(), "BUG-1301")
        .expect("the verdict is written");
    assert_eq!(recorded.reviewed_sha.as_deref(), Some(head.as_str()));
    assert_eq!(head.len(), 40);
}

#[test]
fn a_mistyped_sha_is_refused_and_nothing_is_written() {
    let (root, head) = project();
    // The incident's shape: the right first seven characters, a wrong eighth.
    let mut typo = head[..8].to_string();
    let wrong = if typo.ends_with('0') { "1" } else { "0" };
    typo.replace_range(7..8, wrong);
    let err = format!(
        "{:#}",
        record(root.path(), &typo).expect_err("a mistyped sha must be refused")
    );
    assert!(err.starts_with("refused:"), "{err}");
    assert!(err.contains(&format!("`--sha {typo}`")), "{err}");
    assert!(err.contains("nothing was recorded"), "{err}");
    let path = crate::gate::review_verdict::verdict_path(root.path(), "BUG-1301");
    assert!(
        !path.exists(),
        "no verdict may be written: {}",
        path.display()
    );
}

#[test]
fn a_real_commit_that_is_not_the_head_under_review_is_refused() {
    let (root, old) = project();
    let new = crate::test_git::commit_fixture(root.path());
    assert_ne!(old, new);
    let err = format!(
        "{:#}",
        record(root.path(), &old).expect_err("a commit other than HEAD must be refused")
    );
    assert!(err.contains(&format!("resolves to {old}")), "{err}");
    assert!(
        err.contains(&format!("the head under review is {new}")),
        "{err}"
    );
    assert!(crate::gate::review_verdict::read_recorded_verdict(root.path(), "BUG-1301").is_none());
}

//! BUG-b6b1278890774ef993d2ebc327862470-013 regression: `aida review record
//! --sha` takes the reviewed commit from git, not from the typed string. A
//! prefix that resolves to the head under review records the full sha; a
//! mistyped sha is refused at record time and nothing is written.
// trace:BUG-b6b1278890774ef993d2ebc327862470-013 | ai:claude

use crate::{handle_review_record_at, with_review_pr_subject, ReviewPrSubject};
use anyhow::Result;

/// A tempdir project whose root holds one commit; returns it and that sha.
fn project() -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    let head = crate::test_git::commit_fixture(root.path());
    (root, head)
}

/// `aida review record BUG-1301 --pr 1301 --verdict approved --sha <sha>`
/// from an independent reviewer session, every ambient root pinned to `root`.
/// The fixture has no forge, so without an envelope the PR head is unknown.
fn record(root: &std::path::Path, sha: &str) -> Result<()> {
    record_with(root, Some(sha), None)
}

/// [`record`] with an optional `--sha` and an optional launch-envelope PR
/// head (`AIDA_FROM_PR_HEAD_SHA`).
fn record_with(root: &std::path::Path, sha: Option<&str>, envelope: Option<&str>) -> Result<()> {
    let root_str = root.to_str().expect("tempdir path is utf-8");
    let _pinned = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_PROJECT_ROOT", Some(root_str)),
        ("AIDA_DRIVE_ROOT", Some(root_str)),
        ("AIDA_REVIEW_VERDICT_FILE", None),
        ("AIDA_FROM_PR_HEAD_SHA", envelope),
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
                sha,
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

fn recorded_sha(root: &std::path::Path) -> Option<String> {
    crate::gate::review_verdict::read_recorded_verdict(root, "BUG-1301")
        .and_then(|v| v.reviewed_sha)
}

/// Advisor ruling: a `--pr` whose forge cannot report the head falls back to
/// the checkout's HEAD. The typed sha is checked against it, and the refusal
/// does not send the caller to omit `--sha` (that would fail on the same
/// forge read).
#[test]
fn an_unreachable_forge_checks_against_the_checkout_head() {
    let (root, old) = project();
    let new = crate::test_git::commit_fixture(root.path());
    let err = format!(
        "{:#}",
        record(root.path(), &old).expect_err("a non-checkout commit is refused")
    );
    assert!(
        err.contains(&format!("the head under review is {new}")),
        "{err}"
    );
    assert!(err.contains("checkout's HEAD stands in"), "{err}");
    assert!(!err.contains("omit `--sha`"), "{err}");
    assert!(recorded_sha(root.path()).is_none());

    record(root.path(), &new[..9]).expect("the checkout head records");
    assert_eq!(recorded_sha(root.path()), Some(new));
}

/// The launch envelope's PR head outranks the checkout: `--sha` must match
/// the envelope, even when the checkout has moved past it.
#[test]
fn an_envelope_head_is_the_head_a_typed_sha_must_match() {
    let (root, pr_head) = project();
    let checkout = crate::test_git::commit_fixture(root.path());
    let err = format!(
        "{:#}",
        record_with(root.path(), Some(&checkout), Some(&pr_head))
            .expect_err("the checkout's HEAD is not the PR head")
    );
    assert!(
        err.contains(&format!("the head under review is {pr_head}")),
        "{err}"
    );
    assert!(err.contains("omit `--sha`"), "{err}");
    assert!(recorded_sha(root.path()).is_none());

    record_with(root.path(), Some(&pr_head[..8]), Some(&pr_head))
        .expect("a prefix of the envelope head records");
    assert_eq!(recorded_sha(root.path()), Some(pr_head));
}

/// Without `--sha` the head under review is recorded as git's full sha, even
/// when the envelope carries an abbreviated one.
#[test]
fn the_default_path_records_the_full_sha() {
    let (root, pr_head) = project();
    crate::test_git::commit_fixture(root.path());
    record_with(root.path(), None, Some(&pr_head[..10])).expect("the default path records");
    assert_eq!(recorded_sha(root.path()), Some(pr_head));
}

// trace:BUG-2522 | ai:codex
#[test]
fn native_launcher_refusal_records_without_placing_a_hold() {
    let (root, head) = project();
    let anchor = root.path().join(".aida/review-verdicts/PR-2522.json");
    let root_str = root.path().to_str().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_PROJECT_ROOT", Some(root_str)),
        ("AIDA_DRIVE_ROOT", Some(root_str)),
        ("AIDA_REVIEW_VERDICT_FILE", Some(anchor.to_str().unwrap())),
        ("AIDA_CONTAINED", None),
        ("AIDA_FROM_PR_HEAD_SHA", Some(&head)),
        (crate::gate::seat_authority::GRANT_ENV, None),
    ]);
    assert!(!crate::session::process_contained());
    let result = handle_review_record_at(
        root.path().to_path_buf(),
        "PR-2522",
        "request-changes",
        Some(&head),
        None,
        Some("needs a fix"),
        &[],
        &[],
        Some(2522),
    );
    // This minimal project has no spec store to park; that later failure
    // must not hide the persisted refusal or attempt primary-side holding.
    if let Err(error) = result {
        assert!(error.to_string().contains("could not park"), "{error:#}");
    }
    let verdict =
        crate::gate::review_verdict::read_recorded_verdict(root.path(), "PR-2522").unwrap();
    assert!(verdict.kind.requires_changes());
    assert!(crate::gate::merge_hold_core::read_hold_record(root.path(), 2522).is_none());
    let collected = crate::gate::review_verdict::CollectedVerdict {
        verdict,
        spec_records: vec![],
        collected_from_worktree: false,
    };
    assert!(crate::merge_hold::place_collected_refusal(root.path(), 2522, &collected).unwrap());
    assert!(crate::gate::merge_hold_core::read_hold_record(root.path(), 2522).is_some());
}

// trace:BUG-2522 | ai:codex
#[test]
fn operator_hold_failure_preserves_both_verdict_records() {
    let (root, head) = project();
    let root_str = root.path().to_str().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_PROJECT_ROOT", Some(root_str)),
        ("AIDA_DRIVE_ROOT", Some(root_str)),
        ("AIDA_REVIEW_VERDICT_FILE", None),
        ("AIDA_CONTAINED", None),
        ("AIDA_FROM_PR_HEAD_SHA", Some(&head)),
        (crate::gate::seat_authority::GRANT_ENV, None),
    ]);
    // A directory in place of the lock refuses hold placement, without
    // preventing either verdict write in the same ledger directory.
    std::fs::create_dir_all(crate::gate::merge_hold_core::hold_lock_path(
        root.path(),
        2522,
    ))
    .unwrap();
    let error = handle_review_record_at(
        root.path().to_path_buf(),
        "BUG-2522",
        "request-changes",
        Some(&head),
        None,
        Some("needs a fix"),
        &[],
        &[],
        Some(2522),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("verdict recorded, but could not protect"),
        "{error:#}"
    );
    for key in ["BUG-2522", "PR-2522"] {
        assert!(
            crate::gate::review_verdict::read_recorded_verdict(root.path(), key)
                .unwrap()
                .kind
                .requires_changes()
        );
    }
}

// trace:BUG-2522 | ai:codex
#[test]
fn interactive_review_anchor_refusals_are_collected_and_held() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    for (disposition, file_anchor, label_fails) in [
        ("request-changes", false, false),
        ("rejected", false, false),
        ("request-changes", true, false),
        ("rejected", true, false),
        ("request-changes", true, true),
        ("rejected", true, true),
    ] {
        let (root, head) = project();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let anchor = crate::gate::review_verdict::verdict_path(root.path(), "BUG-2522");
        let root_str = root.path().to_str().unwrap();
        let mut env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_PROJECT_ROOT", Some(root_str)),
            ("AIDA_DRIVE_ROOT", Some(root_str)),
            ("AIDA_REVIEW_VERDICT_FILE", Some(anchor.to_str().unwrap())),
            ("AIDA_HOME", Some(home.to_str().unwrap())),
            ("HOME", Some(home.to_str().unwrap())),
            ("AIDA_CONTAINED", None),
            ("AIDA_FROM_PR_HEAD_SHA", Some(&head)),
            (crate::gate::seat_authority::GRANT_ENV, None),
        ]);
        // Exercise the actual parent's forge-label side effect as well as
        // its local hold. A pure-git fixture silently skips label sync.
        // trace:BUG-2522 | ai:codex
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(root.path())
            .args([
                "remote",
                "add",
                "origin",
                "git@github.com:fixture/review.git"
            ])
            .status()
            .unwrap()
            .success());
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "[forge]\nprovider = \"github\"\n",
        )
        .unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let gh = bin.join("gh");
        crate::test_exec::write_executable(
            &gh,
            "#!/bin/sh\nif [ \"$1 $2\" = 'pr edit' ]; then\n  printf '%s\\n' \"$@\" >> .aida/label-calls\n  if [ -f .aida/refuse-label ]; then\n    echo 'forge unavailable' >&2\n    exit 1\n  fi\n  exit 0\nfi\nexit 1\n",
        );
        // trace:BUG-2522 | ai:codex
        let inputs = aida_core::exec_context::Environment::capture();
        let path = format!("{}:{}", bin.display(), inputs.var("PATH").unwrap());
        env.set_key("PATH", &path);
        let label_calls = root.path().join(".aida/label-calls");
        let grant = crate::gate::seat_authority::test_support::mint_grant_for(
            root.path(),
            &crate::current_user_id(None),
            "reviewer",
            &[],
        );
        env.set_key(crate::gate::seat_authority::GRANT_ENV, &grant);
        let launched_at = std::time::SystemTime::now();
        assert!(!crate::session::process_contained());
        let result = handle_review_record_at(
            root.path().to_path_buf(),
            "BUG-2522",
            disposition,
            Some(&head),
            None,
            Some("needs a fix"),
            &[],
            &[],
            Some(2522),
        );
        if let Err(error) = result {
            assert!(error.to_string().contains("could not park"), "{error:#}");
        }
        assert!(crate::gate::merge_hold_core::read_hold_record(root.path(), 2522).is_none());
        assert!(
            !label_calls.exists(),
            "the child must leave labeling to the parent"
        );
        // Native record is ledger-only, under the SPEC anchor, not PR-2522.
        assert!(!anchor.exists());
        // Interactive skills can also leave a file at the SPEC anchor. Keep
        // the original child's grant/head/timestamp intact and exercise that
        // collection route as well: an anchor file must not bypass holding.
        // trace:BUG-2522 | ai:codex
        if file_anchor {
            let body = crate::gate::review_verdict::current_verdict_body_at(&anchor)
                .expect("the child's original recording is readable");
            std::fs::create_dir_all(anchor.parent().unwrap()).unwrap();
            std::fs::write(&anchor, body).unwrap();
            assert!(anchor.is_file());
            // Model a skill that writes only the interactive anchor, without
            // running `review record`. Remove the fixture's seed ledger so
            // collection cannot accidentally pass through ledger evidence.
            // trace:BUG-2522 | ai:codex
            let ledger_root = crate::gate::ledger::store_root(root.path()).unwrap();
            assert_eq!(ledger_root, root.path());
            let ledger_dir = crate::gate::ledger::ledger_dir(&ledger_root);
            std::fs::remove_dir_all(&ledger_dir).unwrap();
            assert!(!ledger_dir.exists());
            assert!(
                crate::gate::review_verdict::read_recorded_verdict(root.path(), "PR-2522")
                    .is_none(),
                "only the SPEC anchor may supply this interactive refusal"
            );
        }
        let mut checks = crate::queue_cmd::StandaloneVerdictChecks {
            project_root: root.path(),
            pr: 2522,
            head_sha: Some(head.clone()),
            launcher_grant: None,
            child_grant: Some("other-reviewer".into()),
        };
        assert!(
            crate::gate::review_verdict::collect_interactive_review_refusal(
                root.path(),
                2522,
                "BUG-2522",
                launched_at,
                &checks,
            )
            .is_err(),
            "another reviewer's grant must not collect"
        );
        assert!(crate::gate::merge_hold_core::read_hold_record(root.path(), 2522).is_none());
        assert!(
            !label_calls.exists(),
            "an invalid grant must not label the PR"
        );
        checks.child_grant = Some(grant);
        // A matching head/grant from an earlier round is still not this
        // interactive child's verdict. It must leave both hold surfaces clear.
        // trace:BUG-2522 | ai:codex
        let next_launch = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        assert!(
            crate::gate::review_verdict::collect_interactive_review_refusal(
                root.path(),
                2522,
                "BUG-2522",
                next_launch,
                &checks,
            )
            .is_err(),
            "a verdict recorded before this launch must not collect"
        );
        assert!(crate::gate::merge_hold_core::read_hold_record(root.path(), 2522).is_none());
        assert!(
            !label_calls.exists(),
            "an earlier round must not label the PR"
        );
        checks.head_sha = Some("0000000000000000000000000000000000000000".into());
        assert!(
            crate::gate::review_verdict::collect_interactive_review_refusal(
                root.path(),
                2522,
                "BUG-2522",
                launched_at,
                &checks,
            )
            .is_err(),
            "a different PR head must not collect"
        );
        assert!(crate::gate::merge_hold_core::read_hold_record(root.path(), 2522).is_none());
        assert!(
            !label_calls.exists(),
            "a stale verdict must not label the PR"
        );
        checks.head_sha = Some(head.clone());
        let lock = crate::gate::merge_hold_core::hold_lock_path(root.path(), 2522);
        std::fs::create_dir_all(&lock).unwrap();
        let error = crate::gate::review_verdict::collect_interactive_review_refusal(
            root.path(),
            2522,
            "BUG-2522",
            launched_at,
            &checks,
        )
        .unwrap_err();
        assert!(error.to_string().contains("could not protect"), "{error:#}");
        assert!(
            !label_calls.exists(),
            "a failed hold must not attempt labeling"
        );
        assert!(
            crate::gate::review_verdict::read_recorded_verdict(root.path(), "BUG-2522")
                .unwrap()
                .kind
                .requires_changes()
        );
        std::fs::remove_dir(&lock).unwrap();
        // The interactive parent must keep the first refusal's local hold
        // even when the forge cannot apply its label. Preserve the repair
        // diagnostic rather than treating collection as an unprotected review.
        // trace:BUG-2522 | ai:codex
        if label_fails {
            std::fs::write(root.path().join(".aida/refuse-label"), "").unwrap();
        }
        crate::gate::review_verdict::collect_interactive_review_refusal(
            root.path(),
            2522,
            "BUG-2522",
            launched_at,
            &checks,
        )
        .expect("the interactive parent must protect its child's refusal");
        let hold = crate::gate::merge_hold_core::read_hold_record(root.path(), 2522).unwrap();
        assert_eq!(
            hold.reason_kind,
            crate::gate::merge_hold_core::HoldReasonKind::Rework
        );
        assert_eq!(hold.target_head_sha.as_deref(), Some(head.as_str()));
        let label_call = "pr\nedit\n2522\n-R\nfixture/review\n--add-label\naida:merge-hold,aida:merge-hold-recorded\n--remove-label\naida:merge-hold-cleared\n";
        assert_eq!(
            std::fs::read_to_string(label_calls).unwrap(),
            label_call.repeat(if label_fails { 2 } else { 1 }),
            "the interactive parent must apply the labels, retrying a forge failure"
        );
        let label_state = crate::gate::merge_hold_core::read_label_state(root.path(), 2522);
        if label_fails {
            assert_eq!(
                label_state,
                crate::gate::merge_hold_core::LabelState::Unsynced("forge unavailable".into())
            );
        } else {
            assert_eq!(
                label_state,
                crate::gate::merge_hold_core::LabelState::Synced
            );
        }
    }
}

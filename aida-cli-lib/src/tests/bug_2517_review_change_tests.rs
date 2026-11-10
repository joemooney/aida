// trace:BUG-2517 | ai:codex
use crate::gate::ledger::{self, ChangeKey};
use crate::gate::review_verdict as rv;
use crate::*;
use serde_json::json;

fn lease(scope: &str) -> SessionLease {
    serde_json::from_value(json!({
        "id":"251725172517", "scope":scope, "slug":"review", "owner":"test",
        "worktree_path":"/not-the-checkout", "branch":"review",
        "started_at":"2026-11-10T00:00:00Z", "hostname":"test",
        "role":"reviewer", "review_verb":true
    }))
    .unwrap()
}

#[test]
fn bug_2517_lease_selects_pr_and_mr_and_refuses_conflicting_names() {
    for scope in ["PR-3764", "MR-3764"] {
        let leases = [lease(scope)];
        let tokens = vec!["lease:251725172517".to_string()];
        let resolve =
            |spec, pr| review_record_change(&leases, &tokens, spec, pr, forge::ForgeKind::GitHub);
        assert_eq!(resolve("BUG-2492", None).unwrap(), ChangeKey::parse(scope));
        assert_eq!(
            resolve("BUG-2492", Some(3764)).unwrap(),
            ChangeKey::parse(scope)
        );
        assert!(resolve("BUG-2492", Some(3651)).is_err());
        assert!(resolve("PR-3651", None).is_err());
    }
    assert_eq!(
        review_record_change(&[], &[], "BUG-2492", None, forge::ForgeKind::GitHub).unwrap(),
        None
    );
}

#[test]
fn bug_2517_two_prs_one_spec_records_only_the_leased_change() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let head = crate::test_git::commit_fixture(root);
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/test/repo.git",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    crate::test_exec::write_executable(&bin.join("gh"), &format!(
        "#!/bin/sh\nprintf '%s\\n' '{{\"url\":\"https://github.com/test/repo/pull/3764\",\"state\":\"OPEN\",\"headRefOid\":\"{head}\",\"labels\":[]}}'\n"));
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        root,
        None,
        &[
            ("PATH", Some(&path)),
            ("AIDA_CONTAINED", None),
            ("AIDA_SESSION_ID", Some("251725172517")),
            ("AIDA_PROJECT_ROOT", Some(root.to_str().unwrap())),
            ("AIDA_REVIEW_VERDICT_FILE", None),
            (crate::gate::seat_authority::GRANT_ENV, None),
        ],
    );
    let l = lease("PR-3764");
    let file = crate::lease_path(root, &l.id);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, toml::to_string(&l).unwrap()).unwrap();
    let first = rv::verdict_path(root, "PR-3651");
    let rec = json!({"verdict":"approved", "reviewed_sha":"1111111111111111111111111111111111111111",
        "recorded_at":"2026-11-09T00:00:00Z", "recorded_by":"prior-reviewer"});
    rv::write_verdict_object_for_spec(&first, rec.as_object().unwrap(), Some("BUG-2492")).unwrap();
    let before = rv::current_verdict_body_at(&first).unwrap();
    let bytes = std::fs::read(ledger::store::ledger_file(root, ChangeKey::pr(3651))).unwrap();
    handle_review_record_at(
        root.to_path_buf(),
        "BUG-2492",
        "waiting",
        Some(&head),
        None,
        Some("operator must run the canary; resume when its result is available"),
        &[],
        &[],
        None,
    )
    .unwrap();
    assert_eq!(rv::current_verdict_body_at(&first).unwrap(), before);
    assert_eq!(
        std::fs::read(ledger::store::ledger_file(root, ChangeKey::pr(3651))).unwrap(),
        bytes
    );
    let second = rv::read_recorded_verdict(root, "PR-3764").unwrap();
    assert_eq!(second.reviewed_sha.as_deref(), Some(head.as_str()));
    crate::test_exec::write_executable(&bin.join("gh"), "#!/bin/sh\nexit 1\n");
    assert!(handle_review_record_at(
        root.to_path_buf(),
        "BUG-2492",
        "waiting",
        Some(&head),
        None,
        Some("operator must run the canary; resume after its result"),
        &[],
        &[],
        Some(3764)
    )
    .is_err());
    assert_eq!(rv::current_verdict_body_at(&first).unwrap(), before);
    // A local review with no change lease stays unbound, even with two
    // changes already indexed for the same spec and an unavailable forge.
    std::fs::remove_file(file).unwrap();
    let second_path = rv::verdict_path(root, "PR-3764");
    let second_before = rv::current_verdict_body_at(&second_path).unwrap();
    handle_review_record_at(
        root.to_path_buf(),
        "BUG-2492",
        "waiting",
        Some(&head),
        None,
        Some("operator must run the local check; resume after its result"),
        &[],
        &[],
        None,
    )
    .unwrap();
    assert_eq!(rv::current_verdict_body_at(&first).unwrap(), before);
    assert_eq!(
        rv::current_verdict_body_at(&second_path).unwrap(),
        second_before
    );
}

#[test]
fn bug_2517_head_moving_during_recording_refuses_before_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let head = crate::test_git::commit_fixture(root);
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_and(root, None, &[("AIDA_CONTAINED", None)]);
    let reads = std::cell::Cell::new(0);
    let result = handle_review_record_with_head_at(
        root.to_path_buf(),
        "BUG-2492",
        "waiting",
        Some(&head),
        None,
        Some("operator must run the canary; resume after its result"),
        &[],
        &[],
        Some(3764),
        |_, change| {
            assert_eq!(change, ChangeKey::pr(3764));
            reads.set(reads.get() + 1);
            Ok(if reads.get() == 1 {
                head.clone()
            } else {
                "2222222222222222222222222222222222222222".into()
            })
        },
    );
    assert!(result.unwrap_err().to_string().contains("head moved"));
    assert_eq!(reads.get(), 2);
    assert!(!ledger::store::ledger_file(root, ChangeKey::pr(3764)).exists());
    assert!(rv::read_recorded_verdict(root, "BUG-2492").is_none());
}

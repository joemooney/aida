// trace:BUG-2517 | ai:codex
use crate::gate::ledger::{self, ChangeKey};
use crate::gate::review_verdict as rv;
use crate::{
    forge, handle_review_record_at, handle_review_record_with_head_at, review_record_change,
    SessionLease,
};
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
    let anchor = rv::verdict_path(root, "PR-3764-security");
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        root,
        None,
        &[
            ("PATH", Some(&path)),
            ("AIDA_CONTAINED", None),
            ("AIDA_SESSION_ID", Some("251725172517")),
            ("AIDA_PROJECT_ROOT", Some(root.to_str().unwrap())),
            ("AIDA_REVIEW_VERDICT_FILE", Some(anchor.to_str().unwrap())),
            (crate::gate::seat_authority::GRANT_ENV, None),
        ],
    );
    let l = lease("PR-3764");
    let file = crate::lease_path(root, &l.id);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, toml::to_string(&l).unwrap()).unwrap();
    let first = rv::verdict_path(root, "PR-3651");
    let rec = json!({"verdict":"request-changes", "findings":["foreign actionable defect"], "reviewed_sha":head,
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
    assert!(
        rv::current_verdict_body_at(&anchor).is_some(),
        "explicit gate anchor must survive"
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
    // A real local record settles neither of the two live change claims.
    for n in [3651, 3764] {
        let marker = crate::gate::review_marker::Marker::for_this_process(
            n,
            Some(&head),
            Some("BUG-2492"),
            "reviewer",
        );
        crate::gate::review_marker::write(root, &marker).unwrap();
    }
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
    for n in [3651, 3764] {
        assert!(crate::gate::review_marker::read(root, n).is_some());
    }
    // Opening/adopting or collecting either change must not adopt the local record.
    let spec = crate::gate::canon::GateSpec::with_key("BUG-2492", None);
    for n in [3651, 3764] {
        let path = ledger::store::ledger_file(root, ChangeKey::pr(n));
        let before = std::fs::read(&path).unwrap();
        assert_eq!(
            ledger::store::bind_unbound(root, &spec, ChangeKey::pr(n)).unwrap(),
            0
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
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
        false,
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

#[test]
fn bug_2517_inherited_findings_use_the_same_change_as_the_builder() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let head = crate::test_git::commit_fixture(root);
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_and(root, None, &[("AIDA_CONTAINED", None)]);
    let own = rv::verdict_path(root, "PR-3764");
    let prior = json!({"verdict":"request-changes", "reviewed_sha":head,
        "findings":["own actionable defect"], "recorded_at":"2026-11-09T00:00:00Z"});
    rv::write_verdict_object(&own, prior.as_object().unwrap()).unwrap();
    let before = rv::current_verdict_body_at(&own).unwrap();
    let other = rv::verdict_path(root, "PR-3651");
    let clean = json!({"verdict":"approved", "reviewed_sha":head, "findings":[]});
    rv::write_verdict_object(&other, clean.as_object().unwrap()).unwrap();
    assert_eq!(
        crate::review_record_kind(&own, "approved", &[]).unwrap(),
        rv::VerdictKind::RequestChanges
    );
    assert_eq!(
        crate::review_record_kind(&other, "approved", &[]).unwrap(),
        rv::VerdictKind::Approved
    );
    assert_eq!(rv::current_verdict_body_at(&own).unwrap(), before);
    // Reverse the differing prior records: foreign findings cannot turn this approval into a hold.
    rv::write_verdict_object(&own, clean.as_object().unwrap()).unwrap();
    rv::write_verdict_object(&other, prior.as_object().unwrap()).unwrap();
    assert_eq!(
        crate::review_record_kind(&own, "approved", &[]).unwrap(),
        rv::VerdictKind::Approved
    );
}

#[test]
fn bug_2517_contained_mr_handshake_collects_only_into_its_mr_ledger() {
    let tmp = tempfile::tempdir().unwrap();
    let launcher = tmp.path();
    let worktree = launcher.join("reviewer");
    std::fs::create_dir(&worktree).unwrap();
    let head = crate::test_git::commit_fixture(&worktree);
    let canonical = rv::verdict_path(launcher, "MR-3764");
    let local = rv::verdict_path(&worktree, "MR-3764");
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        &worktree,
        None,
        &[
            ("AIDA_CONTAINED", Some("1")),
            ("AIDA_SESSION_ID", Some("251725172517")),
            ("AIDA_REVIEW_VERDICT_FILE", Some(local.to_str().unwrap())),
        ],
    );
    let l = lease("MR-3764");
    let file = crate::lease_path(&worktree, &l.id);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, toml::to_string(&l).unwrap()).unwrap();
    handle_review_record_with_head_at(
        worktree.clone(),
        "BUG-2492",
        "waiting",
        Some(&head),
        None,
        Some("operator must run check; resume after result"),
        &[],
        &[],
        None,
        false,
        |_, c| {
            assert_eq!(c, ChangeKey::parse("MR-3764").unwrap());
            Ok(head.clone())
        },
    )
    .unwrap();
    // Supply the launch grant stamp; collection checks below verify it, independently
    // of live seat issuance (covered by the collection authority fixtures).
    let mut obj: serde_json::Value =
        serde_json::from_str(&rv::current_verdict_body_at(&local).unwrap()).unwrap();
    obj["recorder_attestation"]["grant_id"] = "reviewer-grant".into();
    std::fs::write(&local, obj.to_string()).unwrap();
    struct Checks;
    impl rv::VerdictCollectionChecks for Checks {
        fn check_record(&self, c: &rv::VerdictCandidate) -> Result<(), String> {
            assert_eq!(c.verdict.kind, rv::VerdictKind::Waiting);
            Ok(())
        }
        fn check_identity(&self, grant: &str) -> Result<(), String> {
            assert_eq!(grant, "reviewer-grant");
            Ok(())
        }
    }
    drop(_ambient);
    let _host =
        crate::test_env::AmbientGuard::hermetic_and(launcher, None, &[("AIDA_CONTAINED", None)]);
    let collected = rv::collect_standalone_verdict(
        launcher,
        &worktree,
        &canonical,
        &[],
        std::time::UNIX_EPOCH,
        &Checks,
    )
    .unwrap();
    assert!(collected.collected_from_worktree);
    assert!(rv::read_recorded_verdict(launcher, "MR-3764").is_some());
    assert!(!ledger::store::ledger_file(launcher, ChangeKey::pr(3764)).exists());
}

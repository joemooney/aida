// trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
use super::*;
use crate::gate::review_verdict::{RecordedVerdict, VerdictKind};
use std::collections::HashSet;

fn approved_verdict(findings: &[&str]) -> RecordedVerdict {
    RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".to_string(),
        findings: findings.iter().map(|s| s.to_string()).collect(),
        inherited_findings: false,
        ..Default::default()
    }
}

/// An APPROVED verdict carrying 2 non-blocking findings, with nothing filed
/// yet, needs a successor for both of them.
// trace:STORY-1421 | ai:claude
#[test]
fn approved_verdict_with_two_findings_needs_two_successors() {
    let verdict = approved_verdict(&["F2: doc comment misplaced", "F3: same defect, second site"]);
    let to_file = findings_needing_a_successor(&verdict, &HashSet::new());
    assert_eq!(to_file.len(), 2);
    let texts: Vec<&str> = to_file.iter().map(|(t, _)| t.as_str()).collect();
    assert!(texts.contains(&"F2: doc comment misplaced"));
    assert!(texts.contains(&"F3: same defect, second site"));
    // Each gets a distinct hash tag — the idempotency key.
    assert_ne!(to_file[0].1, to_file[1].1);
}

/// Re-running the planner with the hashes the first pass already produced
/// (simulating a re-observed auto-bump scan on an already-completed spec)
/// yields nothing — no duplicates.
// trace:STORY-1421 | ai:claude
#[test]
fn rerun_with_already_filed_hashes_yields_no_duplicates() {
    let verdict = approved_verdict(&["F2: doc comment misplaced", "F3: same defect, second site"]);
    let first_pass = findings_needing_a_successor(&verdict, &HashSet::new());
    assert_eq!(first_pass.len(), 2);

    let already_filed: HashSet<String> = first_pass.iter().map(|(_, hash)| hash.clone()).collect();
    let second_pass = findings_needing_a_successor(&verdict, &already_filed);
    assert!(
        second_pass.is_empty(),
        "a re-run against the same verdict must not refile already-filed findings"
    );
}

/// An APPROVED verdict with no findings at all has nothing to carry forward.
// trace:STORY-1421 | ai:claude
#[test]
fn approved_verdict_with_no_findings_yields_none() {
    let verdict = approved_verdict(&[]);
    let to_file = findings_needing_a_successor(&verdict, &HashSet::new());
    assert!(to_file.is_empty());
}

/// A blocking verdict's findings are rework, not a carry-forward case — even
/// with findings present, nothing is emitted.
// trace:STORY-1421 | ai:claude
#[test]
fn request_changes_verdict_never_emits_findings() {
    let verdict = RecordedVerdict {
        kind: VerdictKind::RequestChanges,
        raw: "request-changes".to_string(),
        findings: vec!["blocking defect".to_string()],
        inherited_findings: false,
        ..Default::default()
    };
    let to_file = findings_needing_a_successor(&verdict, &HashSet::new());
    assert!(to_file.is_empty());
}

/// A duplicate line within a single verdict's findings list is only carried
/// forward once — the within-call de-dupe.
// trace:STORY-1421 | ai:claude
#[test]
fn duplicate_finding_text_in_one_verdict_files_once() {
    let verdict = approved_verdict(&["same text", "same text"]);
    let to_file = findings_needing_a_successor(&verdict, &HashSet::new());
    assert_eq!(to_file.len(), 1);
}

// ── Targeted-write integration test (BUG-1506) ──────────────────────────

/// Tiny shell-out helper — setup only, panics loudly on failure.
fn run_git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git binary on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Mirrors `auto_bump_done_tests::init_git_canonical_test_project`: a code
/// repo plus an orphan-branch-shaped store dir, both real git repos.
fn init_git_canonical_test_store() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let project_root = tmp.path().to_path_buf();
    run_git(&project_root, &["init", "--initial-branch=main", "--quiet"]);
    run_git(&project_root, &["config", "user.email", "test@example.com"]);
    run_git(&project_root, &["config", "user.name", "Test"]);
    std::fs::write(project_root.join("README.md"), "init\n").unwrap();
    run_git(&project_root, &["add", "README.md"]);
    run_git(&project_root, &["commit", "-m", "chore: init"]);

    let store_dir = project_root.join(".aida-store");
    std::fs::create_dir_all(&store_dir).unwrap();
    run_git(
        &store_dir,
        &["init", "--initial-branch=aida-store", "--quiet"],
    );
    run_git(&store_dir, &["config", "user.email", "test@example.com"]);
    run_git(&store_dir, &["config", "user.name", "Test"]);
    // Configured store readers require a committed snapshot, even when empty.
    // trace:STORY-2698 | ai:codex
    run_git(
        &store_dir,
        &["commit", "--allow-empty", "-m", "fixture: empty store"],
    );
    // trace:BUG-2017 | ai:codex — enroll before constructing the live dispenser.
    crate::test_env::enroll_synthetic_home(&store_dir).unwrap();

    // trace:STORY-2698 | ai:codex — host approval discovers the configured store.
    std::fs::create_dir_all(project_root.join(".aida")).unwrap();
    std::fs::write(
        project_root.join(".aida/config.toml"),
        "[deployment]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    (tmp, project_root, store_dir)
}

/// A locked sweep creation preserves unrelated objects and carries N findings
/// in one new spec. Re-observing the verdict is idempotent.
// trace:TASK-3749 trace:BUG-1506 | ai:codex
#[test]
fn emit_writes_targeted_commits_and_preserves_concurrent_spec() {
    use aida_core::db::DatabaseBackend;

    let (_tmp, project_root, store_dir) = init_git_canonical_test_store();
    let _guard = crate::test_env::AmbientGuard::hermetic_and(
        &project_root,
        None,
        &[("AIDA_CONTAINED", None)],
    );

    // Simulate a concurrent write: another session's `aida add` lands a
    // brand-new spec in the store that this call never loads a full
    // in-memory snapshot of.
    let dispenser = load_dispenser(&store_dir).unwrap();
    let concurrent_backend = crate::test_env::home_fixture_backend(&store_dir)
        .unwrap()
        .with_dispenser(dispenser);
    let mut concurrent_req =
        aida_core::Requirement::new("concurrently added".to_string(), String::new());
    concurrent_req.req_type = aida_core::RequirementType::Task;
    concurrent_req.status = aida_core::RequirementStatus::Draft;
    let written_concurrent = concurrent_backend.add_requirement(concurrent_req).unwrap();
    let concurrent_spec_id = written_concurrent.spec_id.clone().unwrap();

    let seed_head = run_git(&store_dir, &["rev-parse", "HEAD"]);

    // An APPROVED verdict carrying 2 non-blocking findings for the spec
    // that's completing.
    let verdict_path = crate::gate::review_verdict::verdict_path(&project_root, "STORY-9821");
    std::fs::create_dir_all(verdict_path.parent().unwrap()).unwrap();
    std::fs::write(
        &verdict_path,
        r#"{"verdict":"approved","findings":["F1: finding text","F2: finding text"],"finding_dispositions":[{"severity":"minor","disposition":"deferred","rationale":"follow-up"},{"severity":"minor","disposition":"deferred","rationale":"follow-up"}],"reviewed_sha":"deadbeefcafe"}"#,
    )
    .unwrap();

    let filed = try_emit_nonblocking_findings_on_completion(
        &project_root,
        &store_dir,
        "STORY-9821",
        "landedsha",
        Some(7),
    )
    .unwrap();
    assert_eq!(
        filed, 2,
        "both non-blocking findings should be carried forward"
    );

    // The concurrently-added spec must survive — the whole point of the
    // targeted write.
    let check_dispenser = load_dispenser(&store_dir).unwrap();
    let check_backend = crate::test_env::home_fixture_backend(&store_dir)
        .unwrap()
        .with_dispenser(check_dispenser);
    let survived = check_backend
        .get_requirement_by_spec_id(&concurrent_spec_id)
        .unwrap();
    assert!(
        survived.is_some(),
        "{} (added concurrently) must NOT be deleted by the targeted finding write",
        concurrent_spec_id
    );

    // Both findings landed in one sweep, tagged back to the source spec.
    let all = check_backend.list_requirements(true).unwrap();
    let carried: Vec<_> = all
        .iter()
        .filter(|r| r.tags.iter().any(|t| t == "carried-from:STORY-9821"))
        .collect();
    assert_eq!(carried.len(), 1);
    assert!(carried[0].tags.contains("kind:finding-sweep"));
    // trace:TASK-3777 | ai:codex
    assert!(carried[0].description.starts_with("## Findings from "));
    assert!(carried[0].description.contains("F1: finding text"));
    assert!(carried[0].description.contains("F2: finding text"));
    assert!(carried[0]
        .description
        .contains("Reviewed sha: deadbeefcafe"));
    assert_eq!(all.len(), 2, "N findings create exactly one sweep");
    for req in &carried {
        assert!(req.tags.iter().any(|t| t.starts_with("finding-hash:")));
        assert!(req.tags.iter().any(|t| t == "from-review:PR-7"));
    }

    // The transaction writes only changed objects; no bulk chore save.
    let new_subjects = run_git(
        &store_dir,
        &["log", "--format=%s", &format!("{}..HEAD", seed_head)],
    );
    assert!(
        !new_subjects.lines().any(|s| s.starts_with("chore: update")),
        "no bulk chore commit expected, got: {:?}",
        new_subjects
    );

    // A re-run against the same verdict must not duplicate the findings —
    // idempotency survives the targeted-write path too.
    let refiled = try_emit_nonblocking_findings_on_completion(
        &project_root,
        &store_dir,
        "STORY-9821",
        "landedsha",
        Some(7),
    )
    .unwrap();
    assert_eq!(
        refiled, 0,
        "a re-run must not refile already-carried findings"
    );
}

// trace:STORY-2698 | ai:codex
fn deferred_record_json(findings: serde_json::Value, sha: Option<&str>) -> String {
    let metadata: Vec<_> = findings.as_array().unwrap().iter().map(|_| {
        serde_json::json!({"severity":"minor","disposition":"deferred","rationale":"safe follow-up"})
    }).collect();
    let mut record = serde_json::json!({"verdict":"approved","findings":findings,"finding_dispositions":metadata});
    if let Some(sha) = sha {
        record["reviewed_sha"] = sha.into();
    }
    record.to_string()
}

// trace:TASK-3749 | ai:codex
#[test]
fn task_3749_existing_sweep_appends_and_deduplicates() {
    use aida_core::db::DatabaseBackend;
    let (_tmp, root, store) = init_git_canonical_test_store();
    let _guard =
        crate::test_env::AmbientGuard::hermetic_and(&root, None, &[("AIDA_CONTAINED", None)]);
    let backend = crate::test_env::home_fixture_backend(&store)
        .unwrap()
        .with_dispenser(load_dispenser(&store).unwrap());
    let mut sweep = aida_core::Requirement::new("Existing sweep".into(), "Prior findings".into());
    sweep.tags.insert("kind:finding-sweep".into());
    let sweep = backend.add_requirement(sweep).unwrap();
    let path = crate::gate::review_verdict::verdict_path(&root, "TASK-9821");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for (findings, expected) in [
        (r#"["first", "second", "first"]"#, 2),
        (r#"["first", "second", "third"]"#, 1),
        (r#"["first", "second", "third"]"#, 0),
    ] {
        std::fs::write(
            &path,
            deferred_record_json(serde_json::from_str(findings).unwrap(), Some("review-head")),
        )
        .unwrap();
        assert_eq!(
            try_emit_nonblocking_findings_on_completion(&root, &store, "TASK-9821", "", Some(42))
                .unwrap(),
            expected
        );
        let rows = backend.list_requirements(true).unwrap();
        assert_eq!(rows.len(), 1, "open sweep must be reused");
        assert_eq!(rows[0].id, sweep.id);
        assert!(rows[0].description.starts_with("Prior findings"));
        assert!(rows[0].description.contains("PR: #42"));
        assert!(rows[0].description.contains("Reviewed sha: review-head"));
        assert_eq!(rows[0].description.matches("\nfirst\n").count(), 1);
    }
}

// trace:TASK-3749 | ai:codex
#[test]
fn task_3749_sweep_keys_preserve_source_scope_and_closed_sweeps_roll_over() {
    use aida_core::db::DatabaseBackend;
    for status in [
        aida_core::RequirementStatus::Completed,
        aida_core::RequirementStatus::Superseded,
        aida_core::RequirementStatus::Done,
    ] {
        let (_tmp, root, store) = init_git_canonical_test_store();
        let _guard =
            crate::test_env::AmbientGuard::hermetic_and(&root, None, &[("AIDA_CONTAINED", None)]);
        let backend = crate::test_env::home_fixture_backend(&store)
            .unwrap()
            .with_dispenser(load_dispenser(&store).unwrap());
        for (source, text) in [
            ("TASK-9821", "first"),
            ("TASK-9822", "second"),
            ("TASK-9822", "first"),
        ] {
            let path = crate::gate::review_verdict::verdict_path(&root, source);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, deferred_record_json(serde_json::json!([text]), None)).unwrap();
            assert_eq!(
                try_emit_nonblocking_findings_on_completion(&root, &store, source, "", Some(42))
                    .unwrap(),
                1
            );
        }
        let rows = backend.list_requirements(true).unwrap();
        assert_eq!(rows.len(), 1);
        let mut closed = rows[0].clone();
        assert!(!closed.description.contains("Reviewed sha:"));
        closed.status = status;
        backend.update_requirement(&closed).unwrap();
        // Completed sweeps retain de-duplication, but new text needs a new sweep.
        assert_eq!(
            try_emit_nonblocking_findings_on_completion(&root, &store, "TASK-9822", "", Some(42))
                .unwrap(),
            0
        );
        let path = crate::gate::review_verdict::verdict_path(&root, "TASK-9822");
        std::fs::write(
            path,
            deferred_record_json(serde_json::json!(["third"]), None),
        )
        .unwrap();
        assert_eq!(
            try_emit_nonblocking_findings_on_completion(&root, &store, "TASK-9822", "", Some(42))
                .unwrap(),
            1
        );
        assert_eq!(backend.list_requirements(true).unwrap().len(), 2);
    }
}

// trace:STORY-2698 | ai:codex
#[test]
fn story_2698_approval_delivery_is_durable_and_retry_deduplicates() {
    use crate::gate::{ledger, review_verdict as rv};
    use aida_core::db::DatabaseBackend;
    let (_tmp, project, store) = init_git_canonical_test_store();
    let _guard =
        crate::test_env::AmbientGuard::hermetic_and(&project, None, &[("AIDA_CONTAINED", None)]);
    let backend = crate::test_env::home_fixture_backend(&store)
        .unwrap()
        .with_dispenser(load_dispenser(&store).unwrap());
    backend
        .add_requirement(aida_core::Requirement::new("seed".into(), String::new()))
        .unwrap();
    let path = rv::verdict_path(&project, "STORY-9821");
    let record = serde_json::json!({"verdict":"request-changes", "findings":["deferred bug"],
        "finding_dispositions":[{"severity":"minor", "disposition":"deferred", "rationale":"safe follow-up"}],
        "recorded_by":"reviewer", "recorded_at":"2026-11-09T00:00:00Z"});
    ledger::record_verdict_at(&path, record.as_object().unwrap(), None).unwrap();
    assert!(rv::read_recorded_verdict(&project, "STORY-9821")
        .unwrap()
        .kind
        .approves());
    ledger::record_verdict_at(&path, record.as_object().unwrap(), None).unwrap();
    let all = backend.list_requirements(true).unwrap();
    let sweep: Vec<_> = all
        .iter()
        .filter(|r| r.tags.contains("kind:finding-sweep"))
        .collect();
    assert_eq!(sweep.len(), 1);
    assert_eq!(sweep[0].description.matches("deferred bug").count(), 1);
}

/// A direct approval whose follow-up findings cannot be filed records
/// nothing: no request-changes barrier stands in for it, whether or not a
/// verdict already stood, and the error names the cause.
// trace:STORY-2698 | ai:codex
// trace:BUG-2662 | ai:claude
#[test]
fn bug_2662_failed_approval_delivery_records_nothing() {
    use crate::gate::{ledger, review_verdict as rv};
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    let _guard =
        crate::test_env::AmbientGuard::hermetic_and(project, None, &[("AIDA_CONTAINED", None)]);
    let record = serde_json::json!({"verdict":"approved", "findings":["deferred bug"],
        "finding_dispositions":[{"severity":"minor", "disposition":"deferred", "rationale":"safe follow-up"}],
        "recorded_by":"reviewer", "recorded_at":"2026-11-09T00:00:00Z"});
    for key in ["STORY-9821", "PR-9821"] {
        let path = rv::verdict_path(project, key);
        let error = ledger::record_verdict_at(&path, record.as_object().unwrap(), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no canonical store"), "{error}");
        assert!(error.contains("nothing was recorded"), "{error}");
        assert!(rv::current_verdict_at(&path).is_none(), "{key}");
        // A standing refusal is left exactly as it was.
        let refusal = serde_json::json!({"verdict":"request-changes", "findings":["blocker"],
            "recorded_by":"reviewer-a", "recorded_at":"2026-11-08T00:00:00Z"});
        ledger::record_verdict_at(&path, refusal.as_object().unwrap(), None).unwrap();
        let before = ledger::project::verdict_records(project, key);
        assert_eq!(before.len(), 1);
        assert!(ledger::record_verdict_at(&path, record.as_object().unwrap(), None).is_err());
        assert_eq!(ledger::project::verdict_records(project, key), before);
    }
}

/// An approval is appended only while its spec holds no verdict record that
/// was not standing when its follow-up findings began to be filed.
// trace:BUG-2662 | ai:claude
#[test]
fn bug_2662_approval_is_refused_when_another_review_lands_during_delivery() {
    use crate::gate::ledger::{self, event, store, GateSpec};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _guard =
        crate::test_env::AmbientGuard::hermetic_and(root, None, &[("AIDA_CONTAINED", None)]);
    let change = ledger::ChangeKey::pr(8);
    let body = |verdict: &str, by: &str| {
        serde_json::json!({"verdict":verdict,"recorded_by":by})
            .as_object()
            .unwrap()
            .clone()
    };
    let evt = |record: &crate::gate::review_verdict::JsonObj| {
        event::verdict_event(root, change, record, None)
    };
    store::append(
        root,
        change,
        vec![evt(&body("request-changes", "reviewer-a"))],
    )
    .unwrap();
    let standing = store::standing(root, Some(change), None);
    // Nothing joined the standing records: appended once, then unchanged.
    let approval = body("approved", "reviewer-a");
    for expected in [ledger::Delivered::Appended, ledger::Delivered::Unchanged] {
        assert_eq!(
            store::append_one_standing(root, change, evt(&approval), &standing).unwrap(),
            expected
        );
    }
    // A review recorded after the snapshot refuses the approval.
    let standing = store::standing(root, Some(change), None);
    store::append(root, change, vec![evt(&body("rejected", "reviewer-b"))]).unwrap();
    let late = body("approved", "reviewer-c");
    let error = store::append_one_standing(root, change, evt(&late), &standing)
        .unwrap_err()
        .to_string();
    assert!(error.contains("nothing was recorded"), "{error}");
    assert_eq!(
        ledger::project::newest_verdict(root, "PR-8").unwrap()["verdict"],
        "rejected"
    );
    // The same holds on a spec's unbound ledger.
    let spec = GateSpec::resolve(root, "STORY-9830");
    store::append_unbound(root, &spec, &body("request-changes", "reviewer-a")).unwrap();
    let standing = store::standing(root, None, Some(&spec.label));
    store::append_unbound(root, &spec, &body("rejected", "reviewer-b")).unwrap();
    assert!(store::append_unbound_standing(root, &spec, &late, &standing).is_err());
    let standing = store::standing(root, None, Some(&spec.label));
    assert_eq!(
        store::append_unbound_standing(root, &spec, &late, &standing).unwrap(),
        ledger::Delivered::Appended
    );
}

// trace:STORY-2698 | ai:codex
#[test]
fn story_2698_collected_approval_delivers_and_retries() {
    use crate::gate::{ledger, review_verdict as rv};
    use aida_core::db::DatabaseBackend;
    let (_tmp, project, store) = init_git_canonical_test_store();
    let _guard =
        crate::test_env::AmbientGuard::hermetic_and(&project, None, &[("AIDA_CONTAINED", None)]);
    let backend = crate::test_env::home_fixture_backend(&store)
        .unwrap()
        .with_dispenser(load_dispenser(&store).unwrap());
    backend
        .add_requirement(aida_core::Requirement::new("seed".into(), String::new()))
        .unwrap();
    let record = serde_json::json!({"verdict":"request-changes", "findings":["collected follow-up"],
        "finding_dispositions":[{"severity":"major", "disposition":"deferred", "rationale":"advisor accepts follow-up"}],
        "inherited_findings":true, "recorded_by":"reviewer", "recorded_at":"2026-11-09T00:00:00Z"});
    for _ in 0..2 {
        ledger::record_collected_verdict(
            &project,
            ledger::ChangeKey::pr(7),
            record.as_object().unwrap(),
            &[],
        )
        .unwrap();
    }
    assert!(rv::read_recorded_verdict(&project, "PR-7")
        .unwrap()
        .kind
        .approves());
    let all = backend.list_requirements(true).unwrap();
    let sweep: Vec<_> = all
        .iter()
        .filter(|r| r.tags.contains("kind:finding-sweep"))
        .collect();
    assert_eq!(sweep.len(), 1);
    assert_eq!(
        sweep[0].description.matches("collected follow-up").count(),
        1
    );
}

// trace:STORY-2698 | ai:codex
#[test]
fn story_2698_delivery_cannot_overwrite_a_newer_review() {
    use crate::gate::ledger::{self, event, store};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _guard =
        crate::test_env::AmbientGuard::hermetic_and(root, None, &[("AIDA_CONTAINED", None)]);
    let change = ledger::ChangeKey::pr(7);
    let pending = serde_json::json!({"verdict":"request-changes","sweep_pending":true,"recorded_by":"reviewer-a"});
    let delivered = serde_json::json!({"verdict":"approved","recorded_by":"reviewer-a"});
    let newer = serde_json::json!({"verdict":"rejected","recorded_by":"reviewer-b"});
    let evt = |body: &serde_json::Value| {
        event::verdict_event(root, change, body.as_object().unwrap(), None)
    };
    store::append(root, change, vec![evt(&pending)]).unwrap();
    store::append(root, change, vec![evt(&newer)]).unwrap();
    assert!(
        store::append_delivered(root, change, vec![evt(&delivered)], &[evt(&pending)]).is_err()
    );
    assert_eq!(
        ledger::project::newest_verdict(root, "PR-7").unwrap()["verdict"],
        "rejected"
    );

    let spec = ledger::GateSpec::resolve(root, "STORY-9821");
    store::append_unbound(root, &spec, pending.as_object().unwrap()).unwrap();
    store::append_unbound(root, &spec, newer.as_object().unwrap()).unwrap();
    assert!(store::append_unbound_expected(
        root,
        &spec,
        delivered.as_object().unwrap(),
        Some(pending.as_object().unwrap())
    )
    .is_err());
    assert_eq!(
        ledger::project::newest_verdict(root, "STORY-9821").unwrap()["verdict"],
        "rejected"
    );
}

// trace:TASK-3814 | ai:codex
#[test]
fn task_3814_tier_policy_files_each_followup_and_keeps_p1_blocking() {
    use crate::gate::{ledger, review_verdict as rv};
    use aida_core::db::DatabaseBackend;
    let (_tmp, root, store) = init_git_canonical_test_store();
    let _guard =
        crate::test_env::AmbientGuard::hermetic_and(&root, None, &[("AIDA_CONTAINED", None)]);
    let backend = crate::test_env::home_fixture_backend(&store)
        .unwrap()
        .with_dispenser(load_dispenser(&store).unwrap());
    let mut parent = aida_core::Requirement::new("reviewed spec".into(), String::new());
    parent.spec_id = Some("TASK-9821".into());
    let parent_id = parent.id;
    backend.add_requirement(parent).unwrap();
    let source = backend
        .get_requirement(&parent_id)
        .unwrap()
        .unwrap()
        .spec_id
        .unwrap();
    let config = root.join(".aida/config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(
        "\n[review]\nblocking_tiers = [\"p1\", \"data-loss\", \"security\", \"acceptance\"]\n",
    );
    std::fs::write(config, text).unwrap();
    // trace:STORY-2913 | ai:claude — the policy is read from the default branch.
    crate::review_blocking::commit_config_for_test(&root);
    let findings: Vec<String> = [("p2", "first followup"), ("p3", "second followup")]
        .iter()
        .map(|(tier, text)| {
            serde_json::json!({"text":text,"blocking_tier":tier,"severity":"minor"}).to_string()
        })
        .collect();
    let path = rv::verdict_path(&root, &source);
    let record = rv::build_verdict_object(
        &root,
        &path,
        Some("request-changes"),
        None,
        None,
        None,
        &findings,
        "reviewer",
    )
    .unwrap();
    assert_eq!(record["verdict"], "approved");
    ledger::record_verdict_at(&path, &record, None).unwrap();
    ledger::record_verdict_at(&path, &record, None).unwrap();
    let rows = backend.list_requirements(true).unwrap();
    let followups: Vec<_> = rows.iter().filter(|r| r.id != parent_id).collect();
    assert_eq!(followups.len(), 2);
    for task in followups {
        assert_eq!(task.status, aida_core::RequirementStatus::Draft);
        assert!(task
            .relationships
            .iter()
            .any(|r| r.target_id == parent_id
                && r.rel_type == aida_core::RelationshipType::References));
    }
    assert!(rv::read_recorded_verdict(&root, &source)
        .unwrap()
        .kind
        .approves());
    let mixed = vec![
        serde_json::json!({"text":"third followup","blocking_tier":"p2"}).to_string(),
        serde_json::json!({"text":"critical defect","blocking_tier":"p1"}).to_string(),
    ];
    let record = rv::build_verdict_object(
        &root,
        &path,
        Some("request-changes"),
        None,
        None,
        None,
        &mixed,
        "reviewer",
    )
    .unwrap();
    ledger::record_verdict_at(&path, &record, None).unwrap();
    assert_eq!(
        rv::read_recorded_verdict(&root, &source).unwrap().kind,
        VerdictKind::RequestChanges
    );
    assert_eq!(backend.list_requirements(true).unwrap().len(), 4);

    // The host collector receives both an overall PR record and spec records.
    // The overall record must link to the covered spec and deduplicate its copy.
    let collected =
        vec![serde_json::json!({"text":"collected followup","blocking_tier":"p2"}).to_string()];
    let record = rv::build_verdict_object(
        &root,
        &path,
        Some("request-changes"),
        None,
        None,
        None,
        &collected,
        "reviewer",
    )
    .unwrap();
    let spec = crate::gate::canon::GateSpec::resolve(&root, &source);
    for _ in 0..2 {
        ledger::record_collected_verdict(
            &root,
            ledger::ChangeKey::pr(42),
            &record,
            &[(spec.clone(), record.clone())],
        )
        .unwrap();
    }
    assert_eq!(backend.list_requirements(true).unwrap().len(), 5);
    assert!(rv::read_recorded_verdict(&root, "PR-42")
        .unwrap()
        .kind
        .approves());
}

// trace:TASK-3814 | ai:claude — a review recorded against the agreed ID (which
// differs from the native spec_id, as TASK-3814 does) files linked follow-ups,
// and retries through every alias deduplicate by the source's identity.
#[test]
fn task_3814_agreed_id_review_files_followups_and_aliases_deduplicate() {
    use crate::gate::{ledger, review_verdict as rv};
    use aida_core::db::DatabaseBackend;
    let (_tmp, root, store) = init_git_canonical_test_store();
    let _guard =
        crate::test_env::AmbientGuard::hermetic_and(&root, None, &[("AIDA_CONTAINED", None)]);
    let backend = crate::test_env::home_fixture_backend(&store)
        .unwrap()
        .with_dispenser(load_dispenser(&store).unwrap());
    let mut parent = aida_core::Requirement::new("reviewed spec".into(), String::new());
    parent.spec_id = Some("TASK-b6b1278890774ef993d2ebc327862470-1097".into());
    parent.agreed_id = Some("TASK-97777".into());
    let parent_id = parent.id;
    backend.add_requirement(parent).unwrap();
    let stored = backend.get_requirement(&parent_id).unwrap().unwrap();
    assert_eq!(stored.agreed_id.as_deref(), Some("TASK-97777"));
    assert_ne!(stored.spec_id.as_deref(), Some("TASK-97777"));
    let config = root.join(".aida/config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(
        "\n[review]\nblocking_tiers = [\"p1\", \"data-loss\", \"security\", \"acceptance\"]\n",
    );
    std::fs::write(config, text).unwrap();
    // trace:STORY-2913 | ai:claude — the policy is read from the default branch.
    crate::review_blocking::commit_config_for_test(&root);
    let findings: Vec<String> = [("p2", "agreed followup one"), ("p3", "agreed followup two")]
        .iter()
        .map(|(tier, text)| {
            serde_json::json!({"text":text,"blocking_tier":tier,"severity":"minor"}).to_string()
        })
        .collect();
    let build = |path: &std::path::Path| {
        rv::build_verdict_object(
            &root,
            path,
            Some("request-changes"),
            None,
            None,
            None,
            &findings,
            "reviewer",
        )
        .unwrap()
    };
    let linked = || {
        backend
            .list_requirements(true)
            .unwrap()
            .into_iter()
            .filter(|r| {
                r.relationships.iter().any(|rel| {
                    rel.target_id == parent_id
                        && rel.rel_type == aida_core::RelationshipType::References
                })
            })
            .count()
    };

    // Direct recording under the agreed ID approves and files both follow-ups.
    let agreed_path = rv::verdict_path(&root, "TASK-97777");
    let record = build(&agreed_path);
    assert_eq!(record["verdict"], "approved");
    ledger::record_verdict_at(&agreed_path, &record, None).unwrap();
    assert_eq!(linked(), 2);
    assert!(rv::read_recorded_verdict(&root, "TASK-97777")
        .unwrap()
        .kind
        .approves());

    // Retries through the agreed ID, a lowercase alias, the native ID and the
    // UUID deduplicate against the same source identity.
    for alias in [
        "TASK-97777".to_string(),
        "task-97777".to_string(),
        "TASK-b6b1278890774ef993d2ebc327862470-1097".to_string(),
        parent_id.to_string(),
    ] {
        let path = rv::verdict_path(&root, &alias);
        ledger::record_verdict_at(&path, &build(&path), None).unwrap();
        assert_eq!(linked(), 2, "direct retry via {alias}");
    }

    // The host collector hands off a PR record plus a spec record labelled by
    // the agreed ID; it files only new findings and deduplicates via aliases.
    let collected = vec![
        serde_json::json!({"text":"collected agreed followup","blocking_tier":"p2"}).to_string(),
    ];
    let pr_path = rv::verdict_path(&root, "PR-43");
    let record = rv::build_verdict_object(
        &root,
        &pr_path,
        Some("request-changes"),
        None,
        None,
        None,
        &collected,
        "reviewer",
    )
    .unwrap();
    for label in [
        "TASK-97777",
        "task-97777",
        "TASK-b6b1278890774ef993d2ebc327862470-1097",
    ] {
        let spec = crate::gate::canon::GateSpec::resolve(&root, label);
        ledger::record_collected_verdict(
            &root,
            ledger::ChangeKey::pr(43),
            &record,
            &[(spec, record.clone())],
        )
        .unwrap();
        assert_eq!(linked(), 3, "host handoff via {label}");
    }
    assert!(rv::read_recorded_verdict(&root, "PR-43")
        .unwrap()
        .kind
        .approves());
}

// trace:TASK-3814 | ai:claude — `aida review record --finding-class` (the
// positional channel) joins the policy before the kind/authority decision in
// the production handler: a P2 gate finding classified untested-path, in
// canonical or alias spelling, cannot be explicitly approved and is a refusal
// (which needs `--pr`); without that class the same finding is approvable.
#[test]
fn task_3814_positional_finding_class_blocks_gate_untested_path_end_to_end() {
    use crate::gate::review_verdict as rv;
    let record = |root: &std::path::Path, head: &str, verdict: &str, classes: &[&str]| {
        let finding = serde_json::json!({
            "text": "the refusal path in the gate has no regression",
            "severity": "minor",
            "blocking_tier": "p2",
            "file": "aida-cli-lib/src/gate/ledger/mod.rs",
        })
        .to_string();
        let classes: Vec<String> = classes.iter().map(|c| c.to_string()).collect();
        let pinned = head.to_string();
        crate::handle_review_record_with_head_at(
            root.to_path_buf(),
            "BUG-9900",
            verdict,
            Some(head),
            None,
            None,
            &[finding],
            &classes,
            None,
            false,
            move |_, _| Ok(pinned.clone()),
        )
        .map_err(|e| format!("{e:#}"))
    };
    let setup = || {
        let tmp = tempfile::tempdir().unwrap();
        let head = crate::test_git::commit_fixture(tmp.path());
        let config = tmp.path().join(".aida/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let mut text = std::fs::read_to_string(&config).unwrap_or_default();
        text.push_str(
            "\n[review]\nblocking_tiers = [\"p1\", \"data-loss\", \"security\", \"acceptance\"]\n",
        );
        std::fs::write(&config, text).unwrap();
        // trace:STORY-2913 | ai:claude — the policy is read from the default branch.
        crate::review_blocking::commit_config_for_test(tmp.path());
        (tmp, head)
    };
    for class in [
        "untested-path",
        "missing-test",
        "Untested_Path",
        "mutation-kill",
    ] {
        let (tmp, head) = setup();
        let root = tmp.path();
        let _ambient =
            crate::test_env::AmbientGuard::hermetic_and(root, None, &[("AIDA_CONTAINED", None)]);
        let err = record(root, &head, "approved", &[class]).unwrap_err();
        assert!(
            err.contains(rv::EXPLICIT_APPROVAL_REFUSAL),
            "{class}: explicit approval must be refused: {err}"
        );
        let err = record(root, &head, "request-changes", &[class]).unwrap_err();
        assert!(
            err.contains("a refusing verdict must name `--pr <N>`"),
            "{class}: must remain a refusal: {err}"
        );
        assert!(
            rv::read_recorded_verdict(root, "BUG-9900").is_none(),
            "{class}"
        );
    }
    // Paired control: no class, or an ordinary class, is a deferred P2 that
    // the same handler does not refuse at the kind decision.
    for classes in [&[][..], &["perf"][..]] {
        let (tmp, head) = setup();
        let root = tmp.path();
        let _ambient =
            crate::test_env::AmbientGuard::hermetic_and(root, None, &[("AIDA_CONTAINED", None)]);
        if let Err(err) = record(root, &head, "approved", classes) {
            assert!(
                !err.contains(rv::EXPLICIT_APPROVAL_REFUSAL)
                    && !err.contains("a refusing verdict must name"),
                "{classes:?}: an ordinary P2 must be approvable: {err}"
            );
        }
    }
}

// trace:TASK-3814 | ai:claude — the reverse merge direction through the
// production handler: an embedded untested-path/mutation-kill class on an
// ordinary (non-sensitive) path plus a positional security or
// authority-bypass class must still block on both the explicit-approval and
// the request-changes routes. Paired control: the same embedded class without
// the positional security class defers and is approvable.
#[test]
fn task_3814_positional_security_outranks_embedded_path_scoped_class_end_to_end() {
    use crate::gate::review_verdict as rv;
    let record =
        |root: &std::path::Path, head: &str, verdict: &str, embedded: &str, classes: &[&str]| {
            let finding = serde_json::json!({
                "text": "security defect",
                "severity": "minor",
                "blocking_tier": "p2",
                "class": embedded,
                "file": "aida-cli-lib/src/report.rs",
            })
            .to_string();
            let classes: Vec<String> = classes.iter().map(|c| c.to_string()).collect();
            let pinned = head.to_string();
            crate::handle_review_record_with_head_at(
                root.to_path_buf(),
                "BUG-9900",
                verdict,
                Some(head),
                None,
                None,
                &[finding],
                &classes,
                None,
                false,
                move |_, _| Ok(pinned.clone()),
            )
            .map_err(|e| format!("{e:#}"))
        };
    let setup = || {
        let tmp = tempfile::tempdir().unwrap();
        let head = crate::test_git::commit_fixture(tmp.path());
        let config = tmp.path().join(".aida/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let mut text = std::fs::read_to_string(&config).unwrap_or_default();
        text.push_str(
            "\n[review]\nblocking_tiers = [\"p1\", \"data-loss\", \"security\", \"acceptance\"]\n",
        );
        std::fs::write(&config, text).unwrap();
        // trace:STORY-2913 | ai:claude — the policy is read from the default branch.
        crate::review_blocking::commit_config_for_test(tmp.path());
        (tmp, head)
    };
    for embedded in ["untested-path", "mutation-kill"] {
        for positional in ["security", "authority-bypass"] {
            let (tmp, head) = setup();
            let root = tmp.path();
            let _ambient = crate::test_env::AmbientGuard::hermetic_and(
                root,
                None,
                &[("AIDA_CONTAINED", None)],
            );
            let err = record(root, &head, "approved", embedded, &[positional]).unwrap_err();
            assert!(
                err.contains(rv::EXPLICIT_APPROVAL_REFUSAL),
                "{embedded}+{positional}: explicit approval must be refused: {err}"
            );
            let err = record(root, &head, "request-changes", embedded, &[positional]).unwrap_err();
            assert!(
                err.contains("a refusing verdict must name `--pr <N>`"),
                "{embedded}+{positional}: must remain a refusal: {err}"
            );
            assert!(
                rv::read_recorded_verdict(root, "BUG-9900").is_none(),
                "{embedded}+{positional}"
            );
        }
        // Paired control: without the positional security class the
        // ordinary-path exception defers and is not refused at the kind decision.
        let (tmp, head) = setup();
        let root = tmp.path();
        let _ambient =
            crate::test_env::AmbientGuard::hermetic_and(root, None, &[("AIDA_CONTAINED", None)]);
        if let Err(err) = record(root, &head, "approved", embedded, &[]) {
            assert!(
                !err.contains(rv::EXPLICIT_APPROVAL_REFUSAL)
                    && !err.contains("a refusing verdict must name"),
                "{embedded}: an ordinary-path P2 must be approvable: {err}"
            );
        }
    }
}

// trace:STORY-2913 | ai:claude — AC3/AC6: under a custom list the findings
// outside it are approved and auto-filed as linked follow-ups, deduplicated
// by finding hash across retries; a listed class still refuses.
// trace:STORY-2913.ac0add0e | ai:claude
#[test]
fn story_2913_custom_list_auto_files_followups() {
    use crate::gate::{ledger, review_verdict as rv};
    use aida_core::db::DatabaseBackend;
    let (_tmp, root, store) = init_git_canonical_test_store();
    let _guard =
        crate::test_env::AmbientGuard::hermetic_and(&root, None, &[("AIDA_CONTAINED", None)]);
    let backend = crate::test_env::home_fixture_backend(&store)
        .unwrap()
        .with_dispenser(load_dispenser(&store).unwrap());
    let mut parent = aida_core::Requirement::new("reviewed spec".into(), String::new());
    parent.spec_id = Some("TASK-9822".into());
    let parent_id = parent.id;
    backend.add_requirement(parent).unwrap();
    let source = backend
        .get_requirement(&parent_id)
        .unwrap()
        .unwrap()
        .spec_id
        .unwrap();
    let config = root.join(".aida/config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("\n[review]\nblocking_tiers = [\"p1\", \"style\"]\n");
    std::fs::write(&config, text).unwrap();
    crate::review_blocking::commit_config_for_test(&root);
    // p2 and a p3 test gap on ordinary code are outside the list.
    let findings = vec![
        serde_json::json!({"text":"slow loop","blocking_tier":"p2"}).to_string(),
        serde_json::json!({"text":"no regression test","blocking_tier":"p3","class":"test-gap",
            "file":"aida-cli-lib/src/report.rs"})
        .to_string(),
    ];
    let path = rv::verdict_path(&root, &source);
    let record = rv::build_verdict_object(
        &root,
        &path,
        Some("request-changes"),
        None,
        None,
        None,
        &findings,
        "reviewer",
    )
    .unwrap();
    assert_eq!(record["verdict"], "approved");
    ledger::record_verdict_at(&path, &record, None).unwrap();
    ledger::record_verdict_at(&path, &record, None).unwrap();
    let rows = backend.list_requirements(true).unwrap();
    let followups: Vec<_> = rows.iter().filter(|r| r.id != parent_id).collect();
    assert_eq!(followups.len(), 2, "one per finding, retry deduplicated");
    for task in &followups {
        assert!(task.tags.iter().any(|t| t.starts_with("finding-hash:")));
        assert!(task
            .relationships
            .iter()
            .any(|r| r.target_id == parent_id
                && r.rel_type == aida_core::RelationshipType::References));
    }
    // A listed class (style) blocks under the same list.
    let styled = vec![
        serde_json::json!({"text":"inconsistent naming","blocking_tier":"p3","class":"style"})
            .to_string(),
    ];
    let record = rv::build_verdict_object(
        &root,
        &path,
        Some("request-changes"),
        None,
        None,
        None,
        &styled,
        "reviewer",
    )
    .unwrap();
    assert_eq!(record["verdict"], "request-changes");
}

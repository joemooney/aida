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

// trace:STORY-2698 | ai:codex
#[test]
fn story_2698_failed_delivery_leaves_pending_refusal() {
    use crate::gate::{ledger, review_verdict as rv};
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    let _guard =
        crate::test_env::AmbientGuard::hermetic_and(project, None, &[("AIDA_CONTAINED", None)]);
    let path = rv::verdict_path(project, "STORY-9821");
    let record = serde_json::json!({"verdict":"approved", "findings":["deferred bug"],
        "finding_dispositions":[{"severity":"minor", "disposition":"deferred", "rationale":"safe follow-up"}],
        "recorded_by":"reviewer", "recorded_at":"2026-11-09T00:00:00Z"});
    assert!(ledger::record_verdict_at(&path, record.as_object().unwrap(), None).is_err());
    let current = rv::current_verdict_at(&path).expect("pending payload is durable");
    assert!(current.body.contains("sweep_pending"));
    assert_eq!(
        rv::parse_recorded_verdict(&current.body).unwrap().kind,
        VerdictKind::RequestChanges
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

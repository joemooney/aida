use super::*;
use crate::review_verdict::{RecordedVerdict, VerdictKind};
use std::collections::HashSet;

fn approved_verdict(findings: &[&str]) -> RecordedVerdict {
    RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".to_string(),
        findings: findings.iter().map(|s| s.to_string()).collect(),
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
    let out = std::process::Command::new("git")
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

    (tmp, project_root, store_dir)
}

/// BUG-1506: `try_emit_nonblocking_findings_on_completion` must write each
/// successor finding through the SAME targeted, single-object
/// `DatabaseBackend::add_requirement` path `aida add` itself uses on the
/// git-canonical store — never a full-store load-then-save (the old
/// `CachedGitBackend::update_atomically` shape this replaced), which would
/// silently drop a spec written concurrently by another session in between.
/// This reproduces that scenario directly: a spec is added to the store
/// through its own backend handle (standing in for a concurrent `aida add`)
/// BEFORE the emit call, and must still be present after it, alongside both
/// new findings landing via their own targeted `add TASK-…` commits.
// trace:STORY-1421 trace:BUG-1506 | ai:claude
#[test]
fn emit_writes_targeted_commits_and_preserves_concurrent_spec() {
    use aida_core::db::DatabaseBackend;

    let (_tmp, project_root, store_dir) = init_git_canonical_test_store();

    // Simulate a concurrent write: another session's `aida add` lands a
    // brand-new spec in the store that this call never loads a full
    // in-memory snapshot of.
    let dispenser = load_dispenser(&store_dir).unwrap();
    let concurrent_backend = aida_core::GitBackend::new(&store_dir)
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
    let verdict_path = crate::review_verdict::verdict_path(&project_root, "STORY-9821");
    std::fs::create_dir_all(verdict_path.parent().unwrap()).unwrap();
    std::fs::write(
        &verdict_path,
        r#"{"verdict":"approved","findings":["F1: finding text","F2: finding text"],"reviewed_sha":"deadbeefcafe"}"#,
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
    let check_backend = aida_core::GitBackend::new(&store_dir)
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

    // Both new findings landed, tagged back to the source spec.
    let all = check_backend.list_requirements(true).unwrap();
    let carried: Vec<_> = all
        .iter()
        .filter(|r| r.tags.iter().any(|t| t == "carried-from:STORY-9821"))
        .collect();
    assert_eq!(carried.len(), 2);
    for req in &carried {
        assert!(req.tags.iter().any(|t| t.starts_with("finding-hash:")));
        assert!(req.tags.iter().any(|t| t == "from-review:PR-7"));
    }

    // One targeted `add SPEC-ID` commit per new finding, no bulk chore
    // commit — mirrors the BUG-1506 auto-bump assertion.
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

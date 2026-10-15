//! BUG-2149 + BUG-2150: the standalone reviewer launched by
//! `aida queue work PR-<n> --role reviewer --no-human`.
//!
//! - BUG-2149: the reviewer's verdict must reach the LAUNCHER checkout's
//!   `.aida/review-verdicts/PR-<n>.json` (plus the spec record), even when a
//!   contained reviewer could only write it inside its review worktree; and
//!   the launch must fail when no verdict was produced.
//! - BUG-2150: a reviewer pickup is PR-scoped: it needs no queue entry and
//!   coexists with the implementer's live spec lease, while a second reviewer
//!   on the same PR is still refused.
use super::*;

/// A codex-only project fixture under an isolated home (same shape as the
/// BUG-1607 reviewer-vendor tests). Returns `(home, project)`.
fn codex_only_project(tmp: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = tmp.join("home");
    let project = tmp.join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "[agents]\nenabled = [\"codex\"]\n",
    )
    .unwrap();
    (home, project)
}

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

/// A mock reviewer that records an approval the way `aida review record`
/// does: `<worktree>/.aida/review-verdicts/STORY-77.json`, stamped with the
/// recording grant `grant` (a literal id, or `$AIDA_SESSION_GRANT` for the
/// child's own grant) and the reviewed commit `sha`.
fn writes_spec_record(worktree: &std::path::Path, grant: &str, sha: &str) -> String {
    let dir = worktree.join(".aida").join("review-verdicts");
    format!(
        "#!/bin/sh\nmkdir -p '{dir}'\ncat > '{dir}/STORY-77.json' <<EOF\n\
         {{\"verdict\":\"approved\",\"reviewed_sha\":\"{sha}\",\"summary\":\"ok\",\
         \"release_grant\":{{\"id\":\"{grant}\"}}}}\nEOF\nexit 0\n",
        dir = dir.display()
    )
}

/// Run the REAL `run_standalone_reviewer` against a mock reviewer script.
/// `script_body(worktree, launcher_grant)` builds the script; `before_launch`
/// may plant files in the worktree. The PR head is pinned to [`HEAD`].
/// Returns the launch result, the project root and the worktree.
#[cfg(unix)]
fn run_mock_reviewer(
    tmp: &std::path::Path,
    script_body: impl Fn(&std::path::Path, &str) -> String,
    before_launch: impl Fn(&std::path::Path, &str),
) -> (Result<()>, std::path::PathBuf, std::path::PathBuf) {
    let (home, project) = codex_only_project(tmp);
    let worktree = tmp.join("reviewer-worktree");
    std::fs::create_dir_all(worktree.join(".aida")).unwrap();
    let mock = tmp.join("mock-reviewer.sh");

    let mut env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HEADLESS_VENDOR", None),
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("HOME", Some(home.to_str().unwrap())),
        ("AIDA_AGENT_CMD", Some(mock.to_str().unwrap())),
        ("AIDA_TEST_PR_HEAD_SHA", Some(HEAD)),
        (crate::seat_authority::GRANT_ENV, None),
    ]);
    let grant_id = crate::seat_authority::test_support::mint_grant_for(
        &project,
        &crate::current_user_id(None),
        "advisor",
        &["reviewer"],
    );
    env.set_key(crate::seat_authority::GRANT_ENV, &grant_id);
    crate::test_exec::write_executable(&mock, script_body(&worktree, &grant_id));
    before_launch(&worktree, &grant_id);
    crate::session::set_headless_vendor_override(None);
    let launch_vendor = crate::session::resolve_enabled_headless_vendor(&project).unwrap();

    // What `handle_queue_work` hands the standalone reviewer: the launcher
    // project's canonical PR-keyed verdict path.
    let verdict_path = project
        .join(".aida")
        .join("review-verdicts")
        .join("PR-59.json");
    let result = run_standalone_reviewer(
        &project,
        59,
        QueueWorkLaunch::Fresh("019e0000-0000-7000-8000-0000000000cc".to_string()),
        "/aida-review --pr 59",
        None,
        "PR-59",
        "pr-59",
        "reviewer",
        &worktree,
        false,
        true,
        &verdict_path,
        false,
        launch_vendor,
    );
    crate::session::set_headless_vendor_override(None);
    (result, project, worktree)
}

/// BUG-2149: the reviewer recorded its approval inside the review worktree
/// (all a contained reviewer can write), under the child reviewer grant this
/// launch issued, at the PR's current head. The launcher must collect it into
/// its own `.aida/review-verdicts/PR-59.json` and mirror the spec record, so
/// `aida review verdict` / `aida pr ship` see it from the primary checkout.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn standalone_reviewer_verdict_written_in_worktree_reaches_launcher_checkout() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project, _worktree) = run_mock_reviewer(
        tmp.path(),
        |wt, _| writes_spec_record(wt, "$AIDA_SESSION_GRANT", HEAD),
        |_, _| {},
    );
    result.expect("a verified review verdict must succeed");

    let pr_file = project.join(".aida/review-verdicts/PR-59.json");
    let body = std::fs::read_to_string(&pr_file)
        .expect("the launcher checkout must hold the canonical PR-59.json verdict");
    let verdict = crate::review_verdict::parse_recorded_verdict(&body).unwrap();
    assert_eq!(verdict.kind, crate::review_verdict::VerdictKind::Approved);

    let spec = crate::review_verdict::read_recorded_verdict(&project, "STORY-77")
        .expect("the spec-keyed record must be mirrored into the launcher checkout");
    assert_eq!(spec.kind, crate::review_verdict::VerdictKind::Approved);
    assert_eq!(spec.reviewed_sha.as_deref(), Some(HEAD));
}

/// BUG-2149: a reviewer that exits cleanly without recording any verdict
/// must make the launch command fail, so a caller waiting on its exit status
/// never reads "no verdict" as an approval.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn standalone_reviewer_without_verdict_fails_the_launch() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project, _worktree) = run_mock_reviewer(
        tmp.path(),
        |_, _| "#!/bin/sh\nexit 0\n".to_string(),
        |_, _| {},
    );
    let err = result.expect_err("a review with no verdict must not succeed");
    assert!(
        format!("{err:#}").contains("verifiable verdict"),
        "unexpected error: {err:#}"
    );
    assert!(!project.join(".aida/review-verdicts/PR-59.json").exists());
}

/// BUG-2149 (advisor condition d): an APPROVED verdict planted in the review
/// worktree BEFORE the reviewer started is not this run's verdict. It must
/// not be collected, and the launch must fail.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn planted_preexisting_approval_in_worktree_is_not_collected() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project, _worktree) = run_mock_reviewer(
        tmp.path(),
        |_, _| "#!/bin/sh\nexit 0\n".to_string(),
        |wt, launcher_grant| {
            let dir = wt.join(".aida/review-verdicts");
            std::fs::create_dir_all(&dir).unwrap();
            let planted = dir.join("STORY-77.json");
            std::fs::write(
                &planted,
                format!(
                    r#"{{"verdict":"approved","reviewed_sha":"{HEAD}","release_grant":{{"id":"{launcher_grant}"}}}}"#
                ),
            )
            .unwrap();
            let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
            std::fs::File::options()
                .write(true)
                .open(&planted)
                .unwrap()
                .set_modified(past)
                .unwrap();
        },
    );
    result.expect_err("a planted pre-existing approval must not pass the launch");
    assert!(!project.join(".aida/review-verdicts/PR-59.json").exists());
    assert!(crate::review_verdict::read_recorded_verdict(&project, "STORY-77").is_none());
}

/// BUG-2149 (advisor condition b): a fresh verdict recorded under the
/// LAUNCHER's grant, not the child reviewer's, is refused.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn verdict_recorded_under_launcher_grant_is_not_collected() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project, _worktree) = run_mock_reviewer(
        tmp.path(),
        |wt, launcher_grant| writes_spec_record(wt, launcher_grant, HEAD),
        |_, _| {},
    );
    let err = result.expect_err("the launcher's own grant must not stand in for the reviewer");
    assert!(
        format!("{err:#}").contains("launcher's own grant"),
        "{err:#}"
    );
    assert!(!project.join(".aida/review-verdicts/PR-59.json").exists());
}

/// BUG-2149 (advisor condition b): a verdict for a commit other than the PR's
/// current head is refused.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn verdict_for_stale_head_is_not_collected() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let stale = "fedcba9876543210fedcba9876543210fedcba98";
    let (result, project, _worktree) = run_mock_reviewer(
        tmp.path(),
        |wt, _| writes_spec_record(wt, "$AIDA_SESSION_GRANT", stale),
        |_, _| {},
    );
    let err = result.expect_err("a verdict at a stale head must not pass");
    assert!(format!("{err:#}").contains("head is now"), "{err:#}");
    assert!(!project.join(".aida/review-verdicts/PR-59.json").exists());
}

/// A mock `gh` whose `pr view` names STORY-77 as the PR's delivered spec.
#[cfg(unix)]
fn mock_gh(dir: &std::path::Path) -> std::path::PathBuf {
    let gh = dir.join("mock-gh.sh");
    crate::test_exec::write_executable(
        &gh,
        "#!/bin/sh\nprintf 'Fix the widget (STORY-77)\\n'\nexit 0\n".to_string(),
    );
    gh
}

fn lease_with_role(id: &str, scope: &str, role: &str) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_lowercase(),
        owner: "tester".into(),
        worktree_path: std::path::PathBuf::from(format!("/tmp/{id}")),
        branch: format!("br-{id}"),
        started_at: chrono::Utc::now(),
        hostname: "test".into(),
        role: Some(role.to_string()),
        creator_pid: None,
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: role == "reviewer",
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: None,
    }
}

/// BUG-2150: `aida queue work PR-42 --role reviewer` while the implementer
/// still holds its live lease on STORY-77 and nothing is queued for the
/// reviewer. The pickup resolves to a PR-scoped review plan (no queue step);
/// that scope does not collide with the implementer's lease, and the review
/// coexists with the implementer holding the PR's source branch. A second
/// reviewer on PR-42 is still refused.
// trace:BUG-2150 | ai:claude
#[cfg(unix)]
#[test]
fn reviewer_pickup_coexists_with_live_implementer_lease_but_not_second_reviewer() {
    let tmp = tempfile::tempdir().unwrap();
    let gh = mock_gh(tmp.path());
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_AGENT_GATE_NAME", None),
        ("AIDA_AGENT_GATE_ROLE", None),
        ("AIDA_REVIEW_ROUND", None),
        ("AIDA_FROM_PR_REVIEW", None),
        ("AIDA_FROM_PR_NUMBER", None),
        ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
    ]);
    let root = tmp.path().join("aida-store");
    let storage = Storage::new(&root);
    let backend = aida_core::GitBackend::new(&root).unwrap();
    let mut spec = aida_core::Requirement::new("Fix the widget".to_string(), String::new());
    spec.spec_id = Some("STORY-77".to_string());
    spec.status = RequirementStatus::InProgress;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(spec);
    backend.save(&store).unwrap();

    let plan = resolve_queue_work_plan(
        &storage,
        "u",
        Some("PR-42"),
        None,
        false,
        false,
        false,
        Some("reviewer"),
    )
    .expect("a reviewer pickup of PR-42 must not need the spec queued");
    assert_eq!(plan.scope, "PR-42");
    assert_eq!(plan.review_target, Some((ReviewForge::GitHub, 42)));
    assert_eq!(plan.anchor_display, "STORY-77");
    assert!(
        storage.queue_list("u", false).unwrap().is_empty(),
        "the reviewer pickup must not add a queue entry"
    );

    // The implementer's live lease is on the spec; the review lease is on
    // the PR, so the pickup's scope-conflict check passes.
    let implementer = lease_with_role("impl", "STORY-77", "implementer");
    assert!(find_scope_lease_conflict(&[implementer.clone()], &plan.scope).is_none());
    // ...and the implementer holding the PR's source branch does not block
    // a headless review (no --force-claim, no ending the implementer).
    assert_eq!(
        review_source_branch_conflict_decision(
            true,
            true,
            implementer.role.as_deref(),
            Some("reviewer")
        ),
        LeaseConflictDecision::Coexist
    );

    // A second reviewer on the same PR is refused.
    let first_reviewer = lease_with_role("rev1", "PR-42", "reviewer");
    let leases = vec![implementer, first_reviewer];
    assert_eq!(
        find_scope_lease_conflict(&leases, &plan.scope).map(|l| l.id),
        Some("rev1".to_string())
    );
    assert_eq!(
        review_source_branch_conflict_decision(true, true, Some("reviewer"), Some("reviewer")),
        LeaseConflictDecision::Refuse
    );
}

/// BUG-2150: without `--role reviewer`, PR-N with no review story keeps the
/// existing behavior — no synthesized review plan.
// trace:BUG-2150 | ai:claude
#[test]
fn reviewer_pickup_keeps_pr_scope_only_for_explicit_reviewer_role() {
    assert!(reviewer_pickup_keeps_pr_scope(Some("reviewer")));
    assert!(reviewer_pickup_keeps_pr_scope(Some("Reviewer")));
    assert!(!reviewer_pickup_keeps_pr_scope(Some("implementer")));
    assert!(!reviewer_pickup_keeps_pr_scope(None));
}

/// BUG-2150 (advisor condition): the coexistence exception is only for a
/// reviewer pickup against a non-reviewer lease. An implementer pickup of the
/// same work still conflicts exactly as before: PR-42 without `--role
/// reviewer` gets no synthesized review plan, an implementer pickup of
/// STORY-77 collides with the live implementer lease, and an implementer
/// holding the PR's source branch still refuses a headless non-reviewer.
// trace:BUG-2150 | ai:claude
#[cfg(unix)]
#[test]
fn implementer_pickup_still_conflicts_with_live_implementer_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let gh = mock_gh(tmp.path());
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_AGENT_GATE_NAME", None),
        ("AIDA_AGENT_GATE_ROLE", None),
        ("AIDA_REVIEW_ROUND", None),
        ("AIDA_FROM_PR_REVIEW", None),
        ("AIDA_FROM_PR_NUMBER", None),
        ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
    ]);
    let root = tmp.path().join("aida-store");
    let storage = Storage::new(&root);
    let backend = aida_core::GitBackend::new(&root).unwrap();
    let mut spec = aida_core::Requirement::new("Fix the widget".to_string(), String::new());
    spec.spec_id = Some("STORY-77".to_string());
    spec.status = RequirementStatus::InProgress;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(spec);
    backend.save(&store).unwrap();

    for hint in [None, Some("implementer")] {
        assert!(
            resolve_queue_work_plan(
                &storage,
                "u",
                Some("PR-42"),
                None,
                false,
                false,
                false,
                hint
            )
            .is_err(),
            "PR-42 with role {hint:?} must not synthesize a review plan"
        );
    }

    let implementer = lease_with_role("impl", "STORY-77", "implementer");
    assert_eq!(
        find_scope_lease_conflict(&[implementer.clone()], "STORY-77").map(|l| l.id),
        Some("impl".to_string())
    );
    assert_eq!(
        review_source_branch_conflict_decision(
            true,
            true,
            implementer.role.as_deref(),
            Some("implementer")
        ),
        LeaseConflictDecision::Refuse
    );
    assert_eq!(
        review_source_branch_conflict_decision(true, true, implementer.role.as_deref(), None),
        LeaseConflictDecision::Refuse
    );
}

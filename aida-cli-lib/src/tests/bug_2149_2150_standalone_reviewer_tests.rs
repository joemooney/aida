//! BUG-2149 + BUG-2150: the standalone reviewer launched by
//! `aida queue work PR-<n> --role reviewer --no-human`.
//!
//! - BUG-2149: the reviewer's verdict must reach the LAUNCHER checkout's
//!   `.aida/review-verdicts/PR-<n>.json` (plus the spec record), even when a
//!   contained reviewer could only write it inside its review worktree — but
//!   only a verdict written during this run, at the PR's current head, by the
//!   exact reviewer grant this launch issued. The launch fails (with its own
//!   exit code) when no such verdict was produced.
//! - BUG-2150: a reviewer pickup is PR-scoped: it needs no queue entry and
//!   coexists with the implementer's live spec lease, while a second reviewer
//!   on the same PR is still refused.
use super::*;

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

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

/// Shell for a mock reviewer that records an approval the way `aida review
/// record` does — `<worktree>/.aida/review-verdicts/STORY-77.json`, stamped
/// with the recording grant `grant` (a literal id, or `$AIDA_SESSION_GRANT`
/// for the child's own grant) in both the attestation and the release stamp —
/// then runs `after` (e.g. a `touch` backdating it).
#[cfg(unix)]
fn records_approval(worktree: &std::path::Path, grant: &str, sha: &str, after: &str) -> String {
    let dir = worktree.join(".aida").join("review-verdicts");
    format!(
        "#!/bin/sh\nmkdir -p '{dir}'\ncat > '{dir}/STORY-77.json' <<EOF\n\
         {{\"verdict\":\"approved\",\"reviewed_sha\":\"{sha}\",\"summary\":\"ok\",\
         \"recorder_attestation\":{{\"sha\":\"{sha}\",\"seat\":\"reviewer\",\"grant_id\":\"{grant}\"}},\
         \"release_grant\":{{\"id\":\"{grant}\"}}}}\nEOF\n{after}\nexit 0\n",
        dir = dir.display()
    )
}

/// Backdate the recorded file to 2020 — "written before the reviewer started".
#[cfg(unix)]
fn backdate(worktree: &std::path::Path) -> String {
    format!(
        "touch -t 202001010000 '{}'",
        worktree
            .join(".aida/review-verdicts/STORY-77.json")
            .display()
    )
}

/// What a test's setup hands the launch.
#[cfg(unix)]
struct Launch<'a> {
    project: &'a std::path::Path,
    worktree: &'a std::path::Path,
    launcher_grant: &'a str,
}

/// Run the REAL `run_standalone_reviewer` against a mock reviewer script.
/// `setup` runs under the test's env guard once the launcher holds its grant
/// (so it may mint further grants) and returns the mock script. A mock `gh`
/// on PATH credits STORY-77 to PR-59; the PR head is pinned to [`HEAD`].
/// Returns the launch result and the project root.
#[cfg(unix)]
fn run_mock_reviewer(
    tmp: &std::path::Path,
    setup: impl Fn(&Launch<'_>, &mut crate::test_env::EnvVarsGuard) -> String,
) -> (Result<()>, std::path::PathBuf) {
    let (home, project) = codex_only_project(tmp);
    let worktree = tmp.join("reviewer-worktree");
    std::fs::create_dir_all(worktree.join(".aida")).unwrap();
    let mock = tmp.join("mock-reviewer.sh");
    let bin = tmp.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    crate::test_exec::write_executable(
        &bin.join("gh"),
        "#!/bin/sh\nprintf '%s' '{\"title\":\"Fix the widget (STORY-77)\",\"body\":\"\"}'\nexit 0\n",
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HEADLESS_VENDOR", None),
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("HOME", Some(home.to_str().unwrap())),
        ("PATH", Some(path.as_str())),
        ("AIDA_AGENT_CMD", Some(mock.to_str().unwrap())),
        ("AIDA_TEST_PR_HEAD_SHA", Some(HEAD)),
        (crate::seat_authority::GRANT_ENV, None),
    ]);
    let launcher_grant = crate::seat_authority::test_support::mint_grant_for(
        &project,
        &crate::current_user_id(None),
        "advisor",
        &["reviewer"],
    );
    env.set_key(crate::seat_authority::GRANT_ENV, &launcher_grant);
    let script = setup(
        &Launch {
            project: &project,
            worktree: &worktree,
            launcher_grant: &launcher_grant,
        },
        &mut env,
    );
    env.set_key(crate::seat_authority::GRANT_ENV, &launcher_grant);
    crate::test_exec::write_executable(&mock, script);
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
    (result, project)
}

/// Another reviewer grant delegated by the launcher — the grant an EARLIER or
/// a parallel sibling reviewer of the same launcher would hold.
#[cfg(unix)]
fn sibling_reviewer_grant(l: &Launch<'_>) -> String {
    crate::seat_authority::issue_child(l.project, "reviewer", &crate::current_user_id(None))
        .unwrap()
        .id
}

/// Assert the launch refused to collect anything, for `reason`.
#[cfg(unix)]
fn assert_refused(result: Result<()>, project: &std::path::Path, reason: &str) {
    let err = result.expect_err("the verdict must not be collected");
    assert_eq!(
        crate::exit_code_for_error(&err),
        NO_REVIEW_VERDICT_EXIT_CODE,
        "a missing verdict exits with its own code: {err:#}"
    );
    let msg = format!("{err:#}");
    assert!(msg.contains(reason), "expected `{reason}` in: {msg}");
    assert!(!project.join(".aida/review-verdicts/PR-59.json").exists());
    assert!(crate::review_verdict::read_recorded_verdict(project, "STORY-77").is_none());
}

/// BUG-2149: the reviewer recorded its approval inside the review worktree
/// (all a contained reviewer can write), under this launch's reviewer grant,
/// at the PR's current head. The launcher collects it into its own
/// `.aida/review-verdicts/PR-59.json` and mirrors the record for the spec the
/// PR credits, so `aida review verdict` / `aida pr ship` see it from the
/// primary checkout.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn standalone_reviewer_verdict_written_in_worktree_reaches_launcher_checkout() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project) = run_mock_reviewer(tmp.path(), |l, _| {
        records_approval(l.worktree, "$AIDA_SESSION_GRANT", HEAD, "")
    });
    result.expect("a verified review verdict must succeed");

    let body = std::fs::read_to_string(project.join(".aida/review-verdicts/PR-59.json"))
        .expect("the launcher checkout must hold the canonical PR-59.json verdict");
    let verdict = crate::review_verdict::parse_recorded_verdict(&body).unwrap();
    assert_eq!(verdict.kind, crate::review_verdict::VerdictKind::Approved);
    assert!(
        verdict.attestation.is_some(),
        "the reviewer's attestation must survive collection"
    );
    let spec = crate::review_verdict::read_recorded_verdict(&project, "STORY-77")
        .expect("the PR's spec record must be mirrored into the launcher checkout");
    assert_eq!(spec.kind, crate::review_verdict::VerdictKind::Approved);
    assert_eq!(spec.reviewed_sha.as_deref(), Some(HEAD));
}

/// BUG-2149: a reviewer that exits cleanly without recording any verdict
/// makes the launch fail with the no-verdict exit code (not 1, a crash).
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn standalone_reviewer_without_verdict_fails_the_launch() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project) = run_mock_reviewer(tmp.path(), |_, _| "#!/bin/sh\nexit 0\n".into());
    assert_refused(result, &project, "wrote no verdict file after it started");
}

/// BUG-2149 (condition d): a verdict file that predates the reviewer's start
/// is not collected. Freshness is the ONLY defect here: the record names this
/// launch's own reviewer grant and the current head — it is backdated.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn preexisting_verdict_is_not_collected_on_freshness_alone() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project) = run_mock_reviewer(tmp.path(), |l, _| {
        records_approval(
            l.worktree,
            "$AIDA_SESSION_GRANT",
            HEAD,
            &backdate(l.worktree),
        )
    });
    assert_refused(result, &project, "wrote no verdict file after it started");
}

/// BUG-2149 (round 1): a stale verdict (past mtime) stamped by an EARLIER
/// reviewer grant of the same launcher is refused.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn stale_verdict_from_earlier_reviewer_grant_is_refused() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project) = run_mock_reviewer(tmp.path(), |l, _| {
        let earlier = sibling_reviewer_grant(l);
        records_approval(l.worktree, &earlier, HEAD, &backdate(l.worktree))
    });
    assert_refused(result, &project, "wrote no verdict file after it started");
}

/// BUG-2149 (advisor ruling: equality): a FRESH verdict recorded by a sibling
/// reviewer grant of the same launcher — a parallel reviewer — is refused.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn fresh_verdict_from_sibling_reviewer_grant_is_refused() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project) = run_mock_reviewer(tmp.path(), |l, _| {
        let sibling = sibling_reviewer_grant(l);
        records_approval(l.worktree, &sibling, HEAD, "")
    });
    assert_refused(result, &project, "not this launch's reviewer grant");
}

/// BUG-2149 (round 1): a fresh verdict from a reviewer grant whose PARENT is a
/// different grant (another launcher) is refused.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn fresh_verdict_from_other_launchers_reviewer_grant_is_refused() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project) = run_mock_reviewer(tmp.path(), |l, env| {
        let other_parent = crate::seat_authority::test_support::mint_grant_for(
            l.project,
            &crate::current_user_id(None),
            "advisor",
            &["reviewer"],
        );
        assert_ne!(other_parent, l.launcher_grant);
        env.set_key(crate::seat_authority::GRANT_ENV, &other_parent);
        let foreign = crate::seat_authority::issue_child(
            l.project,
            "reviewer",
            &crate::current_user_id(None),
        )
        .unwrap()
        .id;
        records_approval(l.worktree, &foreign, HEAD, "")
    });
    assert_refused(result, &project, "not this launch's reviewer grant");
}

/// BUG-2149 (condition b): a fresh verdict recorded under the LAUNCHER's
/// grant is refused.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn verdict_recorded_under_launcher_grant_is_refused() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project) = run_mock_reviewer(tmp.path(), |l, _| {
        records_approval(l.worktree, l.launcher_grant, HEAD, "")
    });
    assert_refused(result, &project, "launcher's own grant");
}

/// BUG-2149 (condition b): a verdict for a commit other than the PR's current
/// head is refused.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn verdict_for_stale_head_is_refused() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let stale = "fedcba9876543210fedcba9876543210fedcba98";
    let (result, project) = run_mock_reviewer(tmp.path(), |l, _| {
        records_approval(l.worktree, "$AIDA_SESSION_GRANT", stale, "")
    });
    assert_refused(result, &project, "head is now");
}

/// BUG-2149 (round 1): a verdict path that is a symlink to a FIFO must not
/// hang the launcher — only regular files are read.
// trace:BUG-2149 | ai:claude
#[cfg(unix)]
#[test]
fn fifo_symlink_verdict_is_skipped_not_read() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project) = run_mock_reviewer(tmp.path(), |l, _| {
        let dir = l.worktree.join(".aida/review-verdicts");
        format!(
            "#!/bin/sh\nmkdir -p '{d}'\nmkfifo '{d}/pipe'\nln -s '{d}/pipe' '{d}/STORY-77.json'\nexit 0\n",
            d = dir.display()
        )
    });
    assert_refused(result, &project, "wrote no verdict file after it started");
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
        worktree_path: std::env::temp_dir().join(id),
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
    // A DEAD implementer lease is still ended, as before.
    assert_eq!(
        review_source_branch_conflict_decision(true, false, Some("implementer"), Some("reviewer")),
        LeaseConflictDecision::AutoEnd
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

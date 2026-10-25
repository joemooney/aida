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
// trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
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
    run_mock_reviewer_mode(tmp, false, setup)
}

// trace:BUG-2187 | ai:codex
#[cfg(unix)]
fn run_mock_reviewer_mode(
    tmp: &std::path::Path,
    no_human: bool,
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
    // Headless launch resolves its grant root independently of the arguments
    // passed to the standalone collector; pin that root to this fixture too.
    // trace:BUG-2187 | ai:codex
    struct RestoreAmbient(Option<crate::test_ambient::Ambient>);
    impl Drop for RestoreAmbient {
        fn drop(&mut self) {
            crate::test_ambient::replace(self.0.take());
        }
    }
    let _ambient = RestoreAmbient(crate::test_ambient::replace(Some(
        crate::test_ambient::Ambient {
            project_root: project.clone(),
            stdin_is_terminal: false,
            stdout_is_terminal: false,
            stderr_is_terminal: false,
        },
    )));
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
        no_human,
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
    // The PATH isolation helper above guarantees that this watchdog exits only
    // this exact-test subprocess if a blocking-open regression returns.
    // trace:TASK-1786 | ai:codex
    assert!(crate::test_env::is_path_isolated_process());
    let (done, wait) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        if matches!(
            wait.recv_timeout(std::time::Duration::from_secs(60)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ) {
            eprintln!("FIFO verdict test exceeded 60 seconds");
            std::process::exit(124);
        }
    });
    let tmp = tempfile::tempdir().unwrap();
    let (result, project) = run_mock_reviewer(tmp.path(), |l, _| {
        let dir = l.worktree.join(".aida/review-verdicts");
        format!(
            "#!/bin/sh\nmkdir -p '{d}'\nmkfifo '{d}/pipe'\nln -s '{d}/pipe' '{d}/STORY-77.json'\nexit 0\n",
            d = dir.display()
        )
    });
    assert_refused(result, &project, "wrote no verdict file after it started");
    // A direct FIFO reaches open itself; a symlink-only fixture cannot prove
    // that O_NONBLOCK is present on the candidate open.
    let direct = tmp.path().join("direct");
    std::fs::create_dir(&direct).unwrap();
    let (result, project) = run_mock_reviewer(&direct, |l, _| {
        let dir = l.worktree.join(".aida/review-verdicts");
        format!(
            "#!/bin/sh\nmkdir -p '{d}'\nmkfifo '{d}/STORY-77.json'\nexit 0\n",
            d = dir.display()
        )
    });
    drop(done);
    watchdog.join().unwrap();
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
        tree_kind: Default::default(),
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
        child_grant_id: None,
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
    let backend = crate::test_env::home_fixture_backend(&root).unwrap();
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
            Some("reviewer"),
            Some("STORY-77"),
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
        review_source_branch_conflict_decision(
            true,
            true,
            Some("reviewer"),
            Some("reviewer"),
            Some("STORY-77")
        ),
        LeaseConflictDecision::Refuse
    );
    // A DEAD implementer lease is still ended, as before.
    assert_eq!(
        review_source_branch_conflict_decision(
            true,
            false,
            Some("implementer"),
            Some("reviewer"),
            Some("STORY-77")
        ),
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
    let backend = crate::test_env::home_fixture_backend(&root).unwrap();
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
            Some("implementer"),
            Some("STORY-77"),
        ),
        LeaseConflictDecision::Refuse
    );
    assert_eq!(
        review_source_branch_conflict_decision(
            true,
            true,
            implementer.role.as_deref(),
            None,
            Some("STORY-77")
        ),
        LeaseConflictDecision::Refuse
    );
}

// trace:TASK-1786 | ai:codex
#[test]
fn reviewer_coexistence_requires_explicit_role_and_backing_spec() {
    for (explicit, backing) in [
        (None, Some("STORY-77")),
        (Some("reviewer"), None),
        (Some("reviewer"), Some("")),
    ] {
        assert_eq!(
            review_source_branch_conflict_decision(
                true,
                true,
                Some("implementer"),
                explicit,
                backing
            ),
            LeaseConflictDecision::Refuse
        );
    }
}

/// Exercise the queue-work input builder passed to session_start, including
/// rework's stored route. A resolved reviewer role is insufficient authority.
// trace:TASK-1786 | ai:codex
#[test]
fn queue_work_coexistence_input_preserves_explicit_intent() {
    use crate::queue_cmd::{rework_role_input, QueueWorkRoleInput};

    let mut spec = aida_core::Requirement::new("Fix the widget".into(), String::new());
    spec.spec_id = Some("STORY-77".into());
    let mut plan = standalone_review_plan(ReviewForge::GitHub, 42, &spec, "u");
    let implementer = lease_with_role("impl", "STORY-77", "implementer");
    // The plan itself carries a reviewer route even without a CLI override.
    assert_eq!(infer_queue_work_role(&plan, None).0, "reviewer");
    for (label, input, expected) in [
        (
            "inferred reviewer",
            QueueWorkRoleInput::direct(None),
            LeaseConflictDecision::Refuse,
        ),
        (
            "explicit --role reviewer",
            QueueWorkRoleInput::direct(Some("reviewer")),
            LeaseConflictDecision::Coexist,
        ),
        (
            "rework inherited reviewer route, no --for",
            rework_role_input(None, Some("reviewer")),
            LeaseConflictDecision::Refuse,
        ),
        (
            "rework explicit --for reviewer",
            rework_role_input(Some("reviewer"), Some("reviewer")),
            LeaseConflictDecision::Coexist,
        ),
        (
            "rework explicit --for any",
            rework_role_input(Some("any"), None),
            LeaseConflictDecision::Refuse,
        ),
    ] {
        let coexistence = input.review_coexistence(&plan);
        if expected == LeaseConflictDecision::Coexist {
            assert_eq!(coexistence, Some(("reviewer", "STORY-77")), "{label}");
        }
        assert_eq!(
            review_source_branch_conflict_decision(
                true,
                true,
                implementer.role.as_deref(),
                coexistence.map(|(role, _)| role),
                coexistence.map(|(_, spec)| spec),
            ),
            expected,
            "{label}",
        );
    }
    plan.review_target = None;
    assert_eq!(
        QueueWorkRoleInput::direct(Some("reviewer")).review_coexistence(&plan),
        None,
        "an ordinary work plan cannot authorize review coexistence",
    );
}

// trace:BUG-2187 | ai:codex
// trace:BUG-2187.ac2db2c6 | ai:claude
// trace:BUG-2187.ac63597b | ai:claude
#[cfg(unix)]
#[test]
fn bug2187_scrubbed_parent_supplies_headless_reviewer_attribution() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project) = run_mock_reviewer_mode(tmp.path(), true, |l, env| {
        env.unset_key("AIDA_AGENT_TYPE");
        env.unset_key("AIDA_AGENT_NAME");
        env.set_key("AIDA_OS_WRAP", "0");
        env.unset_key("AIDA_CONTAINED");
        std::fs::write(
            l.project.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n[agents]\nenabled = [\"claude\"]\n",
        )
        .unwrap();
        records_approval(l.worktree, "$AIDA_SESSION_GRANT", HEAD, "")
            .replacen("#!/bin/sh\n", "#!/bin/sh\n[ \"$AIDA_AGENT_TYPE\" = claude ] || exit 73\n[ \"$AIDA_AGENT_NAME\" = claude-reviewer ] || exit 73\n", 1)
    });
    result.expect("a scrubbed launcher must still identify its actual child vendor");
    let body = std::fs::read_to_string(project.join(".aida/review-verdicts/PR-59.json")).unwrap();
    let verdict = crate::review_verdict::parse_recorded_verdict(&body).unwrap();
    assert!(verdict.attestation.is_some());
    assert_eq!(verdict.kind, crate::review_verdict::VerdictKind::Approved);
}

/// BUG-2187: a scrubbed parent and a headless Claude reviewer, as the
/// production launch sees it. `script` is the mock reviewer's body.
// trace:BUG-2187 | ai:claude
#[cfg(unix)]
fn run_scrubbed_headless_claude(
    tmp: &std::path::Path,
    script: impl Fn(&Launch<'_>) -> String,
) -> (Result<()>, std::path::PathBuf) {
    run_mock_reviewer_mode(tmp, true, |l, env| {
        env.unset_key("AIDA_AGENT_TYPE");
        env.unset_key("AIDA_AGENT_NAME");
        env.set_key("AIDA_OS_WRAP", "0");
        env.unset_key("AIDA_CONTAINED");
        std::fs::write(
            l.project.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n[agents]\nenabled = [\"claude\"]\n",
        )
        .unwrap();
        script(l)
    })
}

/// A Claude reviewer killed before its result event: stream-json lines with
/// an assistant draft, and no `aida review record` attempt.
#[cfg(unix)]
fn unrecorded_claude_draft(exit: i32) -> String {
    let init = serde_json::json!({"type":"system","subtype":"init","session_id":"s"});
    let draft = serde_json::json!({"type":"assistant","message":{"content":[
        {"type":"text","text":"Verdict draft: request-changes, the leak is unfixed."}
    ]}});
    format!("#!/bin/sh\ncat <<'EOF'\n{init}\n{draft}\nEOF\nexit {exit}\n")
}

/// BUG-2187 AC2: the reviewer exits cleanly without recording. The launch
/// still fails with the no-verdict code, and even under `--quiet` its error
/// carries the reviewer's final draft, the retained log and a ready `aida
/// review record` command filled with the PR's spec, number and head.
// trace:BUG-2187.aca06f8e | ai:claude
#[cfg(unix)]
#[test]
fn bug2187_unrecorded_headless_review_surfaces_draft_and_record_command() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, project) =
        run_scrubbed_headless_claude(tmp.path(), |_| unrecorded_claude_draft(0));
    let err = result.expect_err("an unrecorded review is never a success");
    assert_eq!(
        crate::exit_code_for_error(&err),
        NO_REVIEW_VERDICT_EXIT_CODE
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("Verdict draft: request-changes, the leak is unfixed."),
        "{msg}"
    );
    assert!(
        msg.contains(&format!(
            "aida review record STORY-77 --pr 59 --sha {HEAD} --verdict"
        )),
        "{msg}"
    );
    assert!(msg.contains(".aida/headless-logs/"), "{msg}");
    assert!(!project.join(".aida/review-verdicts/PR-59.json").exists());
}

/// BUG-2187 AC2: a reviewer that exits non-zero without recording keeps its
/// own exit code, and the recovery text is returned rather than lost to a
/// bare `process::exit`.
// trace:BUG-2187.aca06f8e | ai:claude
#[cfg(unix)]
#[test]
fn bug2187_failed_headless_reviewer_keeps_exit_code_and_draft() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (result, _) = run_scrubbed_headless_claude(tmp.path(), |_| unrecorded_claude_draft(3));
    let err = result.expect_err("a failed reviewer with no verdict is an error");
    assert_eq!(crate::exit_code_for_error(&err), 3);
    let msg = format!("{err:#}");
    assert!(
        msg.contains("Verdict draft: request-changes, the leak is unfixed."),
        "{msg}"
    );
    assert!(msg.contains("aida review record STORY-77 --pr 59"), "{msg}");
}

/// BUG-2187 AC1, interactive arm: the launch-plan reviewer is identified by
/// its own vendor and seat, and its prompt carries no headless-only
/// recovery appendix.
// trace:BUG-2187.ac2db2c6 | ai:claude
#[cfg(unix)]
#[test]
fn bug2187_interactive_reviewer_gets_identity_without_headless_appendix() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let argv = tmp.path().join("argv.txt");
    let (result, _) = run_mock_reviewer(tmp.path(), |l, env| {
        env.unset_key("AIDA_AGENT_TYPE");
        env.unset_key("AIDA_AGENT_NAME");
        records_approval(l.worktree, "$AIDA_SESSION_GRANT", HEAD, "").replacen(
            "#!/bin/sh\n",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n[ \"$AIDA_AGENT_TYPE\" = codex ] || exit 0\n[ \"$AIDA_AGENT_NAME\" = codex-reviewer ] || exit 0\n",
                argv.display()
            ),
            1,
        )
    });
    result.expect("the interactive reviewer must carry its recording identity");
    let argv = std::fs::read_to_string(&argv).unwrap();
    assert!(argv.contains("/aida-review --pr 59"), "{argv}");
    assert!(!argv.contains("If recording the verdict fails"), "{argv}");
}

/// BUG-2187 AC1, interactive resume: a granted Claude resume carries the
/// reviewer identity even when the launcher's env has none.
// trace:BUG-2187.ac2db2c6 | ai:claude
#[cfg(unix)]
#[test]
fn bug2187_granted_reviewer_resume_sets_identity() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mock = tmp.path().join("claude");
    crate::test_exec::write_executable(
        &mock,
        "#!/bin/sh\n[ \"$AIDA_AGENT_TYPE\" = claude ] && [ \"$AIDA_AGENT_NAME\" = claude-reviewer ]\n",
    );
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_AGENT_CMD", Some(mock.to_str().unwrap())),
        ("AIDA_AGENT_TYPE", None),
        ("AIDA_AGENT_NAME", None),
        ("AIDA_CONTAINED", None),
    ]);
    let status = crate::session::spawn_claude_resume_with_grant(
        "019e0000-0000-7000-8000-0000000000cc",
        None,
        false,
        Some("grant-id"),
    )
    .unwrap();
    assert!(status.success(), "the resumed reviewer saw no identity");
}

/// The behavioral input test above must cover the adapter actually used by
/// rework's --work chain. Pin that dispatch seam too: replacing it with direct
/// would otherwise leave the adapter's unit tests green.
// trace:TASK-1801 | ai:codex
#[test]
fn rework_work_dispatch_uses_intent_preserving_adapter() {
    let source = include_str!("../queue_cmd.rs");
    let handler = source
        .split("pub(crate) fn handle_queue_rework(")
        .nth(1)
        .unwrap();
    let body = handler.split("/// Keep route inheritance").next().unwrap();
    assert!(body.contains("rework_role_input(for_role, for_role_resolved.as_deref())"));
    assert!(!body.contains("QueueWorkRoleInput::direct("));
}

// trace:TASK-1801 | ai:codex
#[test]
fn task1801_review_anchor_must_be_credited_by_change() {
    use crate::session_start::review_anchor_matches_change;
    let mut backing = aida_core::Requirement::new("backing".into(), String::new());
    backing.spec_id = Some("STORY-77".into());
    backing.agreed_id = Some("STORY-1-77".into());
    let mut other = aida_core::Requirement::new("unrelated".into(), String::new());
    other.spec_id = Some("TASK-88".into());
    let store = aida_core::RequirementsStore {
        requirements: vec![backing, other],
        ..Default::default()
    };
    for anchor in ["STORY-77", "STORY-1-77"] {
        assert!(review_anchor_matches_change(
            &store,
            anchor,
            "Fix (STORY-77)",
            "widget"
        ));
        assert!(review_anchor_matches_change(
            &store,
            anchor,
            "Fix",
            "codex/story-77"
        ));
    }
    assert!(!review_anchor_matches_change(
        &store,
        "TASK-88",
        "Fix (STORY-77)",
        "widget"
    ));
    assert!(!review_anchor_matches_change(
        &store,
        "STORY-77",
        "Mentions STORY-77 but fixes other (TASK-88)",
        "story-77"
    ));
    assert!(!review_anchor_matches_change(
        &store,
        "STORY-77",
        "Unidentified",
        "widget"
    ));
    assert!(!review_anchor_matches_change(
        &store,
        "STORY-99",
        "Fix (STORY-99)",
        "widget"
    ));
}

/// The live coexistence branch: unavailable or uncrediting change metadata
/// refuses a headless reviewer beside a live implementer lease, and the forge
/// is only asked once every other coexistence condition already holds.
// trace:TASK-1801 | ai:claude
#[test]
fn task1801_pickup_decision_refuses_without_credited_metadata() {
    use crate::session_start::review_pickup_conflict_decision;
    let mut backing = aida_core::Requirement::new("backing".into(), String::new());
    backing.spec_id = Some("STORY-77".into());
    let mut other = aida_core::Requirement::new("unrelated".into(), String::new());
    other.spec_id = Some("TASK-88".into());
    let store = aida_core::RequirementsStore {
        requirements: vec![backing, other],
        ..Default::default()
    };
    let pickup = Some(("reviewer", "STORY-77"));
    let decide = |title: anyhow::Result<String>| {
        review_pickup_conflict_decision(
            true,
            true,
            Some("implementer"),
            pickup,
            Some(&store),
            "widget",
            || title,
        )
    };
    assert_eq!(
        decide(Ok("Fix (STORY-77)".into())),
        LeaseConflictDecision::Coexist
    );
    assert_eq!(
        decide(Err(anyhow::anyhow!("forge unreachable"))),
        LeaseConflictDecision::Refuse,
        "unavailable metadata"
    );
    assert_eq!(
        decide(Ok("Fix (TASK-88)".into())),
        LeaseConflictDecision::Refuse,
        "metadata credits another spec"
    );
    // The already-resolved source branch can credit the anchor on its own.
    assert_eq!(
        review_pickup_conflict_decision(
            true,
            true,
            Some("implementer"),
            pickup,
            Some(&store),
            "codex/story-77",
            || Ok("Untitled".into()),
        ),
        LeaseConflictDecision::Coexist
    );

    // Lazy: no forge read when coexistence is already impossible.
    let unreachable = || -> anyhow::Result<String> { panic!("forge must not be queried") };
    for (live, holder, pickup, store) in [
        (false, Some("implementer"), pickup, Some(&store)),
        (true, Some("reviewer"), pickup, Some(&store)),
        (
            true,
            Some("implementer"),
            Some(("implementer", "STORY-77")),
            Some(&store),
        ),
        (true, Some("implementer"), None, Some(&store)),
        (true, Some("implementer"), pickup, None),
    ] {
        assert_ne!(
            review_pickup_conflict_decision(
                true,
                live,
                holder,
                pickup,
                store,
                "widget",
                unreachable
            ),
            LeaseConflictDecision::Coexist
        );
    }
}

/// TASK-1674: the exit-30 message names at most five denials and counts the
/// rest, so a run with dozens of denials stays one readable line.
// trace:TASK-1955 | ai:claude (TASK-1674)
#[test]
fn no_verdict_message_caps_the_denial_list() {
    let denials: Vec<String> = (1..=12).map(|n| format!("Bash(denied-{n})")).collect();
    let err = crate::queue_cmd::standalone_reviewer_outcome(
        7,
        Some("no verdict".into()),
        std::path::Path::new("/v/PR-7.json"),
        &denials,
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("denied 12 of the reviewer's tool call(s)"),
        "{err}"
    );
    assert!(err.contains("Bash(denied-5); …7 more."), "{err}");
    assert!(!err.contains("Bash(denied-6)"), "{err}");

    let few = &denials[..2];
    let err = crate::queue_cmd::standalone_reviewer_outcome(
        7,
        Some("no verdict".into()),
        std::path::Path::new("/v/PR-7.json"),
        few,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("Bash(denied-1); Bash(denied-2)."), "{err}");
    assert!(!err.contains("more"), "{err}");
}

/// TASK-1805: an explicit reviewer whose backing ID is unknown, or ambiguous
/// (two requirements share it), never coexists with a live implementer lease,
/// even when the change title credits that same ID; and the explicit role
/// comes from the pickup, so a non-reviewer pickup never coexists.
// trace:TASK-1955 | ai:claude (TASK-1805)
#[test]
fn task1805_pickup_refuses_an_unknown_or_ambiguous_backing_id() {
    use crate::session_start::review_pickup_conflict_decision;
    let spec = |id: &str, title: &str| {
        let mut r = aida_core::Requirement::new(title.into(), String::new());
        r.spec_id = Some(id.into());
        r
    };
    let unique = aida_core::RequirementsStore {
        requirements: vec![spec("STORY-77", "backing")],
        ..Default::default()
    };
    let colliding = aida_core::RequirementsStore {
        requirements: vec![spec("STORY-77", "one"), spec("STORY-77", "two")],
        ..Default::default()
    };
    let decide = |pickup, store, title: &'static str| {
        review_pickup_conflict_decision(
            true,
            true,
            Some("implementer"),
            pickup,
            Some(store),
            "widget",
            || Ok(title.to_string()),
        )
    };
    assert_eq!(
        decide(Some(("reviewer", "STORY-77")), &unique, "Fix (STORY-77)"),
        LeaseConflictDecision::Coexist,
        "control: a unique, credited backing ID coexists"
    );
    assert_eq!(
        decide(Some(("reviewer", "STORY-99")), &unique, "Fix (STORY-99)"),
        LeaseConflictDecision::Refuse,
        "unknown backing ID"
    );
    assert_eq!(
        decide(Some(("reviewer", "STORY-77")), &colliding, "Fix (STORY-77)"),
        LeaseConflictDecision::Refuse,
        "ambiguous backing ID"
    );
    assert_ne!(
        decide(Some(("implementer", "STORY-77")), &unique, "Fix (STORY-77)"),
        LeaseConflictDecision::Coexist,
        "the explicit role is the pickup's, not the backing spec's"
    );
}

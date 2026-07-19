use super::*;

/// TASK-740: exhaustive parity — `auto_bump_eligible_status` (now delegating
/// to `lifecycle::git_merge_completes`) matches the pre-migration hand-coded
// predicate (BUG-328 + BUG-405) over every status. trace:TASK-740
#[test]
fn auto_bump_eligible_status_parity_with_oracle() {
    use aida_core::models::RequirementStatus as S;
    fn oracle(s: &S) -> bool {
        matches!(
            s,
            S::Approved | S::Planned | S::InProgress | S::Done | S::NeedsAttention
        )
    }
    let all = [
        S::Draft,
        S::Approved,
        S::Planned,
        S::InProgress,
        S::Done,
        S::Completed,
        S::Rejected,
        S::NeedsAttention,
    ];
    for s in &all {
        assert_eq!(
            auto_bump_eligible_status(s),
            oracle(s),
            "parity mismatch at {s}"
        );
    }
}

/// The env-flag opt-out follows the same convention as
/// `auto_merge_gate_enabled`: anything in {false, 0, no, off}
/// (case-insensitive, trimmed) disables; anything else (including
/// unset, empty, "true", "1") leaves the feature on.
#[test]
fn auto_bump_env_flag_respects_opt_out() {
    // TASK-521: route AIDA_AUTO_BUMP mutation through the shared
    // `EnvVarGuard` so parallel tests reading the var don't see
    // torn state, and the prior value is restored on drop (no
    // hand-rolled save/restore). The guard holds ENV_LOCK for the
    // whole test, so `reset` can swap values without releasing it
    // between spellings. trace:TASK-521 | ai:claude
    let mut guard = crate::test_env::EnvVarGuard::unset("AIDA_AUTO_BUMP");

    // Unset → on.
    assert!(auto_bump_enabled());

    // Each "off" spelling → off.
    for off in &["false", "0", "no", "off", "FALSE", "Off", " no "] {
        guard.reset(off);
        assert!(
            !auto_bump_enabled(),
            "AIDA_AUTO_BUMP={:?} should disable",
            off
        );
    }

    // Anything else → on (including the canonical "true" / "1").
    for on in &["true", "1", "", "yes", "anything-else"] {
        guard.reset(on);
        assert!(
            auto_bump_enabled(),
            "AIDA_AUTO_BUMP={:?} should stay on",
            on
        );
    }
}

/// STORY-86: `set_status_from_str("Done")` lands on the canonical
/// variant (not a custom_status fallback), accepts both `"Done"`
/// (display form) and `"done"` (CLI flag form), and clears any prior
/// custom_status. This is the contract `queue done` relies on.
#[test]
fn set_status_from_str_done_lands_canonical() {
    use aida_core::Requirement;

    let mut req = Requirement::new("test".to_string(), "test".to_string());

    // Pre-condition: brand-new req is Draft, no custom_status.
    assert!(matches!(req.status, RequirementStatus::Draft));
    assert!(req.custom_status.is_none());

    // Display form lands on canonical Done.
    req.set_status_from_str("Done");
    assert!(matches!(req.status, RequirementStatus::Done));
    assert!(req.custom_status.is_none());

    // CLI form (lowercase) also lands on canonical Done.
    let mut req2 = Requirement::new("test2".to_string(), "test2".to_string());
    req2.set_status_from_str("done");
    assert!(matches!(req2.status, RequirementStatus::Done));
    assert!(req2.custom_status.is_none());

    // A previously-set custom_status (legacy "done" string that
    // used to fall through before the normalizer recognized it)
    // gets cleared when the canonical match fires.
    let mut req3 = Requirement::new("test3".to_string(), "test3".to_string());
    req3.custom_status = Some("legacy-done".to_string());
    req3.set_status_from_str("done");
    assert!(matches!(req3.status, RequirementStatus::Done));
    assert!(
        req3.custom_status.is_none(),
        "set_status_from_str must clear custom_status when canonical recognized"
    );
}

/// STORY-86: validate_status_input accepts "done" as its own state
/// (separate from "completed"). Used at the CLI boundary by
/// `aida edit --status done` and equivalent paths so typos like
/// "doen" still get rejected.
#[test]
fn validate_status_input_recognizes_done() {
    assert_eq!(validate_status_input("done"), Ok("Done"));
    assert_eq!(validate_status_input("DONE"), Ok("Done"));
    assert_eq!(validate_status_input("Done"), Ok("Done"));
    // "completed" still maps to Completed (not Done — that was the
    // old aliased behavior before STORY-86).
    assert_eq!(validate_status_input("completed"), Ok("Completed"));
    // Typo gets rejected with a list-of-valid-values message that
    // includes "done".
    match validate_status_input("doen") {
        Err(msg) => assert!(
            msg.contains("done"),
            "error message should list `done` as valid: {}",
            msg
        ),
        Ok(_) => panic!("typo should not validate"),
    }
}

/// STORY-332: `validate_status_input` and `parse_status` both recognise
/// the NeedsAttention status across its spelling variants, and the
/// error message lists it so a typo is still rejected helpfully.
#[test]
fn status_parsers_recognize_needs_attention() {
    assert_eq!(
        validate_status_input("needs-attention"),
        Ok("NeedsAttention")
    );
    assert_eq!(
        validate_status_input("Needs Attention"),
        Ok("NeedsAttention")
    );
    assert_eq!(
        validate_status_input("NEEDSATTENTION"),
        Ok("NeedsAttention")
    );
    assert!(
        matches!(
            parse_status("needs-attention"),
            Ok(RequirementStatus::NeedsAttention)
        ),
        "parse_status should accept needs-attention"
    );
    match validate_status_input("needs-attentoin") {
        Err(msg) => assert!(
            msg.contains("needs-attention"),
            "error should list needs-attention: {msg}"
        ),
        Ok(_) => panic!("typo should not validate"),
    }
}

/// STORY-332: a punted spec is not terminal — it resumes — so it must
/// stay out of the terminal-status predicates that drive child-guards
/// and the `aida list` default hide.
#[test]
fn needs_attention_is_not_terminal() {
    assert!(!is_terminal_status(&RequirementStatus::NeedsAttention));
    assert!(!is_terminal_status_str("Needs Attention"));
}

/// STORY-86: `is_terminal_status_str` no longer treats "done" as
/// terminal. The string twin must agree with the enum predicate so
/// `aida list` / `aida history` and the child-guard agree on what
/// "closed work" means.
#[test]
fn is_terminal_status_str_excludes_done() {
    assert!(is_terminal_status_str("Completed"));
    assert!(is_terminal_status_str("Rejected"));
    assert!(is_terminal_status_str("completed"));
    // The whole point of the story: Done is open.
    assert!(!is_terminal_status_str("Done"));
    assert!(!is_terminal_status_str("done"));
    assert!(!is_terminal_status_str("In Progress"));
    assert!(!is_terminal_status_str("Draft"));
}

// ────────────────────────────────────────────────────────────────────
// Integration-style coverage for `auto_bump_done_to_completed`.
//
// Sets up a temp project that has both a code git repo and an
// orphan-store-style YAML backend (we use the plain Storage::new
// path here — simulates `aida init` for purposes of the bump scan).
// Runs the helper against synthetic commits and asserts the
// status / completed_at / completion_sha contract.
// ────────────────────────────────────────────────────────────────────

/// Tiny shell-out helper for the tests below. Returns stdout on
/// success, panics otherwise — these are setup helpers, not the
/// thing under test, so we want failures to be loud.
fn run_git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git binary on PATH");
    if !out.status.success() {
        panic!(
            "git {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Build a temp project: code repo at `<tmp>/` with a single
/// `main` commit, and a YAML-backed store at
/// `<tmp>/requirements.yaml`. We're testing the auto-bump helper's
/// pure interaction with the `Storage` API (load + write through
/// `update_atomically`), so the YAML backend is enough — the helper
/// doesn't care which backend powers the store.
/// Returns `(temp_dir_guard, project_root, store_path)` where
/// `store_path` is the YAML file path.
fn init_test_project() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let project_root = tmp.path().to_path_buf();
    let store_path = project_root.join("requirements.yaml");

    // Code repo init + identity (git refuses commits without one)
    // + an initial commit so HEAD resolves and pre_sha is non-empty.
    run_git(&project_root, &["init", "--initial-branch=main", "--quiet"]);
    run_git(&project_root, &["config", "user.email", "test@example.com"]);
    run_git(&project_root, &["config", "user.name", "Test"]);
    std::fs::write(project_root.join("README.md"), "init\n").unwrap();
    run_git(&project_root, &["add", "README.md"]);
    run_git(&project_root, &["commit", "-m", "chore: init"]);

    // Seed an empty store so the first load doesn't have to default.
    let storage = Storage::new(&store_path);
    storage
        .save(&aida_core::RequirementsStore::default())
        .unwrap();

    (tmp, project_root, store_path)
}

/// Insert a Story spec at the given status with the given spec_id into
/// the store and persist it. Returns the spec_id we used.
fn seed_spec_at(store_path: &std::path::Path, spec_id: &str, status: &str) -> String {
    let storage = Storage::new(store_path);
    let mut store = storage.load().unwrap_or_default();
    let mut req = aida_core::Requirement::new(format!("test-{}", spec_id), String::new());
    req.spec_id = Some(spec_id.to_string());
    req.set_status_from_str(status);
    store.requirements.push(req);
    storage.save(&store).unwrap();
    spec_id.to_string()
}

/// Insert a Story spec at status=Done with the given spec_id into
/// the store and persist it. Returns the spec_id we used.
fn seed_done_spec(store_path: &std::path::Path, spec_id: &str) -> String {
    seed_spec_at(store_path, spec_id, "Done")
}

fn has_flip(flips: &[AutoBumpFlip], spec_id: &str) -> bool {
    flips.iter().any(|f| f.spec_id == spec_id)
}

/// STORY-86: the helper picks up a `(SPEC-ID)` from a commit subject
/// on the default branch, flips the spec from Done to Completed, and
/// stamps `completed_at` + `completion_sha` on `implementation_info`.
// trace:STORY-86 | ai:claude
#[test]
fn auto_bump_picks_up_subject_refs_on_default_branch() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_done_spec(&store_path, "STORY-9001");

    // Pre-snapshot, then create a code commit referencing the spec.
    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &[
            "commit",
            "-m",
            &format!("feat: land the thing ({})", spec_id),
        ],
    );
    let merge_sha = run_git(&project_root, &["rev-parse", "HEAD"]);

    let storage = Storage::new(store_path.clone());
    let flips =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    assert_eq!(flips.len(), 1, "exactly one spec should flip");
    assert_eq!(flips[0].spec_id, spec_id);
    assert_eq!(flips[0].sha, merge_sha);

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "status should be Completed, was {:?}",
        req.status
    );
    let info = req.implementation_info.as_ref().expect("info populated");
    assert_eq!(
        info.completion_sha.as_deref(),
        Some(merge_sha.as_str()),
        "completion_sha should match landing commit"
    );
    assert!(info.completed_at.is_some(), "completed_at should be set");
}

/// BUG-477: the merge-driven Done→Completed auto-bump must record the
/// status transition in the per-spec `history:` array (the source-of-truth
/// for spec-state time series), the same way the manual `aida edit --status`
/// path does. Before the fix the bump only assigned `status` and wrote no
/// `HistoryEntry`, so burn-down analytics walking history under-counted the
// most common completion transition. trace:BUG-477
#[test]
fn auto_bump_records_status_transition_in_history() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_done_spec(&store_path, "STORY-9477");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &[
            "commit",
            "-m",
            &format!("feat: land the thing ({})", spec_id),
        ],
    );

    let storage = Storage::new(store_path.clone());
    let flips =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();
    assert_eq!(flips.len(), 1, "exactly one spec should flip");

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "status should be Completed, was {:?}",
        req.status
    );

    // The transition must land as a structured history row recording the
    // status field change Done → Completed.
    let status_entry = req
        .history
        .iter()
        .find(|h| h.changes.iter().any(|c| c.field_name == "status"))
        .expect("auto-bump must record a status field change in history");
    let change = status_entry
        .changes
        .iter()
        .find(|c| c.field_name == "status")
        .unwrap();
    assert_eq!(
        change.old_value, "Done",
        "history old_value should be the prior status (Done)"
    );
    assert_eq!(
        change.new_value, "Completed",
        "history new_value should be Completed"
    );
}

/// BUG-426: a `docs(plans): …` plan commit names the specs it PLANS for
/// in its trailing `(SPEC-ID …)` group, but a plan is pre-implementation
/// — those specs stay Approved. The auto-bump candidate scan must NOT
/// treat a plan commit's trailer as a completion signal (it previously
/// false-completed the umbrella specs `(TASK-136 BUG-420)` off a plan-only
/// commit). A non-plan commit referencing the same spec still completes
// it. trace:BUG-426 | ai:claude
#[test]
fn auto_bump_ignores_plan_commit_trailers() {
    let (_tmp, project_root, store_path) = init_test_project();
    // Two Approved umbrella specs, exactly as TASK-136/BUG-420 were.
    let planned = seed_spec_at(&store_path, "TASK-8801", "Approved");
    let delivered = seed_spec_at(&store_path, "TASK-8802", "Approved");

    // A plan commit naming both specs — must NOT complete either.
    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    std::fs::write(project_root.join("plan.md"), "the plan\n").unwrap();
    run_git(&project_root, &["add", "plan.md"]);
    run_git(
        &project_root,
        &[
            "commit",
            "-m",
            &format!(
                "[AI:claude] docs(plans): hardening plan ({} {})",
                planned, delivered
            ),
        ],
    );
    // A real delivery commit for the second spec only.
    std::fs::write(project_root.join("ship.rs"), "fn ship() {}\n").unwrap();
    run_git(&project_root, &["add", "ship.rs"]);
    run_git(
        &project_root,
        &["commit", "-m", &format!("fix: ship it ({})", delivered)],
    );
    let ship_sha = run_git(&project_root, &["rev-parse", "HEAD"]);

    let storage = Storage::new(store_path.clone());
    let flips =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    assert!(
        !has_flip(&flips, &planned),
        "plan-only spec must NOT be completed by a docs(plans) trailer"
    );
    assert!(
        has_flip(&flips, &delivered),
        "the fix-committed spec should still complete"
    );

    let after = storage.load().unwrap();
    assert!(
        matches!(
            after.get_requirement_by_spec_id(&planned).unwrap().status,
            RequirementStatus::Approved
        ),
        "planned-only umbrella stays Approved"
    );
    let shipped = after.get_requirement_by_spec_id(&delivered).unwrap();
    assert!(matches!(shipped.status, RequirementStatus::Completed));
    assert_eq!(
        shipped
            .implementation_info
            .as_ref()
            .and_then(|i| i.completion_sha.as_deref()),
        Some(ship_sha.as_str()),
        "completion_sha is the fix commit, not the plan commit"
    );
}

/// BUG-426: `is_plan_commit_subject` recognizes plan commits (with or
/// without an `[AI:tool]` prefix) and leaves ordinary docs / delivery
// commits alone. trace:BUG-426 | ai:claude
#[test]
fn is_plan_commit_subject_classifies_plan_commits() {
    assert!(is_plan_commit_subject("docs(plans): a plan (TASK-1)"));
    assert!(is_plan_commit_subject("docs(plan): a plan (TASK-1)"));
    assert!(is_plan_commit_subject(
        "[AI:claude] docs(plans): a plan (TASK-1)"
    ));
    assert!(is_plan_commit_subject(
        "  [AI:claude:med] docs(plans): a plan (TASK-1)"
    ));
    // Not plan commits.
    assert!(!is_plan_commit_subject("docs: update README (TASK-1)"));
    assert!(!is_plan_commit_subject("docs(readme): tweak (TASK-1)"));
    assert!(!is_plan_commit_subject("feat: land the thing (TASK-1)"));
    assert!(!is_plan_commit_subject("fix(plans): real fix (TASK-1)"));
}

/// BUG-405: a spec the drain shelved into NeedsAttention (with a
/// populated FailureReason) whose referencing commit then lands on the
/// default branch graduates to Completed, and the stale FailureReason is
/// cleared so `aida findings list` stops surfacing a "CI is red" finding
// for work that actually shipped. trace:BUG-405 | ai:claude
#[test]
fn auto_bump_completes_needs_attention_spec_and_clears_failure_reason() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_spec_at(&store_path, "TASK-9021", "Needs Attention");

    // Stamp a shelving FailureReason, exactly as the drain does on CI red.
    {
        let storage = Storage::new(store_path.clone());
        let mut store = storage.load().unwrap();
        let r = store
            .requirements
            .iter_mut()
            .find(|r| r.spec_id.as_deref() == Some(spec_id.as_str()))
            .unwrap();
        r.failure_reason = Some(aida_core::FailureReason {
            phase: "ci".to_string(),
            phase_index: 2,
            kind: "ci-red".to_string(),
            detail: format!("CI is red on PR for {}", spec_id),
            recovery_hint: None,
            shelved_by: None,
            shelved_at: chrono::Utc::now(),
        });
        storage.save(&store).unwrap();
    }

    // A later session fixes CI and the PR merges — the spec's commit
    // lands on the default branch.
    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    std::fs::write(project_root.join("fix.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "fix.txt"]);
    run_git(
        &project_root,
        &[
            "commit",
            "-m",
            &format!("fix: ship after reshelving ({})", spec_id),
        ],
    );

    let storage = Storage::new(store_path.clone());
    let flips =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    assert!(
        has_flip(&flips, &spec_id),
        "a NeedsAttention spec with a merged commit should flip"
    );

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "status should be Completed, was {:?}",
        req.status
    );
    assert!(
        req.failure_reason.is_none(),
        "stale FailureReason must be cleared once the spec completes"
    );
}

/// BUG-328: direct spec refs at Approved/Planned/InProgress now
/// graduate to Completed when their commit lands on main. Draft
/// preserves the approval signal; terminal statuses stay untouched.
// trace:BUG-328 | ai:codex
#[test]
fn auto_bump_eligibility_matrix_for_direct_subject_refs() {
    let cases = [
        ("STORY-9011", "Approved", true),
        ("STORY-9012", "Planned", true),
        ("STORY-9013", "In Progress", true),
        ("STORY-9014", "Done", true),
        ("STORY-9015", "Draft", false),
        ("STORY-9016", "Completed", false),
        ("STORY-9017", "Rejected", false),
    ];
    for (spec_id, status, should_flip) in cases {
        let (_tmp, project_root, store_path) = init_test_project();
        seed_spec_at(&store_path, spec_id, status);

        let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
        std::fs::write(project_root.join("file.txt"), format!("land {spec_id}\n")).unwrap();
        run_git(&project_root, &["add", "file.txt"]);
        run_git(
            &project_root,
            &["commit", "-m", &format!("feat: land ({})", spec_id)],
        );

        let storage = Storage::new(store_path.clone());
        let flips =
            auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage)
                .unwrap();
        assert_eq!(
            has_flip(&flips, spec_id),
            should_flip,
            "{status} eligibility mismatch; flips={flips:?}"
        );

        let after = storage.load().unwrap();
        let req = after.get_requirement_by_spec_id(spec_id).unwrap();
        if should_flip {
            assert!(
                matches!(req.status, RequirementStatus::Completed),
                "{} should be Completed, was {:?}",
                status,
                req.status
            );
            assert!(
                req.implementation_info
                    .as_ref()
                    .and_then(|i| i.completed_at)
                    .is_some(),
                "{} should stamp completed_at",
                status
            );
        } else {
            assert_eq!(
                req.status.to_string(),
                status,
                "{} should not auto-bump",
                status
            );
        }
    }
}

/// STORY-86: when current branch ≠ default branch, the helper is a
/// silent no-op — merges to default haven't happened yet, so no
// auto-bump fires. trace:STORY-86 | ai:claude
#[test]
fn auto_bump_skips_when_not_on_default_branch() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_done_spec(&store_path, "STORY-9002");

    // Move to a feature branch BEFORE the commit. The helper's
    // default-branch detection should refuse to bump from here.
    run_git(&project_root, &["checkout", "-b", "feature/test"]);
    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &[
            "commit",
            "-m",
            &format!("feat: should-not-bump ({})", spec_id),
        ],
    );

    let storage = Storage::new(store_path.clone());
    let flips =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    assert!(
        flips.is_empty(),
        "feature branch should not produce flips: {:?}",
        flips
    );
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Done),
        "spec should still be Done, was {:?}",
        req.status
    );
}

/// STORY-86: helper is idempotent — running it twice on the same
/// merged commit only flips once. The second invocation sees the
/// spec at Completed (not Done) and silently skips.
// trace:STORY-86 | ai:claude
#[test]
fn auto_bump_is_idempotent() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_done_spec(&store_path, "STORY-9003");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &["commit", "-m", &format!("feat: land twice ({})", spec_id)],
    );

    let storage = Storage::new(store_path.clone());

    let first =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();
    assert_eq!(first.len(), 1, "first call flips once");

    let second =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();
    assert!(second.is_empty(), "second call is a no-op: {:?}", second);
}

/// BUG-410: a spec auto-completed by a commit and then MANUALLY REOPENED
/// (status back to eligible, completion_sha retained as a `--force` reopen
/// leaves it) must NOT be re-completed by the SAME commit on a later pull —
/// that silently overwrites the deliberate reopen. Fails without the
// completion_sha dedup guard. trace:BUG-410 | ai:claude
#[test]
fn auto_bump_does_not_recomplete_reopened_spec_by_same_commit() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_done_spec(&store_path, "STORY-9410");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &["commit", "-m", &format!("feat: land ({})", spec_id)],
    );

    let storage = Storage::new(store_path.clone());

    // First pull completes it + stamps completion_sha.
    let first =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();
    assert!(has_flip(&first, &spec_id), "first pull completes it");

    // Manually reopen: status → Approved, completion_sha retained.
    let mut store = storage.load().unwrap();
    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.spec_id.as_deref() == Some(spec_id.as_str()))
        .unwrap();
    assert!(
        req.implementation_info
            .as_ref()
            .and_then(|i| i.completion_sha.as_deref())
            .is_some(),
        "completion_sha was stamped on completion"
    );
    req.set_status_from_str("Approved");
    storage.save(&store).unwrap();

    // A later pull scanning the SAME commit must NOT re-complete it.
    let second =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();
    assert!(
        !has_flip(&second, &spec_id),
        "same-commit re-bump must be skipped: {:?}",
        second
    );
    let reloaded = storage.load().unwrap();
    let still = reloaded
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some(spec_id.as_str()))
        .unwrap();
    assert_eq!(
        still.status,
        aida_core::RequirementStatus::Approved,
        "the deliberate reopen survives the re-pull"
    );
}

/// STORY-86: a commit whose subject does NOT reference any spec in
/// Done leaves the store untouched. Guards against the helper
// over-firing on prose/release commits. trace:STORY-86 | ai:claude
#[test]
fn auto_bump_ignores_unrelated_commits() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_done_spec(&store_path, "STORY-9004");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    // Commit refs a DIFFERENT spec id that doesn't exist in the
    // store. Helper should produce no flips for our seeded spec.
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &["commit", "-m", "chore: unrelated (BUG-9999)"],
    );

    let storage = Storage::new(store_path.clone());
    let flips =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();
    assert!(flips.is_empty());

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Done),
        "unrelated commit should leave the spec at Done"
    );
}

/// Seed a Story-typed review-story at status=Done with the given
/// `Review PR-<n>: <suffix>` title and spec_id. Mirrors
/// `seed_done_spec` but stamps the title that BUG-102's matcher keys
// off of. trace:BUG-102 | ai:claude
fn seed_done_review_story(
    store_path: &std::path::Path,
    spec_id: &str,
    pr_number: u64,
    suffix: &str,
) -> String {
    let storage = Storage::new(store_path);
    let mut store = storage.load().unwrap_or_default();
    let mut req = aida_core::Requirement::new(
        format!("Review PR-{}: {}", pr_number, suffix),
        String::new(),
    );
    req.spec_id = Some(spec_id.to_string());
    req.req_type = aida_core::RequirementType::Story;
    req.set_status_from_str("Done");
    store.requirements.push(req);
    storage.save(&store).unwrap();
    spec_id.to_string()
}

/// BUG-102: a merge commit with `(#N)` squash-suffix flips any Done
/// review story whose title encodes PR-N — even when that review
/// story's spec ID is NOT in any commit subject. This is the gap
/// before the fix: review stories filed by /aida-pr's auto-queue
// were stuck at Done forever. trace:BUG-102 | ai:claude
#[test]
fn auto_bump_flips_review_story_via_pr_number_match() {
    let (_tmp, project_root, store_path) = init_test_project();
    let review_id =
        seed_done_review_story(&store_path, "STORY-9101", 42, "EPIC-23 batch X: small wrap");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    // Squash-merge subject: subject contains code-spec parens AND the
    // (#N) suffix. The review story's spec ID is intentionally absent
    // from the subject — that's the whole point of the bug.
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &[
            "commit",
            "-m",
            "EPIC-23 batch X: small wrap (BUG-9101 TASK-9101) (#42)",
        ],
    );
    let merge_sha = run_git(&project_root, &["rev-parse", "HEAD"]);

    let storage = Storage::new(store_path.clone());
    let flips =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    assert!(
        has_flip(&flips, &review_id),
        "review story should flip via PR-number match, got: {:?}",
        flips
    );
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&review_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "review story should be Completed, was {:?}",
        req.status
    );
    let info = req.implementation_info.as_ref().expect("info populated");
    assert_eq!(
        info.completion_sha.as_deref(),
        Some(merge_sha.as_str()),
        "completion_sha should match the merge commit"
    );
}

/// BUG-106: seed a Done review story for PR-N that `implements`
/// (covers) the given specs — mirrors what /aida-pr's auto-queue
/// records. Each covered spec is seeded at Done. Returns
// `(review_story_spec_id, covered_spec_ids)`. trace:BUG-106 | ai:claude
fn seed_cluster_pr_review_story(
    store_path: &std::path::Path,
    review_spec_id: &str,
    pr_number: u64,
    covered: &[&str],
) -> (String, Vec<String>) {
    let storage = Storage::new(store_path);
    let mut store = storage.load().unwrap_or_default();

    let mut covered_ids: Vec<String> = Vec::new();
    let mut covered_uuids: Vec<uuid::Uuid> = Vec::new();
    for spec in covered {
        let mut req = aida_core::Requirement::new(format!("test-{}", spec), String::new());
        req.spec_id = Some(spec.to_string());
        req.set_status_from_str("Done");
        covered_uuids.push(req.id);
        covered_ids.push(spec.to_string());
        store.requirements.push(req);
    }

    let mut review = aida_core::Requirement::new(
        format!(
            "Review PR-{}: cluster of {} specs",
            pr_number,
            covered.len()
        ),
        String::new(),
    );
    review.spec_id = Some(review_spec_id.to_string());
    review.req_type = aida_core::RequirementType::Story;
    review.set_status_from_str("Done");
    for target_id in covered_uuids {
        review.relationships.push(aida_core::Relationship {
            rel_type: aida_core::RelationshipType::Custom("implements".to_string()),
            target_id,
            created_at: None,
            created_by: None,
        });
    }
    store.requirements.push(review);
    storage.save(&store).unwrap();
    (review_spec_id.to_string(), covered_ids)
}

/// BUG-113: seed a `Done` review story for PR-N that `implements` one
/// covered spec, with the covered spec at `covered_status`. Mirrors
/// what /aida-pr's auto-queue records (`## Covers` list + an
/// `implements` relationship per covered spec). When the covered spec
/// is `Completed` it is stamped with a `completion_sha` so the covers
/// chain has a real merge sha to propagate. Returns
// `(review_spec_id, covered_spec_id)`. trace:BUG-113 | ai:claude
fn seed_covers_chain_review_story(
    store_path: &std::path::Path,
    review_spec_id: &str,
    covered_spec_id: &str,
    pr_number: u64,
    covered_status: &str,
) -> (String, String) {
    let storage = Storage::new(store_path);
    let mut store = storage.load().unwrap_or_default();

    let mut covered =
        aida_core::Requirement::new(format!("test-{}", covered_spec_id), String::new());
    covered.spec_id = Some(covered_spec_id.to_string());
    covered.set_status_from_str(covered_status);
    if matches!(covered.status, RequirementStatus::Completed) {
        let info = covered
            .implementation_info
            .get_or_insert_with(aida_core::ImplementationInfo::default);
        info.completed_at = Some(chrono::Utc::now());
        info.completion_sha = Some("deadbeefcafef00ddeadbeefcafef00ddeadbeef".to_string());
    }
    let covered_uuid = covered.id;
    store.requirements.push(covered);

    let mut review = aida_core::Requirement::new(
        format!("Review PR-{}: covers {}", pr_number, covered_spec_id),
        String::new(),
    );
    review.spec_id = Some(review_spec_id.to_string());
    review.req_type = aida_core::RequirementType::Story;
    review.set_status_from_str("Done");
    review.relationships.push(aida_core::Relationship {
        rel_type: aida_core::RelationshipType::Custom("implements".to_string()),
        target_id: covered_uuid,
        created_at: None,
        created_by: None,
    });
    store.requirements.push(review);
    storage.save(&store).unwrap();
    (review_spec_id.to_string(), covered_spec_id.to_string())
}

/// BUG-113: a review story stranded at `Done` — its PR merged, but the
/// merge commit carried the *covered* spec's `(REQ-ID)` and no `(#N)`
/// suffix, so the BUG-102/BUG-106 `pr_to_sha` linkage never saw it.
/// The covers chain must still graduate the review story in the same
// pass that completes its covered spec. trace:BUG-113 | ai:claude
#[test]
fn auto_bump_flips_review_story_via_covers_chain() {
    let (_tmp, project_root, store_path) = init_test_project();
    let (review_id, covered_id) =
        seed_covers_chain_review_story(&store_path, "STORY-9501", "STORY-9502", 4601, "done");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    // Merge commit references the covered spec only — no `(#N)`
    // suffix, so `pr_to_sha` stays empty and the review-story PR
    // linkage cannot fire. The covers chain is the only path that
    // can reach STORY-9501.
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &[
            "commit",
            "-m",
            &format!("feat: ship the work ({})", covered_id),
        ],
    );
    let merge_sha = run_git(&project_root, &["rev-parse", "HEAD"]);

    let storage = Storage::new(store_path.clone());
    let flips =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    assert!(
        has_flip(&flips, &review_id),
        "review story should flip via the covers chain, got: {:?}",
        flips
    );
    let after = storage.load().unwrap();
    let review = after.get_requirement_by_spec_id(&review_id).unwrap();
    assert!(
        matches!(review.status, RequirementStatus::Completed),
        "{} should be Completed via the covers chain, was {:?}",
        review_id,
        review.status
    );
    // The covered spec flipped too (the ordinary candidate scan), and
    // the review story inherited that spec's merge commit sha.
    let covered = after.get_requirement_by_spec_id(&covered_id).unwrap();
    assert!(
        matches!(covered.status, RequirementStatus::Completed),
        "covered spec should be Completed, was {:?}",
        covered.status
    );
    assert_eq!(
        review
            .implementation_info
            .as_ref()
            .and_then(|i| i.completion_sha.as_deref()),
        Some(merge_sha.as_str()),
        "review story completion_sha should be the covered spec's merge commit"
    );
}

/// BUG-113: `aida db reconcile-status --spec STORY-N` on a review
/// story stuck at `Done` — its covered spec is already `Completed`,
/// but the `(#N)` merge commit is outside the replay window so the
/// BUG-102 PR linkage can't fire. The covers chain must still graduate
// the stuck review story. trace:BUG-113 | ai:claude
#[test]
fn reconcile_status_flips_stuck_review_story_via_covers_chain() {
    let (_tmp, _project_root, store_path) = init_test_project();
    let (review_id, _covered_id) =
        seed_covers_chain_review_story(&store_path, "STORY-9913", "STORY-9914", 88, "completed");

    // No commit carries PR-88's `(#88)` suffix — the covers chain is
    // the only path that can reach STORY-9913.
    let r = handle_db_reconcile_status(&store_path, None, Some(&review_id), false);
    assert!(r.is_ok(), "reconcile-status failed: {:?}", r.err());

    let storage = Storage::new(store_path.clone());
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&review_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "reconcile-status --spec should flip the stuck review story via \
             the covers chain, was {:?}",
        req.status
    );
    assert_eq!(
        req.implementation_info
            .as_ref()
            .and_then(|i| i.completion_sha.as_deref()),
        Some("deadbeefcafef00ddeadbeefcafef00ddeadbeef"),
        "review story should inherit the covered spec's merge sha"
    );
}

/// BUG-106: a cluster-mode PR squash-merges with the PR TITLE as the
/// commit subject — the covered specs' IDs never reach main's commit
/// log. Auto-bump must follow the `(#N)` → review-story →
/// `implements` linkage and flip every covered Done spec.
// trace:BUG-106 | ai:claude
#[test]
fn auto_bump_flips_cluster_pr_covered_specs_via_review_story() {
    let (_tmp, project_root, store_path) = init_test_project();
    let (review_id, covered) = seed_cluster_pr_review_story(
        &store_path,
        "STORY-9301",
        77,
        &["TASK-9301", "TASK-9302", "BUG-9303"],
    );

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    // Squash-merge subject: PR title + `(#N)` ONLY — no covered
    // spec IDs, exactly as a real cluster-PR squash-merge looks.
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &[
            "commit",
            "-m",
            "Cluster drain: polish + fixes (3 specs) (#77)",
        ],
    );
    let merge_sha = run_git(&project_root, &["rev-parse", "HEAD"]);

    let storage = Storage::new(store_path.clone());
    let flips =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    // All three covered specs flip — none were in the commit subject.
    for spec in &covered {
        assert!(
            has_flip(&flips, spec),
            "covered spec {} should flip via PR linkage, got: {:?}",
            spec,
            flips
        );
    }
    let after = storage.load().unwrap();
    for spec in &covered {
        let req = after.get_requirement_by_spec_id(spec).unwrap();
        assert!(
            matches!(req.status, RequirementStatus::Completed),
            "{} should be Completed, was {:?}",
            spec,
            req.status
        );
        assert_eq!(
            req.implementation_info
                .as_ref()
                .and_then(|i| i.completion_sha.as_deref()),
            Some(merge_sha.as_str()),
            "{} completion_sha should match the merge commit",
            spec
        );
    }
    // The review story itself also flips (BUG-102 path) — unchanged.
    assert!(
        has_flip(&flips, &review_id),
        "review story should still flip via the BUG-102 path"
    );
}

/// BUG-106: a `(#N)` merge with no review story for PR-N must not
/// crash and must flip nothing — the PR-linkage path finds nothing.
// trace:BUG-106 | ai:claude
#[test]
fn auto_bump_cluster_pr_without_review_story_is_noop() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_done_spec(&store_path, "STORY-9401");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    // `(#88)` present but no review story encodes PR-88.
    run_git(&project_root, &["commit", "-m", "Some merge (#88)"]);

    let storage = Storage::new(store_path.clone());
    let flips =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();
    assert!(
        flips.is_empty(),
        "no review story for PR-88 → nothing flips, got: {:?}",
        flips
    );
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Done),
        "spec stays Done when its PR has no review story"
    );
}

/// TASK-246: seed an In-Progress review story with a
/// `Review PR-<n>: <suffix>` title — the state a reviewer leaves it
// in when requesting fixups. trace:TASK-246 | ai:claude
fn seed_inprogress_review_story(
    store_path: &std::path::Path,
    spec_id: &str,
    pr_number: u64,
    suffix: &str,
) -> String {
    let storage = Storage::new(store_path);
    let mut store = storage.load().unwrap_or_default();
    let mut req = aida_core::Requirement::new(
        format!("Review PR-{}: {}", pr_number, suffix),
        String::new(),
    );
    req.spec_id = Some(spec_id.to_string());
    req.req_type = aida_core::RequirementType::Story;
    req.set_status_from_str("in-progress");
    store.requirements.push(req);
    storage.save(&store).unwrap();
    spec_id.to_string()
}

/// TASK-246: a review story left at In Progress when the user
/// self-merges the PR (no fresh /aida-review iteration) flips to
/// Completed on the next `aida pull`, with an audit comment.
// trace:TASK-246 | ai:claude
#[test]
fn auto_bump_completes_inprogress_review_story_on_self_merge() {
    let (_tmp, project_root, store_path) = init_test_project();
    let review_id = seed_inprogress_review_story(&store_path, "STORY-9501", 91, "fixup iteration");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(&project_root, &["commit", "-m", "Fixups land (#91)"]);
    let merge_sha = run_git(&project_root, &["rev-parse", "HEAD"]);

    let storage = Storage::new(store_path.clone());
    auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&review_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "In-Progress review story should auto-complete on merge, was {:?}",
        req.status
    );
    assert_eq!(
        req.implementation_info
            .as_ref()
            .and_then(|i| i.completion_sha.as_deref()),
        Some(merge_sha.as_str()),
    );
    assert!(
        req.comments
            .iter()
            .any(|c| c.content.contains("without a re-review iteration")),
        "an audit comment should be recorded on the auto-completed story"
    );
}

/// TASK-246: an In-Progress review story whose PR has NOT merged is
/// left untouched — only the `(#N)` merge signal triggers the flip,
/// so an active review iteration is never stomped.
// trace:TASK-246 | ai:claude
#[test]
fn auto_bump_leaves_inprogress_review_story_alone_without_merge() {
    let (_tmp, project_root, store_path) = init_test_project();
    let review_id = seed_inprogress_review_story(&store_path, "STORY-9601", 92, "still reviewing");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    // A commit with NO `(#N)` suffix — the PR has not merged.
    std::fs::write(project_root.join("file.txt"), "wip\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(&project_root, &["commit", "-m", "chore: unrelated work"]);

    let storage = Storage::new(store_path.clone());
    auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&review_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::InProgress),
        "review story stays In Progress until its PR merges, was {:?}",
        req.status
    );
}

/// BUG-219: seed a review story at an arbitrary status with a
/// `Review PR-<n>: <suffix>` title — the shape /aida-pr's auto-queue
// files. trace:BUG-219 | ai:claude
fn seed_review_story_at(
    store_path: &std::path::Path,
    spec_id: &str,
    pr_number: u64,
    suffix: &str,
    status: &str,
) -> String {
    let storage = Storage::new(store_path);
    let mut store = storage.load().unwrap_or_default();
    let mut req = aida_core::Requirement::new(
        format!("Review PR-{}: {}", pr_number, suffix),
        String::new(),
    );
    req.spec_id = Some(spec_id.to_string());
    req.req_type = aida_core::RequirementType::Story;
    req.set_status_from_str(status);
    store.requirements.push(req);
    storage.save(&store).unwrap();
    spec_id.to_string()
}

/// BUG-219: a review story left at Approved when the user self-merges
/// the PR (no reviewer session was ever spawned) flips to Completed
/// on the next `aida pull`, with an audit comment that names the
// skipped review. trace:BUG-219 | ai:claude
#[test]
fn auto_bump_completes_approved_review_story_on_self_merge() {
    let (_tmp, project_root, store_path) = init_test_project();
    let review_id = seed_review_story_at(&store_path, "STORY-9801", 81, "self-merged", "approved");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(&project_root, &["commit", "-m", "feat: ship work (#81)"]);
    let merge_sha = run_git(&project_root, &["rev-parse", "HEAD"]);

    let storage = Storage::new(store_path.clone());
    auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&review_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "Approved review story should auto-complete on merge, was {:?}",
        req.status
    );
    assert_eq!(
        req.implementation_info
            .as_ref()
            .and_then(|i| i.completion_sha.as_deref()),
        Some(merge_sha.as_str()),
    );
    assert!(
        req.comments
            .iter()
            .any(|c| c.content.contains("without a reviewer session")),
        "an audit comment naming the skipped reviewer session should be recorded"
    );
}

/// BUG-219: an Approved review story whose PR has NOT merged is left
/// untouched — only the `(#N)` merge signal triggers the flip, so a
// genuinely-pending review is never stomped. trace:BUG-219 | ai:claude
#[test]
fn auto_bump_leaves_approved_review_story_alone_without_merge() {
    let (_tmp, project_root, store_path) = init_test_project();
    let review_id = seed_review_story_at(&store_path, "STORY-9901", 82, "queued", "approved");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    // A commit with NO `(#N)` suffix — the PR has not merged.
    std::fs::write(project_root.join("file.txt"), "wip\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(&project_root, &["commit", "-m", "chore: unrelated work"]);

    let storage = Storage::new(store_path.clone());
    auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&review_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Approved),
        "review story stays Approved until its PR merges, was {:?}",
        req.status
    );
}

/// BUG-219 acceptance regression: 4 review stories at Approved (the
/// observed PR-52/53/54/56 case — `--auto-complete` shipped them,
/// the orchestrator failed before spawning a reviewer), all 4 PRs
/// merged, a single `aida pull` flips every one to Completed.
// trace:BUG-219 | ai:claude
#[test]
fn auto_bump_completes_four_approved_review_stories_in_one_pass() {
    let (_tmp, project_root, store_path) = init_test_project();
    let prs: [(u64, &str); 4] = [
        (52, "STORY-9701"),
        (53, "STORY-9702"),
        (54, "STORY-9703"),
        (56, "STORY-9704"),
    ];
    for (pr_n, sid) in prs {
        seed_review_story_at(&store_path, sid, pr_n, "self-merged", "approved");
    }

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    // One squash-merge commit per PR landing on the default branch.
    for (pr_n, _) in prs {
        std::fs::write(project_root.join("file.txt"), format!("pr{}\n", pr_n)).unwrap();
        run_git(&project_root, &["add", "file.txt"]);
        run_git(
            &project_root,
            &["commit", "-m", &format!("feat: batch work (#{})", pr_n)],
        );
    }

    let storage = Storage::new(store_path.clone());
    auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    let after = storage.load().unwrap();
    for (pr_n, sid) in prs {
        let req = after.get_requirement_by_spec_id(sid).unwrap();
        assert!(
            matches!(req.status, RequirementStatus::Completed),
            "{} (PR #{}) should auto-complete in the same pass, was {:?}",
            sid,
            pr_n,
            req.status
        );
        assert!(
            req.comments
                .iter()
                .any(|c| c.content.contains("without a reviewer session")),
            "{} should carry the self-merge audit comment",
            sid
        );
    }
}

/// BUG-219: `aida db reconcile-status` — the manual replay tool —
/// also flips an Approved review story whose PR merged, so the
/// safety net covers the same case as the pull-time auto-bump.
// trace:BUG-219 | ai:claude
#[test]
fn reconcile_status_completes_approved_review_story() {
    let (_tmp, project_root, store_path) = init_test_project();
    let review_id = seed_review_story_at(&store_path, "STORY-9951", 71, "self-merged", "approved");

    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(&project_root, &["commit", "-m", "feat: ship work (#71)"]);

    let r = handle_db_reconcile_status(&store_path, None, Some(&review_id), false);
    assert!(r.is_ok(), "reconcile-status failed: {:?}", r.err());

    let storage = Storage::new(store_path.clone());
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&review_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "reconcile-status should flip an Approved review story whose PR merged, was {:?}",
        req.status
    );
    assert!(
        req.comments
            .iter()
            .any(|c| c.content.contains("without a reviewer session")),
        "reconcile-status should record the audit comment too"
    );
}

/// TASK-1-113: the reconcile-status APPLY path must match `agreed_id`,
/// not just `spec_id`. A node-aware spec stores the canonical id
/// (FR-1-042) but is referenced in commit subjects by its agreed id
/// (FR-42). The candidate/dry-run path uses `get_requirement_by_spec_id`
/// (agreed-aware) and reported "would flip"; the apply write-loop matched
/// only `r.spec_id` and silently skipped every agreed≠canonical spec —
// so dry-run and apply diverged. trace:TASK-1-113 | ai:claude
#[test]
fn reconcile_status_flips_node_aware_spec_by_agreed_id() {
    let (_tmp, project_root, store_path) = init_test_project();
    seed_done_spec(&store_path, "FR-1-042");
    {
        // Give it an agreed id distinct from its canonical spec_id.
        let storage = Storage::new(store_path.clone());
        let mut store = storage.load().unwrap();
        let r = store
            .requirements
            .iter_mut()
            .find(|r| r.spec_id.as_deref() == Some("FR-1-042"))
            .unwrap();
        r.agreed_id = Some("FR-42".to_string());
        storage.save(&store).unwrap();
    }

    // A commit references the spec by its AGREED id.
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &["commit", "-m", "feat: implement thing (FR-42)"],
    );

    let r = handle_db_reconcile_status(&store_path, None, None, false);
    assert!(r.is_ok(), "reconcile-status failed: {:?}", r.err());

    let storage = Storage::new(store_path.clone());
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id("FR-42").unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "reconcile-status must flip a node-aware spec matched by agreed_id \
             (FR-42 → FR-1-042); was {:?}",
        req.status
    );
}

/// BUG-94 acceptance check: `aida db sync --pull` calls
/// `auto_bump_done_to_completed` with `pre_sha = None`. The helper's
/// HEAD~50 fallback range must catch any spec-referencing commit
/// that landed on the code branch (typically from a separate
/// `git pull` the user ran before this command). Verifies the
/// "scan range collapses to empty" claim is fixed/non-reproducible:
/// with the fallback range, the bump still fires.
// trace:BUG-94 | ai:claude
#[test]
fn auto_bump_with_none_pre_sha_uses_head_50_fallback() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_done_spec(&store_path, "STORY-9201");

    // Simulate a teammate's merge landing on the default branch.
    // The user has separately `git pull`'d, so the merge is in the
    // local code-branch history. They now run `aida db sync --pull`,
    // which calls auto_bump with pre_sha = None.
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &[
            "commit",
            "-m",
            &format!("feat: teammate merge ({})", spec_id),
        ],
    );

    let storage = Storage::new(store_path.clone());
    let flips = auto_bump_done_to_completed(&project_root, &store_path, None, &storage).unwrap();

    assert!(
        has_flip(&flips, &spec_id),
        "BUG-94: HEAD~50 fallback should catch the merge, got: {:?}",
        flips
    );
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "spec should be Completed after fallback-range bump, was {:?}",
        req.status
    );
}

/// BUG-102: when no `(#N)` suffix is present in any subject, review
/// stories stay untouched (no false-positives from PR-numbered titles
/// that happen to match an unrelated number elsewhere in the repo).
// trace:BUG-102 | ai:claude
#[test]
fn auto_bump_leaves_review_story_alone_without_pr_suffix() {
    let (_tmp, project_root, store_path) = init_test_project();
    let review_id = seed_done_review_story(&store_path, "STORY-9102", 99, "nothing should fire");

    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    // Subject has spec parens but no (#N) trailer (e.g. direct push,
    // not a squash-merge). Review story must NOT flip.
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &["commit", "-m", "chore: direct push (TASK-1)"],
    );

    let storage = Storage::new(store_path.clone());
    let _ =
        auto_bump_done_to_completed(&project_root, &store_path, Some(&pre_sha), &storage).unwrap();

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&review_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Done),
        "review story should still be Done, was {:?}",
        req.status
    );
}

/// TASK-226: reconcile-status replays the same scan as the pull-time
/// auto-bump but over a wider, user-bounded range. Verifies the
/// recovery path for a spec whose YAML was unreadable at pull time:
/// the merge commit is already on local main (so pull's scan window
/// has moved past it), but reconcile-status still finds and flips it.
// trace:TASK-226 | ai:claude
#[test]
fn reconcile_status_replays_missed_bump() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_done_spec(&store_path, "STORY-9601");

    // Land a commit referencing the spec on main. Imagine pull-time
    // auto-bump missed it because the YAML was unreadable.
    std::fs::write(project_root.join("file.txt"), "land\n").unwrap();
    run_git(&project_root, &["add", "file.txt"]);
    run_git(
        &project_root,
        &["commit", "-m", &format!("feat: x ({})", spec_id)],
    );

    let r = handle_db_reconcile_status(&store_path, None, None, false);
    assert!(r.is_ok(), "reconcile-status failed: {:?}", r.err());

    let storage = Storage::new(store_path.clone());
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "spec should be Completed after reconcile, was {:?}",
        req.status
    );
    let info = req.implementation_info.as_ref().expect("info populated");
    assert!(info.completed_at.is_some());
    assert!(info.completion_sha.is_some());
}

/// BUG-418: when a spec's referencing commit IS on the default branch but
/// the spec is already `Completed` (a prior reconcile/pull graduated it),
/// the no-flip message must say "already Completed — nothing to do", NOT
/// the misleading "no commit references it" text that reads as a failed
/// recovery. The store state is correctly untouched either way; this
// guards the OUTPUT, which is the whole of the bug. trace:BUG-418
#[test]
fn reconcile_status_already_completed_says_so_not_no_match() {
    // --spec form: a referencing commit landed, spec already Completed.
    let terminal = vec![("SPIKE-46".to_string(), RequirementStatus::Completed)];
    let msg = reconcile_no_flip_message(Some("SPIKE-46"), &terminal);
    assert!(
        msg.contains("already Completed"),
        "should name the already-Completed state, got: {msg}"
    );
    assert!(
        !msg.contains("no commit"),
        "must NOT claim no commit references it — that's the misleading \
             BUG-418 output. got: {msg}"
    );

    // Full-scan form (no --spec): same disambiguation.
    let msg_all = reconcile_no_flip_message(None, &terminal);
    assert!(
        msg_all.contains("already Completed"),
        "full-scan no-flip should still surface already-Completed, got: {msg_all}"
    );

    // Genuine no-match (nothing terminal, nothing flipped): keep the old
    // guidance so we don't paper over a real "nothing referenced it" case.
    let msg_none = reconcile_no_flip_message(Some("STORY-9999"), &[]);
    assert!(
        msg_none.contains("No eligible flips"),
        "true no-match must keep the no-eligible-flips guidance, got: {msg_none}"
    );
}

/// BUG-418 (end-to-end): drive `handle_db_reconcile_status` against a spec
/// already `Completed` whose merge commit references it; the run must
/// succeed (no error, no state change) — the regression is in the message
// path, exercised by the unit test above. trace:BUG-418
#[test]
fn reconcile_status_completed_spec_with_ref_is_noop_ok() {
    let (_tmp, project_root, store_path) = init_test_project();
    seed_spec_at(&store_path, "SPIKE-9646", "Completed");

    std::fs::write(project_root.join("done.txt"), "x\n").unwrap();
    run_git(&project_root, &["add", "done.txt"]);
    run_git(&project_root, &["commit", "-m", "feat: ship (SPIKE-9646)"]);

    let r = handle_db_reconcile_status(&store_path, None, Some("SPIKE-9646"), false);
    assert!(
        r.is_ok(),
        "reconcile of already-Completed spec failed: {:?}",
        r.err()
    );

    let storage = Storage::new(store_path.clone());
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id("SPIKE-9646").unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "spec should stay Completed, was {:?}",
        req.status
    );
}

/// BUG-328: `aida db reconcile-status` uses the same expanded direct
/// candidate rules as pull-time auto-bump.
// trace:BUG-328 | ai:codex
#[test]
fn reconcile_status_eligibility_matrix_for_direct_subject_refs() {
    let cases = [
        ("STORY-9611", "Approved", true),
        ("STORY-9612", "Planned", true),
        ("STORY-9613", "In Progress", true),
        ("STORY-9614", "Done", true),
        ("STORY-9615", "Draft", false),
        ("STORY-9616", "Completed", false),
        ("STORY-9617", "Rejected", false),
    ];
    for (spec_id, status, should_flip) in cases {
        let (_tmp, project_root, store_path) = init_test_project();
        seed_spec_at(&store_path, spec_id, status);

        std::fs::write(project_root.join("file.txt"), format!("land {spec_id}\n")).unwrap();
        run_git(&project_root, &["add", "file.txt"]);
        run_git(
            &project_root,
            &["commit", "-m", &format!("feat: reconcile ({})", spec_id)],
        );

        handle_db_reconcile_status(&store_path, None, Some(spec_id), false).unwrap();

        let storage = Storage::new(store_path.clone());
        let after = storage.load().unwrap();
        let req = after.get_requirement_by_spec_id(spec_id).unwrap();
        if should_flip {
            assert!(
                matches!(req.status, RequirementStatus::Completed),
                "{} should be Completed, was {:?}",
                status,
                req.status
            );
            assert!(
                req.implementation_info
                    .as_ref()
                    .and_then(|i| i.completion_sha.as_deref())
                    .is_some(),
                "{} should stamp completion_sha",
                status
            );
        } else {
            assert_eq!(
                req.status.to_string(),
                status,
                "{} should not reconcile-bump",
                status
            );
        }
    }
}

/// TASK-226: --dry-run reports the planned flips without writing.
// trace:TASK-226 | ai:claude
#[test]
fn reconcile_status_dry_run_does_not_write() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_done_spec(&store_path, "STORY-9602");

    std::fs::write(project_root.join("f.txt"), "x\n").unwrap();
    run_git(&project_root, &["add", "f.txt"]);
    run_git(
        &project_root,
        &["commit", "-m", &format!("feat: y ({})", spec_id)],
    );

    let r = handle_db_reconcile_status(&store_path, None, None, true);
    assert!(r.is_ok(), "dry-run failed: {:?}", r.err());

    let storage = Storage::new(store_path.clone());
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Done),
        "spec should still be Done after dry-run, was {:?}",
        req.status
    );
}

/// TASK-226: --spec narrows the candidate set to a single requirement.
/// Other Done specs (with referencing commits) stay untouched.
// trace:TASK-226 | ai:claude
#[test]
fn reconcile_status_spec_filter_narrows_candidates() {
    let (_tmp, project_root, store_path) = init_test_project();
    let target = seed_done_spec(&store_path, "STORY-9603");
    let bystander = seed_done_spec(&store_path, "STORY-9604");

    // Both specs have referencing commits.
    std::fs::write(project_root.join("a.txt"), "x\n").unwrap();
    run_git(&project_root, &["add", "a.txt"]);
    run_git(
        &project_root,
        &["commit", "-m", &format!("feat: a ({})", target)],
    );
    std::fs::write(project_root.join("b.txt"), "x\n").unwrap();
    run_git(&project_root, &["add", "b.txt"]);
    run_git(
        &project_root,
        &["commit", "-m", &format!("feat: b ({})", bystander)],
    );

    // Narrow to just `target` — bystander must NOT flip.
    let r = handle_db_reconcile_status(&store_path, None, Some(&target), false);
    assert!(r.is_ok(), "reconcile failed: {:?}", r.err());

    let storage = Storage::new(store_path.clone());
    let after = storage.load().unwrap();
    let target_req = after.get_requirement_by_spec_id(&target).unwrap();
    let bystander_req = after.get_requirement_by_spec_id(&bystander).unwrap();
    assert!(matches!(target_req.status, RequirementStatus::Completed));
    assert!(
        matches!(bystander_req.status, RequirementStatus::Done),
        "bystander should still be Done, was {:?}",
        bystander_req.status
    );
}

/// TASK-226: a spec already at Completed is a no-op (no double-write,
/// no error). This is the idempotency contract — running the
// command twice in a row should be safe. trace:TASK-226 | ai:claude
#[test]
fn reconcile_status_idempotent_on_completed_specs() {
    let (_tmp, project_root, store_path) = init_test_project();
    let spec_id = seed_done_spec(&store_path, "STORY-9605");
    std::fs::write(project_root.join("f.txt"), "x\n").unwrap();
    run_git(&project_root, &["add", "f.txt"]);
    run_git(
        &project_root,
        &["commit", "-m", &format!("feat: z ({})", spec_id)],
    );

    // First run flips.
    handle_db_reconcile_status(&store_path, None, None, false).unwrap();
    // Second run should be a clean no-op (the spec is now Completed).
    let r = handle_db_reconcile_status(&store_path, None, None, false);
    assert!(r.is_ok(), "second reconcile failed: {:?}", r.err());

    let storage = Storage::new(store_path.clone());
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(matches!(req.status, RequirementStatus::Completed));
}

/// TASK-226: refuses to run on a non-default branch — same guard as
/// the pull-time auto-bump, since the scan window only makes sense
// for default-branch history. trace:TASK-226 | ai:claude
#[test]
fn reconcile_status_refuses_non_default_branch() {
    let (_tmp, project_root, store_path) = init_test_project();
    let _ = seed_done_spec(&store_path, "STORY-9606");
    run_git(&project_root, &["checkout", "-b", "feature/x"]);
    let r = handle_db_reconcile_status(&store_path, None, None, false);
    assert!(r.is_err(), "expected error on feature branch, got Ok");
}

// ────────────────────────────────────────────────────────────────────
// TASK-1161: on a git-canonical store, the auto-bump writes each
// bumped spec as its OWN targeted commit (subject `update SPEC-ID`)
// via the BUG-634 targeted path — no bulk "chore: update N
// requirements" full-store commit.
// ────────────────────────────────────────────────────────────────────

/// Build a temp project whose store is a git-canonical DIRECTORY
/// (its own git repo), mirroring the distributed `.aida-store/`
/// layout, instead of the YAML file the other tests use.
/// Returns `(temp_dir_guard, project_root, store_dir)`.
// trace:TASK-1161 | ai:claude
fn init_git_canonical_test_project() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf)
{
    let tmp = tempfile::TempDir::new().unwrap();
    let project_root = tmp.path().to_path_buf();

    // Code repo init + identity + an initial commit so HEAD resolves.
    run_git(&project_root, &["init", "--initial-branch=main", "--quiet"]);
    run_git(&project_root, &["config", "user.email", "test@example.com"]);
    run_git(&project_root, &["config", "user.name", "Test"]);
    std::fs::write(project_root.join("README.md"), "init\n").unwrap();
    run_git(&project_root, &["add", "README.md"]);
    run_git(&project_root, &["commit", "-m", "chore: init"]);

    // Store dir as its own git repo (the orphan-branch worktree shape).
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

// TASK-1161: two Done specs land via commits on the default branch; the
// auto-bump against a git-canonical store must produce one `update SPEC-ID`
// commit per bumped spec in the store repo and NO bulk
// "chore: update N requirements" commit. trace:TASK-1161 | ai:claude
#[test]
fn auto_bump_git_canonical_store_writes_targeted_commits_per_spec() {
    use aida_core::db::DatabaseBackend;

    let (_tmp, project_root, store_dir) = init_git_canonical_test_project();

    // Seed two Done specs straight into the git-canonical store.
    let backend = aida_core::db::GitBackend::new(&store_dir).unwrap();
    let mut store = aida_core::RequirementsStore::default();
    for spec_id in ["STORY-9701", "STORY-9702"] {
        let mut req = aida_core::Requirement::new(format!("test-{}", spec_id), String::new());
        req.spec_id = Some(spec_id.to_string());
        req.set_status_from_str("Done");
        store.requirements.push(req);
    }
    backend.save(&store).unwrap();
    let seed_head = run_git(&store_dir, &["rev-parse", "HEAD"]);

    // Land one code commit per spec on the default branch.
    let pre_sha = aida_core::git_ops::head_sha(&project_root).unwrap();
    for (file, spec_id) in [("a.txt", "STORY-9701"), ("b.txt", "STORY-9702")] {
        std::fs::write(project_root.join(file), "land\n").unwrap();
        run_git(&project_root, &["add", file]);
        run_git(
            &project_root,
            &["commit", "-m", &format!("feat: land ({})", spec_id)],
        );
    }

    let storage = Storage::new(store_dir.clone());
    let flips =
        auto_bump_done_to_completed(&project_root, &store_dir, Some(&pre_sha), &storage).unwrap();
    assert_eq!(flips.len(), 2, "both specs should flip: {:?}", flips);

    // Both flipped to Completed on disk.
    let after = storage.load().unwrap();
    for spec_id in ["STORY-9701", "STORY-9702"] {
        let req = after.get_requirement_by_spec_id(spec_id).unwrap();
        assert!(
            matches!(req.status, RequirementStatus::Completed),
            "{} should be Completed, was {:?}",
            spec_id,
            req.status
        );
    }

    // The store repo gained one targeted commit PER bumped spec — subject
    // `update SPEC-ID` — and no bulk chore commit.
    let new_subjects = run_git(
        &store_dir,
        &["log", "--format=%s", &format!("{}..HEAD", seed_head)],
    );
    let subjects: Vec<&str> = new_subjects.lines().collect();
    assert!(
        subjects.contains(&"update STORY-9701"),
        "expected `update STORY-9701` commit, got: {:?}",
        subjects
    );
    assert!(
        subjects.contains(&"update STORY-9702"),
        "expected `update STORY-9702` commit, got: {:?}",
        subjects
    );
    assert!(
        !subjects.iter().any(|s| s.starts_with("chore: update")),
        "no bulk chore commit expected, got: {:?}",
        subjects
    );
}

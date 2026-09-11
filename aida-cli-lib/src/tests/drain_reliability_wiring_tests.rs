//! TASK-136 / BUG-420: the I/O-shell wiring around the pure decision cores
//! (`gh_verify_backoff_schedule`, `watchdog_verdict`). The pure cores are
//! tested in `auto_complete`; here we lock the config parsing, the watchdog
//! trip-reason text, and the worktree progress signature.
use super::*;

#[test]
fn read_drain_config_parses_drain_section() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        "[node]\nid = \"x\"\n\n[drain]\ngh_verify_retries = 2  # transient blips\n\
             no_progress_minutes = 3\nphase_ceiling_minutes = 20\nci_auto_fix = 2\n\
             retry_transient = 3\n",
    )
    .unwrap();
    let cfg = read_drain_config(tmp.path());
    assert_eq!(cfg.gh_verify_retries, Some(2));
    assert_eq!(cfg.no_progress_minutes, Some(3));
    assert_eq!(cfg.phase_ceiling_minutes, Some(20));
    // trace:TASK-975 | ai:claude
    assert_eq!(cfg.ci_auto_fix, Some(2));
    // trace:STORY-975 | ai:codex
    assert_eq!(cfg.retry_transient, Some(3));
}

#[test]
fn read_drain_config_absent_section_is_all_none() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(tmp.path().join(".aida/config.toml"), "[node]\nid = \"x\"\n").unwrap();
    let cfg = read_drain_config(tmp.path());
    assert_eq!(cfg.gh_verify_retries, None);
    assert_eq!(cfg.no_progress_minutes, None);
    assert_eq!(cfg.phase_ceiling_minutes, None);
    // trace:TASK-975 | ai:claude — default OFF: red CI shelves immediately.
    assert_eq!(cfg.ci_auto_fix, None);
    // trace:STORY-975 | ai:codex
    assert_eq!(cfg.retry_transient, None);
}

/// TASK-975: the CI-fix prompt's contract lines — spec context, the failing
/// log, push-to-the-existing-branch, and the no-new-PR / exit-without-push
/// rules — are pinned so a reword can't silently drop a guard.
// trace:TASK-975 | ai:claude
#[test]
fn ci_fix_prompt_carries_context_and_guardrails() {
    let prompt = build_ci_fix_prompt(
        "TASK-9",
        "task-9-fix",
        "CI is red on PR-46: clippy failed",
        "error: unused variable `x`",
    );
    assert!(prompt.contains("aida show TASK-9 --full"));
    assert!(prompt.contains("CI is red on PR-46: clippy failed"));
    assert!(prompt.contains("error: unused variable `x`"));
    assert!(prompt.contains("git push origin task-9-fix"));
    assert!(prompt.contains("do NOT open a new PR"));
    assert!(prompt.contains("WITHOUT committing or pushing"));

    // No log fetched → the prompt says to fetch the checks itself.
    let no_log = build_ci_fix_prompt("TASK-9", "task-9-fix", "CI is red", "");
    assert!(no_log.contains("No failing-check log could be fetched"));
}

/// TASK-975: the log tail stays bounded and keeps the END of the log (where
/// the failure detail lives), marked as truncated.
// trace:TASK-975 | ai:claude
#[test]
fn ci_fix_log_tail_is_bounded_and_keeps_the_end() {
    let short = "a short log";
    assert_eq!(tail_bounded(short, 100), short);

    let long = format!("{}THE-ACTUAL-ERROR", "x".repeat(50_000));
    let tail = tail_bounded(&long, 1_000);
    assert!(tail.len() < 1_100);
    assert!(tail.ends_with("THE-ACTUAL-ERROR"));
    assert!(tail.starts_with("…(log truncated)…"));
}

#[test]
fn watchdog_trip_reason_names_the_threshold_minutes() {
    let wd = PhaseWatchdog::new(
        std::path::PathBuf::from("/tmp/nonexistent"),
        "sess".to_string(),
        std::time::Duration::from_secs(10 * 60),
        std::time::Duration::from_secs(45 * 60),
    );
    assert!(wd
        .trip_reason(auto_complete::WatchdogTrip::NoProgress)
        .contains("10m"));
    assert!(wd
        .trip_reason(auto_complete::WatchdogTrip::Ceiling)
        .contains("45m"));
}

// trace:BUG-875 | ai:codex
#[test]
fn watchdog_progress_signal_is_phase_specific() {
    use auto_complete::Phase;

    assert_eq!(
        watchdog_progress_signal_for_phase(Phase::Implementer),
        WatchdogProgressSignal::WorktreeAndOutput,
    );
    assert_eq!(
        watchdog_progress_signal_for_phase(Phase::Reviewer),
        WatchdogProgressSignal::OutputOnly,
    );

    assert_eq!(
        PhaseWatchdog::select_progress_signature(
            WatchdogProgressSignal::OutputOnly,
            Some("worktree-changed".into()),
            Some("log:10:20".into()),
        ),
        Some("log:10:20".into()),
        "reviewer watchdog progress must ignore worktree-only movement",
    );
    assert_eq!(
        PhaseWatchdog::select_progress_signature(
            WatchdogProgressSignal::WorktreeAndOutput,
            Some("worktree-changed".into()),
            Some("log:10:20".into()),
        ),
        Some("worktree-changed|log:10:20".into()),
        "implementer watchdog progress keeps the combined signal",
    );
}

// trace:BUG-875 | ai:codex
#[test]
fn reviewer_watchdog_streaming_output_survives_but_silence_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let worktree = root.join("review-wt");
    std::fs::create_dir_all(&worktree).unwrap();
    let log_dir = root.join(".aida/headless-logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    let session_id = "review-session";
    let log = log_dir.join(format!("review-{session_id}.jsonl"));
    std::fs::write(&log, "{}\n").unwrap();

    let old_progress = std::time::Instant::now() - std::time::Duration::from_secs(20 * 60);
    let old_poll = std::time::Instant::now() - std::time::Duration::from_secs(31);
    let mut streaming = PhaseWatchdog::new_for_phase(
        root.to_path_buf(),
        session_id.to_string(),
        std::time::Duration::from_secs(10 * 60),
        std::time::Duration::from_secs(45 * 60),
        auto_complete::Phase::Reviewer,
    );
    streaming.worktree = Some(worktree.clone());
    streaming.last_progress = old_progress;
    streaming.last_poll = old_poll;
    streaming.last_sig = Some("log:0:0".into());

    assert_eq!(
        streaming.check(),
        None,
        "fresh reviewer stream output is progress even with no file changes",
    );

    let current_sig = headless_log_activity_signature(root, session_id).unwrap();
    let mut silent = PhaseWatchdog::new_for_phase(
        root.to_path_buf(),
        session_id.to_string(),
        std::time::Duration::from_secs(10 * 60),
        std::time::Duration::from_secs(45 * 60),
        auto_complete::Phase::Reviewer,
    );
    silent.worktree = Some(worktree);
    silent.last_progress = old_progress;
    silent.last_poll = old_poll;
    silent.last_sig = Some(current_sig);

    let reason = silent.check().expect("silent reviewer should trip");
    assert!(
        reason.contains("no session output for 10m"),
        "reviewer trip reason should name output silence, got {reason:?}",
    );
}

#[test]
fn resume_start_phase_clamp_bumps_ci_to_reviewer_only() {
    use auto_complete::Phase;
    // CI is the one unsafe re-entry (lease-coupled) → bumped to reviewer.
    assert_eq!(
        clamp_resume_start_phase(Phase::Ci),
        Phase::Reviewer,
        "a reconciled CI re-entry must clamp up to the reviewer",
    );
    // Every other phase is left exactly as reconciled.
    for p in [
        Phase::Implementer,
        Phase::Reviewer,
        Phase::Merge,
        Phase::Pull,
        Phase::Build,
    ] {
        assert_eq!(clamp_resume_start_phase(p), p);
    }
}

#[test]
fn probe_resume_facts_is_conservative_when_nothing_exists() {
    // No git / gh / store → every postcondition is conservatively false, so
    // reconcile would re-run from the start rather than skip a real phase.
    let tmp = tempfile::tempdir().unwrap();
    let storage = Storage::new(tmp.path().join("requirements.db"));
    let (facts, branch, pr) = probe_resume_facts(tmp.path(), &storage, "TASK-1", None);
    assert!(!facts.branch_exists);
    assert!(!facts.pr_merged);
    assert!(!facts.spec_completed);
    assert!(!facts.ci_green);
    assert!(!facts.reviewed);
    assert!(!facts.build_ok);
    assert_eq!(branch, None);
    assert_eq!(pr, None);
}

// trace:BUG-881 | ai:codex
// trace:BUG-1021 | ai:claude — the fake `gh` is a bash script; Windows can't exec it.
#[cfg(unix)]
#[test]
fn probe_resume_facts_resolves_open_pr_without_lease_from_forge_surface() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    let storage = Storage::new(root.join("requirements.db"));
    let fake_gh = root.join("gh");
    std::fs::write(
        &fake_gh,
        r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "--version" ]]; then
  echo "gh version 0.0.0"
  exit 0
fi
if [[ "${1:-}" == "pr" && "${2:-}" == "list" ]]; then
  if [[ "$*" == *"--search TASK-4"* ]]; then
    exit 0
  fi
  if [[ "$*" == *"--state open"* ]]; then
    printf '3\t[AI:codex] fix: shipped elsewhere (TASK-4)\thttps://example/pr/3\ttask-4\n'
    exit 0
  fi
fi
if [[ "${1:-}" == "pr" && "${2:-}" == "view" ]]; then
  if [[ "$*" == *"headRefName"* ]]; then
    echo "task-4"
    exit 0
  fi
  if [[ "$*" == *"commits"* ]]; then
    printf '[AI:codex] fix: shipped elsewhere (TASK-4)\n'
    exit 0
  fi
fi
exit 1
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fake_gh).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_gh, perms).unwrap();
    }

    let prev = std::env::var("AIDA_TEST_GH_BINARY").ok();
    std::env::set_var("AIDA_TEST_GH_BINARY", &fake_gh);
    let (facts, branch, pr) = probe_resume_facts(root, &storage, "TASK-4", None);
    match prev {
        Some(value) => std::env::set_var("AIDA_TEST_GH_BINARY", value),
        None => std::env::remove_var("AIDA_TEST_GH_BINARY"),
    }

    assert_eq!(pr, Some(3));
    assert_eq!(branch.as_deref(), Some("task-4"));
    assert!(
        facts.branch_exists,
        "an open PR should satisfy the branch-exists postcondition"
    );
}

#[test]
fn progress_signature_changes_when_a_file_is_edited_then_committed() {
    // A real worktree: an edit and a commit each move the signature, so the
    // no-progress timer resets; an idle worktree keeps it stable.
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(wt)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@t.t"]);
    git(&["config", "user.name", "t"]);
    std::fs::write(wt.join("a.txt"), "one").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "first"]);

    let sig1 = PhaseWatchdog::progress_signature(wt).expect("sig after first commit");
    // Idle: same signature.
    assert_eq!(
        sig1,
        PhaseWatchdog::progress_signature(wt).unwrap(),
        "an idle worktree must not register as progress",
    );
    // A new uncommitted edit changes the porcelain status → progress.
    std::fs::write(wt.join("b.txt"), "two").unwrap();
    let sig2 = PhaseWatchdog::progress_signature(wt).unwrap();
    assert_ne!(sig1, sig2, "an uncommitted edit is progress");
    // Committing it advances HEAD → progress again.
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "second"]);
    let sig3 = PhaseWatchdog::progress_signature(wt).unwrap();
    assert_ne!(sig2, sig3, "a new commit is progress");
}

#[test]
fn command_line_runs_aida_pr_ship_recognizes_direct_and_wrapped_forms() {
    // BUG-749: the phase watchdog uses this local command matcher to treat a
    // live `aida pr ship` CI-wait as progress without calling the forge.
    // trace:BUG-749 | ai:codex
    assert!(command_line_runs_aida_pr_ship(&[
        "/repo/target/debug/aida".into(),
        "pr".into(),
        "ship".into(),
        "1498".into(),
    ]));
    assert!(command_line_runs_aida_pr_ship(&[
        "bash".into(),
        "-lc".into(),
        "aida pr ship 1498".into(),
    ]));
    assert!(!command_line_runs_aida_pr_ship(&[
        "aida".into(),
        "pr".into(),
        "view".into(),
        "1498".into(),
    ]));
}

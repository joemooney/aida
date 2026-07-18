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
             no_progress_minutes = 3\nphase_ceiling_minutes = 20\n",
    )
    .unwrap();
    let cfg = read_drain_config(tmp.path());
    assert_eq!(cfg.gh_verify_retries, Some(2));
    assert_eq!(cfg.no_progress_minutes, Some(3));
    assert_eq!(cfg.phase_ceiling_minutes, Some(20));
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

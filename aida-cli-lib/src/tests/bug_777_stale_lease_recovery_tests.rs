//! BUG-777: recovering an orphaned lease that blocks `aida queue work`.
//!
//! Before this change, a lease minted by an ALREADY-EXITED session kept
//! owning the scope, and all three escapes the error suggested were bad in
//! that state (`--resume` had no session to re-attach to, `--steal` threatened
//! a worktree the operator could not evaluate, `aida session end` no-op'd
//! non-interactively). These tests pin the replacement contract:
//!
//!   * a lease whose owning process is verifiably gone + a CLEAN worktree is
//!     reclaimable — the one `--force-claim` verb takes it;
//!   * a lease whose owning process is LIVE is never reclaimable;
//!   * a gone-owner lease with a DIRTY worktree refuses and names what is
//!     uncommitted;
//!   * every refusal message states the worktree's clean/dirty state, so
//!     "work there is lost unless committed" is evaluable in place.
//!
//! Liveness is asserted against real pids (a reaped child = verifiably dead;
//! this test process = verifiably alive) and real `git status` readings, with
//! NO absolute timestamps anywhere — lease ages are computed relative to now.
//!
//! trace:BUG-777 | ai:claude
use super::*;

/// A worktree-backed implementer lease at `worktree`, minted `age_secs` ago
/// (relative to now — never a fixed wall-clock instant, which would rot into
/// a time bomb as staleness thresholds move).
fn lease_at(
    worktree: &std::path::Path,
    age_secs: i64,
    creator_pid: Option<u32>,
    active_pid: Option<u32>,
) -> SessionLease {
    SessionLease {
        id: "019f8357abcd".to_string(),
        scope: "TASK-5".to_string(),
        slug: "task-5".to_string(),
        owner: "u".into(),
        worktree_path: worktree.to_path_buf(),
        branch: "task-5-orphan".into(),
        started_at: chrono::Utc::now() - chrono::Duration::seconds(age_secs),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid,
        active_pid,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
    }
}

/// A pid that is verifiably NOT in the process table: spawn a trivial child,
/// wait for it to exit (so it is reaped, not a zombie), and hand back its pid.
/// Falls back to a high unlikely pid if spawning is unavailable.
fn reaped_pid() -> u32 {
    let mut child = match std::process::Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return 4_194_303,
    };
    let pid = child.id();
    let _ = child.wait();
    pid
}

/// An initialized git repo with no changes → `git status --porcelain` empty.
fn clean_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["init", "-q"])
        .status();
    assert!(ok.map(|s| s.success()).unwrap_or(false), "git init failed");
    dir
}

/// (a) The canonical BUG-777 state: the owning session exited and left a lease
/// behind over a clean worktree. That must classify as reclaimable, which is
/// exactly what lets `--force-claim` release it and take the scope.
#[test]
fn gone_owner_with_clean_worktree_is_reclaimable() {
    let repo = clean_repo();
    let lease = lease_at(repo.path(), 120, Some(reaped_pid()), None);
    let report = stale_lease_recovery_for_lease(&lease);
    assert_eq!(
        report.verdict,
        StaleLeaseRecovery::ReclaimableClean {
            worktree_missing: false
        },
        "a dead-owner lease over a clean worktree must be reclaimable"
    );
    assert!(report.dirty.is_empty());
}

/// The same state where the worktree itself was already removed — still
/// reclaimable, and flagged so the message can say there is nothing to lose.
#[test]
fn gone_owner_with_missing_worktree_is_reclaimable() {
    let path = {
        let repo = clean_repo();
        repo.path().to_path_buf()
        // repo dropped here → directory removed
    };
    let lease = lease_at(&path, 120, Some(reaped_pid()), None);
    let report = stale_lease_recovery_for_lease(&lease);
    assert_eq!(
        report.verdict,
        StaleLeaseRecovery::ReclaimableClean {
            worktree_missing: true
        }
    );
}

/// (b) A lease whose owning process is LIVE is never reclaimable — no
/// recovery flag may reap it, even though its worktree is clean.
#[test]
fn live_owner_is_not_reclaimable_even_with_a_clean_worktree() {
    let repo = clean_repo();
    // This test process is, by construction, alive.
    let lease = lease_at(repo.path(), 120, Some(std::process::id()), None);
    let report = stale_lease_recovery_for_lease(&lease);
    assert_eq!(
        report.verdict,
        StaleLeaseRecovery::Live,
        "a lease backed by a running process must never be reapable"
    );
}

/// A live `active_pid` pins the lease too — that is the field `aida ps` reads
/// first, so the recovery verdict cannot disagree with the ps table.
#[test]
fn live_active_pid_is_not_reclaimable() {
    let repo = clean_repo();
    let lease = lease_at(
        repo.path(),
        120,
        Some(reaped_pid()),
        Some(std::process::id()),
    );
    let report = stale_lease_recovery_for_lease(&lease);
    assert_eq!(report.verdict, StaleLeaseRecovery::Live);
}

/// (c) A gone-owner lease whose worktree carries uncommitted work refuses,
/// and the report carries the entries so the refusal can name them.
#[test]
fn gone_owner_with_dirty_worktree_refuses_and_names_the_uncommitted_state() {
    let repo = clean_repo();
    std::fs::write(repo.path().join("wip.rs"), "// unfinished work\n").unwrap();
    let lease = lease_at(repo.path(), 120, Some(reaped_pid()), None);
    let report = stale_lease_recovery_for_lease(&lease);
    assert_eq!(
        report.verdict,
        StaleLeaseRecovery::StaleDirty { dirty_entries: 1 },
        "uncommitted work must block reclaim, not be silently discarded"
    );
    assert!(
        report.dirty_sample().contains("wip.rs"),
        "the refusal must name what is uncommitted, got: {}",
        report.dirty_sample()
    );
}

/// A lease that recorded no pid at all cannot be PROVEN dead — absence of
/// evidence is not evidence of absence (BUG-752), so it refuses rather than
/// being force-claimable.
#[test]
fn lease_without_any_recorded_pid_is_not_reclaimable() {
    let repo = clean_repo();
    let lease = lease_at(repo.path(), 120, None, None);
    let report = stale_lease_recovery_for_lease(&lease);
    assert_eq!(report.verdict, StaleLeaseRecovery::UnknownLiveness);
}

/// (d) The clause every blocking message embeds must state the worktree's
/// clean/dirty verdict — the whole point of the fix is that
/// "work there is lost unless committed" becomes evaluable in place.
#[test]
fn worktree_state_phrase_reports_clean_dirty_or_missing() {
    let clean = StaleLeaseRecoveryReport {
        verdict: StaleLeaseRecovery::ReclaimableClean {
            worktree_missing: false,
        },
        dirty: Vec::new(),
        worktree_exists: true,
    };
    assert!(
        clean.worktree_state_phrase().contains("CLEAN"),
        "got: {}",
        clean.worktree_state_phrase()
    );

    let dirty = StaleLeaseRecoveryReport {
        verdict: StaleLeaseRecovery::StaleDirty { dirty_entries: 2 },
        dirty: vec![" M src/a.rs".into(), "?? src/b.rs".into()],
        worktree_exists: true,
    };
    let phrase = dirty.worktree_state_phrase();
    assert!(phrase.contains("DIRTY"), "got: {phrase}");
    assert!(
        phrase.contains('2'),
        "must count the changes, got: {phrase}"
    );

    let gone = StaleLeaseRecoveryReport {
        verdict: StaleLeaseRecovery::ReclaimableClean {
            worktree_missing: true,
        },
        dirty: Vec::new(),
        worktree_exists: false,
    };
    assert!(
        gone.worktree_state_phrase().contains("GONE"),
        "got: {}",
        gone.worktree_state_phrase()
    );
}

/// The uncommitted-entry rendering is bounded — a worktree with dozens of
/// changes must not paste a wall of text into one error line.
#[test]
fn dirty_sample_is_bounded_and_reports_the_overflow() {
    let report = StaleLeaseRecoveryReport {
        verdict: StaleLeaseRecovery::StaleDirty { dirty_entries: 9 },
        dirty: (0..9).map(|i| format!("?? f{i}.rs")).collect(),
        worktree_exists: true,
    };
    let sample = report.dirty_sample();
    assert!(sample.contains("f0.rs"));
    assert!(sample.contains("+4 more"), "got: {sample}");
    assert!(!sample.contains("f8.rs"), "got: {sample}");
}

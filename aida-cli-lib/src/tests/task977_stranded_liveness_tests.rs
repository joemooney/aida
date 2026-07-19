use super::*;
use process_probe::LiveSession;

// A session lease pointing at `worktree`. Advisory-lease flags off, so it
// classifies through the worktree/claude/age matrix like a real fan-out lease.
fn session_lease_at(worktree: &std::path::Path) -> SessionLease {
    SessionLease {
        id: "abc123".into(),
        scope: "TASK-1".into(),
        slug: "task-1".into(),
        owner: "tester".into(),
        worktree_path: worktree.to_path_buf(),
        branch: "task-1".into(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        active_pid: None,
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

// A no-worktree advisory claim lease (TASK-957) whose liveness is its
// `creator_pid` alone.
fn claim_lease_with_pid(pid: u32) -> SessionLease {
    let mut l = session_lease_at(std::path::Path::new(""));
    l.claim_verb = true;
    l.creator_pid = Some(pid);
    l
}

fn live_session_at(cwd: &std::path::Path) -> LiveSession {
    LiveSession {
        pid: 4242,
        cwd: cwd.to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }
}

// Two session leases, ONE live-session snapshot: the lease whose (existing)
// worktree is covered by a live session counts; the lease whose worktree no
// longer exists (dead holder) does NOT — so the raw lease-file count of 2
// reduces to a live count of 1. Pre-fix, both counted and the alarm fired.
#[test]
fn live_lease_count_excludes_dead_holder_lease() {
    let now = chrono::Utc::now();
    let live_dir = tempfile::tempdir().unwrap();
    let live = session_lease_at(live_dir.path());
    // A worktree path that does not exist → classify_lease_state → Stale.
    let dead = session_lease_at(std::path::Path::new("/no/such/worktree/task-2"));
    let leases = vec![live.clone(), dead];
    let snapshot = vec![live_session_at(live_dir.path())];
    assert_eq!(
        live_lease_count(now, &leases, &snapshot),
        1,
        "only the process-alive lease counts toward the stranded alarm"
    );
}

// A dormant session lease (worktree exists, but no live session covers it and
// it's younger than the 24h stale cutoff) is NOT live → not counted. Guards
// the "dead/quiet holder never false-alarms" invariant.
#[test]
fn live_lease_count_excludes_dormant_lease() {
    let now = chrono::Utc::now();
    let dir = tempfile::tempdir().unwrap();
    let lease = session_lease_at(dir.path());
    // Empty snapshot → no live claude covers the worktree → Dormant, not Live.
    assert_eq!(live_lease_count(now, &[lease], &[]), 0);
}

// A claim lease whose creator process is dead → Stale → not counted, even
// with no worktree to probe. This is the exact false-alarm the spec targets:
// a lease-on-record whose holder is gone must not trip the alarm.
#[test]
fn live_lease_count_excludes_claim_lease_with_dead_creator_pid() {
    let now = chrono::Utc::now();
    // A pid that (essentially) never exists → pid_is_alive == false.
    let dead = claim_lease_with_pid(4_000_000_000);
    assert_eq!(live_lease_count(now, &[dead], &[]), 0);
}

// A claim lease whose creator process IS alive (this very test process) →
// Live → counted. Guards the "a live lease is never missed" half of the fix.
#[test]
fn live_lease_count_counts_claim_lease_with_live_creator_pid() {
    let now = chrono::Utc::now();
    let alive = claim_lease_with_pid(std::process::id());
    assert_eq!(live_lease_count(now, &[alive], &[]), 1);
}

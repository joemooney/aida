use super::*;
use process_probe::LiveSession;

// A session lease pointing at `worktree`. Advisory-lease flags off, so it
// classifies through the worktree/claude/age matrix like a real fan-out lease.
fn session_lease_at(worktree: &std::path::Path) -> SessionLease {
    SessionLease {
        holder_record: None,
        pid_namespace: crate::agent_registry::current_pid_namespace(),
        tree_kind: Default::default(),
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
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
        creator_pid_ns: crate::gate::review_authority::origin_pid_ns(),
        active_pid_ns: crate::gate::review_authority::origin_pid_ns(),
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
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        ending_retained_at: None,
        ending_retained_reason: None,
        driver: None,
        child_grant_id: None,
        vendor_session_id: None,
        lane_jobs: None,
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
    // Keep synthetic snapshot evidence outside the inspector's ancestry:
    // a fixed PID can collide with a CI runner and be correctly excluded.
    // trace:BUG-2513 | ai:codex
    let pid = (1..=u32::MAX)
        .find(|pid| !crate::leases::inspector_pids().contains(pid))
        .unwrap();
    LiveSession {
        pid,
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
    // The inspector itself must still not supply liveness for a pidless lease.
    // trace:BUG-2513 | ai:codex
    let inspector_snapshot = vec![LiveSession {
        pid: std::process::id(),
        ..live_session_at(live_dir.path())
    }];
    assert_eq!(live_lease_count(now, &[live], &inspector_snapshot), 0);
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

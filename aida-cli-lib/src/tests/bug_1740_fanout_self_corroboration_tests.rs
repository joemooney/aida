//! Tests for BUG-1740: `possibly_subagent` was self-satisfying.
//!
//! The `harness-worktree` lease cited as evidence of a live Agent-tool fan-out
//! was pinned to the PROJECT ROOT and carried no pid of any kind, so
//! `lease_state_for` could only reach `Live` through its `has_live_claude`
//! arm — a claude process whose cwd is inside the lease's worktree. Because
//! the lease's worktree was the whole checkout, the process that satisfied it
//! was the one running the command. The report said *someone may be on this*
//! on the strength of the reader's own existence.
//!
//! Two rules close it, and this file pins both plus the case that must NOT
//! regress:
//!
//!   AC1 — a harness lease pinned to the project root is never, on its own,
//!         evidence of a fan-out (asserted for a caller-only backing AND for a
//!         distinct live worker, because AC1 asks the rule to be stated).
//!   AC2 — the reporting session is never its own corroboration. This holds
//!         for BOTH evidence doors: presence (the caller's cwd) and identity
//!         (a lease whose `active_pid`/`creator_pid` is the caller or one of
//!         its ancestors — `lease_state_for` short-circuits `Live` on those
//!         pids before the presence list is ever consulted).
//!   AC4 — a harness lease backed by a live process that is not the caller
//!         still reads as a fan-out. TASK-1064's case is real.
//
// trace:BUG-1740 | ai:claude

use super::*;
use crate::dispatch_health_ps::WorktreeGitProbe;

/// A pid that is not in this process's ancestor chain: a genuine other worker.
/// Only the cwd/presence arm is exercised with it, which matches on cwd and
/// never probes `/proc`, so the pid need not exist.
const WORKER_PID: u32 = 424_242;

/// A start time no live process can have, so `process_identity_is_alive`
/// returns false for a pid that genuinely IS alive. Deterministic where
/// "pick a pid and hope it is dead" is a flake.
const NEVER_A_LIVE_START: &str = "1970-01-01T00:00:00+00:00";

/// Local copy of `story_696_ps_tests::ps_lease` — sibling test modules cannot
/// reach each other's private helpers (see `task_1451_mail_identity_ps_tests`).
fn lease(id: &str, scope: &str, worktree: std::path::PathBuf) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: worktree,
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now() - chrono::Duration::hours(48),
        hostname: "h".into(),
        role: Some("implementer".into()),
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
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
    }
}

fn harness_lease(id: &str, worktree: std::path::PathBuf) -> SessionLease {
    let mut l = lease(id, worktree_lease::HARNESS_WORKTREE_SCOPE, worktree);
    l.branch = "main".into();
    l
}

fn live_at(pid: u32, cwd: &std::path::Path) -> process_probe::LiveSession {
    process_probe::LiveSession {
        pid,
        cwd: cwd.to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }
}

// --- AC5: the four corners, on the pure predicate -------------------------

/// Corner 1 (AC1 + AC2, the filed shape): a root-pinned harness lease whose
/// only live backing is the caller. This is the exact lease this repository
/// carried — `worktree_path == parent_project_root`, no `active_pid`, no
/// `creator_pid`, months old.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_root_pinned_lease_backed_only_by_the_caller_is_not_a_fanout() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut l = harness_lease("sess-root", root.clone());
    l.parent_project_root = Some(root.clone());
    let leases = [l];
    let live = vec![live_at(std::process::id(), &root)];
    let caller = [std::process::id()];

    assert!(
        ps_live_fanout_leases(&leases, &live, chrono::Utc::now(), &caller).is_empty(),
        "the reporting session standing in the project root is not a fan-out"
    );
}

/// Corner 2 (AC1, decided and asserted): a root-pinned harness lease backed by
/// a live process that is NOT the caller is still not a fan-out. AC1 offers two
/// rules and asks which was chosen; this is the chosen one. A genuine fan-out
/// does not need the root lease — the harness gives each Agent-tool subagent
/// its own worktree, and corner 3 is that case.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_root_pinned_lease_is_not_a_fanout_even_with_a_distinct_live_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut l = harness_lease("sess-root2", root.clone());
    l.parent_project_root = Some(root.clone());
    let leases = [l];
    let live = vec![live_at(WORKER_PID, &root)];
    let caller = [std::process::id()];

    assert!(
        ps_live_fanout_leases(&leases, &live, chrono::Utc::now(), &caller).is_empty(),
        "a lease pinned to the checkout itself is the ambient session, not a dispatched fan-out"
    );
}

/// Corner 3 (AC4): a harness lease with a worktree of its own, backed by a live
/// process that is not the caller, IS a fan-out. TASK-1064's case survives.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_non_root_harness_lease_with_a_distinct_live_worker_is_a_fanout() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let agent_wt = root.join(".claude/worktrees/agent-abc");
    std::fs::create_dir_all(&agent_wt).unwrap();
    let mut l = harness_lease("sess-agent", agent_wt.clone());
    l.parent_project_root = Some(root.clone());
    let leases = [l];
    let live = vec![live_at(WORKER_PID, &agent_wt)];
    let caller = [std::process::id()];

    let found = ps_live_fanout_leases(&leases, &live, chrono::Utc::now(), &caller);
    assert_eq!(found.len(), 1, "a real subagent worktree is a fan-out");
    assert_eq!(found[0].id, "sess-agent");
}

/// Corner 4: no harness lease at all — a live spec-scoped lease is not a
/// fan-out however alive it is.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_no_harness_lease_is_never_a_fanout() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().to_path_buf();
    let leases = [lease("sess-spec", "BUG-1740", wt.clone())];
    let live = vec![live_at(WORKER_PID, &wt)];
    assert!(
        ps_live_fanout_leases(&leases, &live, chrono::Utc::now(), &[]).is_empty(),
        "a spec-scoped lease is not a generic fan-out lease"
    );
}

/// AC2, separated from AC1: a harness lease with a worktree of its OWN — so
/// corner 2's root rule cannot be what rejects it — whose only live backing is
/// the caller. This is the operator `cd`-ing into a stranded worktree to look
/// at it: their presence must not then testify that someone is working there.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_caller_presence_in_a_non_root_worktree_is_not_a_fanout() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt = root.join("wt-bug-1693");
    std::fs::create_dir_all(&wt).unwrap();
    let mut l = harness_lease("sess-visited", wt.clone());
    l.parent_project_root = Some(root.clone());
    let leases = [l];
    let caller = process_probe::walk_ancestor_pids(std::process::id());
    let live = vec![live_at(std::process::id(), &wt)];

    assert!(
        ps_live_fanout_leases(&leases, &live, chrono::Utc::now(), &caller).is_empty(),
        "the observer's own cwd is not evidence of a worker"
    );
    // ...and the SAME lease, same tree, with a worker that is not the caller.
    let live_worker = vec![live_at(WORKER_PID, &wt)];
    assert_eq!(
        ps_live_fanout_leases(&leases, &live_worker, chrono::Utc::now(), &caller).len(),
        1,
        "mutation control: only the identity of the live process differs"
    );
}

/// AC2's "or its ancestors": the session that invoked `aida` is this process's
/// PARENT, not this process, so the whole chain must be excluded. Pinned
/// separately because excluding only `std::process::id()` passes every other
/// test in this file.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_an_ancestor_of_the_caller_is_also_the_caller() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt = root.join("wt-bug-1688");
    std::fs::create_dir_all(&wt).unwrap();
    let mut l = harness_lease("sess-ancestor", wt.clone());
    l.parent_project_root = Some(root.clone());
    let leases = [l];
    let chain = process_probe::walk_ancestor_pids(std::process::id());
    let parent = chain
        .iter()
        .copied()
        .find(|p| *p != std::process::id())
        .expect("the test process has at least one ancestor");
    let live = vec![live_at(parent, &wt)];

    assert!(
        ps_live_fanout_leases(&leases, &live, chrono::Utc::now(), &chain).is_empty(),
        "the harness hosting this process is still the observer"
    );
}

// --- AC2, the IDENTITY door (rework) ---------------------------------------
//
// `lease_state_for` short-circuits to `Live` on a live `active_pid` (and on
// `creator_pid` in its advisory-lock arm) BEFORE the presence list is
// consulted, so subtracting the caller from the live-session list alone left
// a second door open: a harness lease whose recorded pid IS the reporting
// session (BUG-1772's unreaped harness leases keep exactly that pid) still
// self-corroborated. These tests pin the identity exclusion and its AC4
// boundary.

/// A sleeping child process: a REAL live pid that is not in this process's
/// ancestor chain, for exercising the `active_pid`/`creator_pid` arms of
/// `lease_state_for` (they probe `/proc`, so a made-up pid reads dead).
/// Killed on drop so an assertion failure cannot leak it.
struct LiveWorker(std::process::Child);

impl LiveWorker {
    fn spawn() -> Self {
        LiveWorker(
            std::process::Command::new("sleep")
                .arg("120")
                .spawn()
                .expect("spawn sleep"),
        )
    }
    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for LiveWorker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// AC2, identity arm, the direct shape: a harness lease (worktree of its own,
/// so the root rule is not what rejects it) whose `active_pid` is the caller
/// itself. `lease_state_for` reads it `Live` from the pid arm without ever
/// consulting the (caller-subtracted) presence list — it must still not count
/// as a fan-out. No live sessions at all, so ONLY the identity door could
/// have corroborated.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_harness_lease_whose_active_pid_is_the_caller_is_not_a_fanout() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt = root.join(".claude/worktrees/agent-self");
    std::fs::create_dir_all(&wt).unwrap();
    let mut l = harness_lease("sess-self-pid", wt);
    l.parent_project_root = Some(root);
    l.active_pid = Some(std::process::id());
    let leases = [l];
    let caller = process_probe::walk_ancestor_pids(std::process::id());

    assert!(
        ps_live_fanout_leases(&leases, &[], chrono::Utc::now(), &caller).is_empty(),
        "a lease whose active_pid is the reporting session is the observer testifying for itself"
    );
}

/// AC2's "or its ancestors", identity arm: an Agent-tool subagent executes
/// INSIDE the parent claude process, so a harness lease it minted carries an
/// `active_pid` that is an ANCESTOR of any `aida` later run from that same
/// session. That pid stays alive as long as the session does (BUG-1772 makes
/// the lease persistent), and it must not corroborate a fan-out.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_harness_lease_whose_active_pid_is_an_ancestor_of_the_caller_is_not_a_fanout() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt = root.join(".claude/worktrees/agent-anc");
    std::fs::create_dir_all(&wt).unwrap();
    let chain = process_probe::walk_ancestor_pids(std::process::id());
    let parent = chain
        .iter()
        .copied()
        .find(|p| *p != std::process::id())
        .expect("the test process has at least one ancestor");
    let mut l = harness_lease("sess-anc-pid", wt);
    l.parent_project_root = Some(root);
    l.active_pid = Some(parent);
    let leases = [l];

    assert!(
        ps_live_fanout_leases(&leases, &[], chrono::Utc::now(), &chain).is_empty(),
        "the long-lived session that minted the lease is still the observer"
    );
}

/// AC4 witness (TASK-1064): the SAME lease shape with `active_pid` naming a
/// live process that is NOT in the caller's chain keeps the fan-out. This is
/// the lease as seen from any OTHER session — the operator's window, a
/// monitor seat — and is what the identity exclusion must not delete.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_harness_lease_whose_active_pid_is_a_distinct_live_worker_is_a_fanout() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt = root.join(".claude/worktrees/agent-worker");
    std::fs::create_dir_all(&wt).unwrap();
    let worker = LiveWorker::spawn();
    let mut l = harness_lease("sess-worker-pid", wt);
    l.parent_project_root = Some(root);
    l.active_pid = Some(worker.pid());
    let leases = [l];
    let caller = process_probe::walk_ancestor_pids(std::process::id());

    let found = ps_live_fanout_leases(&leases, &[], chrono::Utc::now(), &caller);
    assert_eq!(
        found.len(),
        1,
        "a live active_pid outside the caller's chain is a real worker (AC4)"
    );
    assert_eq!(found[0].id, "sess-worker-pid");
}

/// The `creator_pid` arm, covered identically. `lease_state_for`'s
/// advisory-lock arm (`review_verb`/`claim_verb` with no worktree of its own)
/// reads `creator_pid` instead of `active_pid`, and `ps_live_fanout_leases`
/// admits such a lease: the scope check is on `scope` alone, and
/// `ps_lease_is_project_root_pinned` returns false for an empty
/// `worktree_path`, so nothing upstream makes the arm unreachable. A
/// caller-minted advisory lease must not corroborate; one minted by a
/// distinct live process still does.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_advisory_harness_lease_creator_pid_mirrors_the_active_pid_rule() {
    let mut l = harness_lease("sess-advisory", std::path::PathBuf::new());
    l.claim_verb = true;
    l.creator_pid = Some(std::process::id());
    let leases = [l];
    let caller = process_probe::walk_ancestor_pids(std::process::id());

    assert!(
        ps_live_fanout_leases(&leases, &[], chrono::Utc::now(), &caller).is_empty(),
        "an advisory lease whose creator_pid is the caller is self-corroboration"
    );

    // AC4 boundary, same shape: a creator that is a live process outside the
    // caller's chain stands.
    let worker = LiveWorker::spawn();
    let mut other = harness_lease("sess-advisory-other", std::path::PathBuf::new());
    other.claim_verb = true;
    other.creator_pid = Some(worker.pid());
    let leases = [other];
    assert_eq!(
        ps_live_fanout_leases(&leases, &[], chrono::Utc::now(), &caller).len(),
        1,
        "mutation control: only whose pid minted the lease differs"
    );
}

/// The mirroring in `ps_lease_identity_is_caller` follows `lease_state_for`'s
/// arm ORDER, not "any recorded pid": a lease the caller MINTED
/// (`creator_pid` in the caller's chain) but whose liveness is attested by a
/// distinct worker's `active_pid` is a genuine fan-out — the orchestrator
/// mints the lease, the worker holds it. An unconditional creator-pid check
/// would delete AC4's case; this pins that it doesn't.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_caller_minted_lease_held_by_a_distinct_live_worker_is_a_fanout() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt = root.join(".claude/worktrees/agent-minted");
    std::fs::create_dir_all(&wt).unwrap();
    let worker = LiveWorker::spawn();
    let mut l = harness_lease("sess-minted", wt);
    l.parent_project_root = Some(root);
    l.creator_pid = Some(std::process::id());
    l.active_pid = Some(worker.pid());
    let leases = [l];
    let caller = process_probe::walk_ancestor_pids(std::process::id());

    assert_eq!(
        ps_live_fanout_leases(&leases, &[], chrono::Utc::now(), &caller).len(),
        1,
        "who minted the lease is not who holds it; the deciding pid is the worker's"
    );
}

// --- AC3/AC4 end to end, through the orphan pass --------------------------

fn running_work(
    specs: &[RunningWorkSpec],
    leases: &[SessionLease],
    live: &[process_probe::LiveSession],
) -> (Vec<PsRow>, Vec<PsOrphan>) {
    build_running_work(
        specs,
        leases,
        live,
        chrono::Utc::now(),
        |_| WorktreeGitProbe::default(),
        |_| None,
        |_| None,
        |_| None,
        |_| None,
        |_| MailIdentityStatus::Unknown,
        |_, _| SeatActivity::Unknown,
    )
}

fn in_progress_spec(disp: &str) -> RunningWorkSpec {
    RunningWorkSpec {
        disp: disp.into(),
        agreed_id: Some(disp.into()),
        spec_id: Some(disp.into()),
        title: format!("{disp} work"),
        in_progress: true,
        orphan_excluded_type: false,
    }
}

/// The reported symptom, end to end. A spec whose own lease died (dead
/// `active_pid` → `Stale`) sits in a worktree the operator has `cd`-ed into to
/// investigate. A harness lease on that worktree reads `Live` from the
/// operator's own process, and before BUG-1740 that made the row say
/// "possibly worked by a subagent" about work nobody was on.
///
/// The second half is the mutation control for AC6: the identical tree with
/// the live pid changed to a worker keeps `possibly_subagent: true`, so the
/// first assertion cannot be passing because the orphan is simply never
/// flagged.
// trace:BUG-1740 | ai:claude
#[test]
fn bug_1740_stranded_spec_is_abandoned_not_possibly_subagent_when_only_the_caller_is_live() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt = root.join("wt-bug-1693");
    std::fs::create_dir_all(&wt).unwrap();

    let specs = vec![in_progress_spec("BUG-1693")];
    let mut spec_lease = lease("sess-dead", "BUG-1693", wt.clone());
    // A crashed session. The recorded pid/start-time pair identifies no live
    // process, so `lease_state_for` returns Stale from the pid arm without
    // ever consulting live sessions — which is what lets the spec lease and
    // the harness lease on the SAME worktree disagree about liveness.
    spec_lease.active_pid = Some(std::process::id());
    spec_lease.active_pid_start_time = Some(NEVER_A_LIVE_START.into());
    spec_lease.branch = "bug-1693".into();
    let mut harness = harness_lease("sess-harness", wt.clone());
    harness.parent_project_root = Some(root.clone());
    let leases = vec![spec_lease, harness];

    let caller_live = vec![live_at(std::process::id(), &wt)];
    let (_rows, orphans) = running_work(&specs, &leases, &caller_live);
    let orphan = orphans
        .iter()
        .find(|o| o.spec == "BUG-1693")
        .expect("the stale spec lease is listed as an orphan");
    assert!(orphan.stale_lease, "the dead pid makes this lease stale");
    assert!(
        !orphan.possibly_subagent,
        "the reporting session must not corroborate a fan-out onto its own row"
    );

    // Mutation control: the only difference is WHOSE process is live.
    let worker_live = vec![live_at(WORKER_PID, &wt)];
    let (_rows, orphans) = running_work(&specs, &leases, &worker_live);
    let orphan = orphans
        .iter()
        .find(|o| o.spec == "BUG-1693")
        .expect("the stale spec lease is listed as an orphan");
    assert!(
        orphan.possibly_subagent,
        "a live subagent that is not the caller still suppresses the orphan framing (AC4)"
    );
}

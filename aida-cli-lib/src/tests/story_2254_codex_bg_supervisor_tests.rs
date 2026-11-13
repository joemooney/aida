//! STORY-2254: the detached supervisor, exec barrier, exec corroboration and
//! terminal cleanup, run for real and hermetically. The test thread plays the
//! dispatcher over the private channel; the supervisor runs in-process; the
//! exec barrier is THIS test binary re-executed into `barrier_main`, which
//! `execve`s a stand-in vendor binary. Grants, leases, registry, ledger and
//! receipts all live in temporary roots. Faults are injected at each named
//! supervisor [`Step`], never by sleeping on races. No model, no real Codex.
// trace:STORY-2254 | ai:claude

use super::*;
use crate::gate::seat_authority::{self as seat, DetachedLaunchCapability, GRANT_ENV};
use crate::test_env::EnvVarsGuard;
use std::os::unix::process::CommandExt;
use std::sync::{Arc, Mutex};

const SUBJECT: &str = "story-2254-supervisor-user";
const AGENT: &str = "codex-implementer-77";
const BARRIER_ENV: &str = "AIDA_STORY_2254_BARRIER_ENTRY";

/// Not a test of its own: the exec barrier's entry when this binary is
/// re-executed by a supervisor under test. A normal run returns at once.
#[test]
fn story_2254_barrier_entry() {
    if std::env::var_os(BARRIER_ENV).is_none() {
        return;
    }
    let result = barrier_main();
    eprintln!("barrier returned without exec: {result:?}");
    std::process::exit(126);
}

fn barrier_test_name() -> String {
    let path = module_path!();
    let path = path.split_once("::").map(|(_, rest)| rest).unwrap_or(path);
    format!("{path}::story_2254_barrier_entry")
}

fn canonical_program(name: &str) -> std::path::PathBuf {
    find_executable_on_path(name)
        .unwrap_or_else(|| panic!("{name} on PATH"))
        .canonicalize()
        .unwrap()
}

struct Harness {
    _home: tempfile::TempDir,
    project: tempfile::TempDir,
    worktree: std::path::PathBuf,
    env: EnvVarsGuard,
    dir: std::path::PathBuf,
    attempt: String,
    spec: String,
    me: ProcId,
    lease_id: String,
    grant_id: String,
    capability: Option<DetachedLaunchCapability>,
    marker: std::path::PathBuf,
}

impl Harness {
    fn root(&self) -> &std::path::Path {
        self.project.path()
    }
    fn receipt(&self) -> LaunchReceipt {
        match receipt::read_receipt(&self.dir, self.root(), &self.attempt) {
            ReceiptRead::Known(r) => *r,
            ReceiptRead::Unknown(why) => panic!("receipt unknown: {why}"),
        }
    }
    fn grant(&self) -> seat::SeatGrant {
        seat::read_grant(&self.grant_id).unwrap()
    }
    fn lease_toml(&self) -> toml::Table {
        std::fs::read_to_string(
            self.root()
                .join(".aida/sessions")
                .join(format!("{}.toml", self.lease_id)),
        )
        .unwrap()
        .parse()
        .unwrap()
    }
    /// A sleeping stand-in vendor worker (a native executable).
    fn sleep_worker(&self, secs: &str) -> (WorkerExec, ExpectedExec) {
        let sleep = canonical_program("sleep");
        (
            WorkerExec {
                program: sleep.clone(),
                args: vec![secs.into()],
                env: vec![("PATH".into(), std::env::var("PATH").unwrap_or_default())],
                cwd: self.worktree.clone(),
            },
            ExpectedExec::Native { path: sleep },
        )
    }
    /// A worker that leaves a marker if it EVER executes.
    fn marker_worker(&self) -> (WorkerExec, ExpectedExec) {
        let sh = canonical_program("sh");
        (
            WorkerExec {
                program: sh.clone(),
                args: vec![
                    "-c".into(),
                    format!("touch '{}'; sleep 5", self.marker.display()),
                ],
                env: vec![("PATH".into(), std::env::var("PATH").unwrap_or_default())],
                cwd: self.worktree.clone(),
            },
            ExpectedExec::Native { path: sh },
        )
    }
    fn init(&self, (worker, expected): (WorkerExec, ExpectedExec)) -> SupervisorInit {
        SupervisorInit {
            attempt: self.attempt.clone(),
            project_root: self.root().to_path_buf(),
            capability: None,
            worker,
            expected,
            agent_name: AGENT.into(),
            role: "implementer".into(),
            spec: self.spec.clone(),
            lease_id: self.lease_id.clone(),
            grant_id: self.grant_id.clone(),
            delegate_seats: Vec::new(),
            description: None,
            mail_wake: "unsupported".into(),
            context_path: None,
        }
    }
    /// The dispatcher's durable ownership transfer.
    fn transfer(&self, sup: &ProcId, worker: &ProcId) {
        let me = self.me.clone();
        let (sup, worker) = (sup.clone(), worker.clone());
        receipt::compare_update(
            &self.dir,
            self.root(),
            &self.attempt,
            |r| {
                anyhow::ensure!(
                    r.owner == Owner::Dispatcher(me) && r.phase == Phase::Prepared,
                    "not ours"
                );
                Ok(())
            },
            |r| {
                r.owner = Owner::Supervisor(sup.clone());
                r.phase = Phase::SupervisorOwned;
                r.supervisor = Some(sup);
                r.worker = Some(worker);
            },
        )
        .unwrap();
    }
}

#[cfg(target_os = "linux")]
fn harness(spec: &str) -> Harness {
    harness_delegating(spec, "implementer", &[])
}

#[cfg(target_os = "linux")]
fn harness_delegating(spec: &str, role: &str, selection: &[String]) -> Harness {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    let mut env = EnvVarsGuard::snapshot(&["AIDA_USER", GRANT_ENV, "AIDA_SESSION_ID", "AIDA_HOME"]);
    env.set_key("AIDA_HOME", home.path());
    env.set_key("AIDA_USER", SUBJECT);
    env.unset_key("AIDA_SESSION_ID");
    let parent_seats: &[&str] = if role == "advisor" {
        &["advisor", "implementer", "reviewer"]
    } else {
        &["implementer"]
    };
    let parent = seat::test_support::mint_grant_for(root, SUBJECT, "advisor", parent_seats);
    env.set_key(GRANT_ENV, &parent);
    let worktree = root.join("wt");
    std::fs::create_dir_all(&worktree).unwrap();
    let dir = receipt::runtime_dir(root).unwrap();
    let me = ProcId::current().unwrap();
    let attempt = uuid::Uuid::new_v4().to_string();
    receipt::admit(
        &dir,
        &LaunchReceipt::new(
            root,
            &attempt,
            ScopeKey::new(role, spec),
            me.clone(),
            AGENT,
            &worktree,
            &dir.join("logs").join(format!("{attempt}.log")),
        ),
        || Ok(()),
    )
    .unwrap();
    let (grant, capability) = seat::issue_detached_launch_child(
        root,
        role,
        AGENT,
        false,
        &attempt,
        &format!("{role}:{spec}"),
        selection,
    )
    .unwrap();
    let lease_id = "a1b2c3d4e5f6".to_string();
    let sessions = root.join(".aida/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{lease_id}.toml")),
        format!(
            "id = \"{lease_id}\"\nscope = \"{spec}\"\nslug = \"{}\"\nowner = \"test\"\nworktree_path = \"{}\"\nbranch = \"story-2254-fixture\"\nstarted_at = \"2026-11-02T00:00:00Z\"\nhostname = \"test\"\nrole = \"{role}\"\nchild_grant_id = \"{}\"\n",
            spec.to_ascii_lowercase(),
            worktree.display(),
            grant.id
        ),
    )
    .unwrap();
    {
        let (lease, gid, wt) = (lease_id.clone(), grant.id.clone(), worktree.clone());
        receipt::compare_update(
            &dir,
            root,
            &attempt,
            |_| Ok(()),
            |r| {
                r.lease_id = Some(lease);
                r.grant_id = Some(gid);
                r.worktree = wt;
            },
        )
        .unwrap();
    }
    let marker = root.join("worker-executed");
    Harness {
        _home: home,
        worktree,
        env,
        dir,
        attempt,
        spec: spec.to_string(),
        me,
        lease_id,
        grant_id: grant.id,
        capability: Some(capability),
        marker,
        project,
    }
}

struct Run {
    ours: std::os::unix::net::UnixStream,
    reader: BufReader<std::os::unix::net::UnixStream>,
    thread: std::thread::JoinHandle<Result<()>>,
}

impl Run {
    fn recv(&mut self) -> Option<Control> {
        recv(&mut self.reader).ok().flatten()
    }
    fn spawned(&mut self) -> (ProcId, ProcId) {
        match self.recv() {
            Some(Control::Spawned { supervisor, worker }) => (supervisor, worker),
            other => panic!("expected Spawned, got {other:?}"),
        }
    }
    fn ack(&mut self) {
        send(&mut self.ours, &Control::Transferred).unwrap();
    }
    fn join(self) -> Result<()> {
        drop(self.ours);
        drop(self.reader);
        self.thread.join().expect("supervisor thread panicked")
    }
}

fn start(
    h: &mut Harness,
    init: SupervisorInit,
    at: impl Fn(Step, &ProcId) -> Result<()> + Send + 'static,
) -> Run {
    let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    ours.set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let capability = DetachedLaunchCapability::received_in_process_for_test(
        h.capability.take().expect("one capability").into_transfer(),
    );
    let mut barrier_env = vec![(BARRIER_ENV.to_string(), "1".to_string())];
    if let Ok(path) = std::env::var("LD_LIBRARY_PATH") {
        barrier_env.push(("LD_LIBRARY_PATH".into(), path));
    }
    let env = SupervisorEnv {
        barrier_program: crate::aida_bin::current_executable_path().unwrap(),
        barrier_args: vec![
            "--exact".into(),
            "--nocapture".into(),
            "--test-threads=1".into(),
            barrier_test_name(),
        ],
        barrier_env,
        at: Box::new(at),
        exec_corroboration: std::time::Duration::from_secs(10),
    };
    let attempt = h.attempt.clone();
    let to_parent = theirs.try_clone().unwrap();
    let reader = BufReader::new(theirs);
    let thread = std::thread::spawn(move || {
        supervise_attempt(&attempt, init, capability, to_parent, reader, env)
    });
    Run {
        reader: BufReader::new(ours.try_clone().unwrap()),
        ours,
        thread,
    }
}

fn no_faults(_: Step, _: &ProcId) -> Result<()> {
    Ok(())
}

fn bound_to(h: &Harness, w: &ProcId) -> bool {
    h.grant()
        .binding
        .contains(&crate::gate::review_authority::process_token(
            "proc",
            w.pid,
            Some(&w.start),
        ))
}

fn wait_gone(p: &ProcId) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if p.state() == receipt::ProcState::Gone {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

/// The whole detached lifecycle on a real process tree: the worker PID the
/// supervisor registers is the exec barrier's, which `execve`s the vendor in
/// place (same PID AND start identity); grant binding, lease, authorship and
/// registry all name that identity before release; acceptance is reported
/// only after the vendor executable is corroborated; on exit the supervisor
/// records the status, ends exactly its own registry row (a successor's row
/// is untouched), revokes its own child grant and releases the scope.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac3769a0 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac9b0efd | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn story_2254_bg_supervisor_end_to_end_accepts_registers_and_cleans_up() {
    let mut h = harness("TASK-2254");
    let init = h.init(h.sleep_worker("2"));
    let sleep = canonical_program("sleep");
    let mut run = start(&mut h, init, no_faults);
    let (sup, worker) = run.spawned();
    assert_eq!(sup.pid, std::process::id(), "the supervisor reports itself");
    assert_eq!(
        agent_registry::parent_pid_of(worker.pid),
        Some(sup.pid),
        "the gated worker is the supervisor's own child"
    );
    assert!(!bound_to(&h, &worker), "nothing binds before transfer");
    h.transfer(&sup, &worker);
    run.ack();
    let (accepted, exe) = match run.recv() {
        Some(Control::Accepted { worker, exe }) => (worker, exe),
        other => panic!("expected Accepted, got {other:?}"),
    };
    assert_eq!(
        accepted, worker,
        "the registered worker is the exec'd vendor"
    );
    assert_eq!(std::path::PathBuf::from(&exe), sleep);
    assert_eq!(
        std::fs::read_link(format!("/proc/{}/exe", worker.pid)).unwrap(),
        sleep
    );
    assert_eq!(
        aida_core::liveness::process_start_identity(worker.pid).as_deref(),
        Some(worker.start.as_str()),
        "execve kept the worker's PID and start identity"
    );
    assert_eq!(h.receipt().phase, Phase::AcceptedRunning);
    assert!(bound_to(&h, &worker), "own child grant bound to the worker");
    assert!(h.grant().launch_capability.is_none(), "capability consumed");
    let lease = h.lease_toml();
    assert_eq!(
        lease["active_pid"].as_integer(),
        Some(i64::from(worker.pid))
    );
    assert_eq!(
        lease["active_pid_start_time"].as_str(),
        Some(worker.start.as_str())
    );
    let token =
        crate::gate::review_authority::process_token("proc", worker.pid, Some(&worker.start));
    assert!(crate::gate::review_authority::load_ledger(h.root())
        .iter()
        .any(|r| r.lease_id == h.lease_id && r.tokens.contains(&token)));
    let live = agent_registry::load_live_entries(h.root());
    let row = live
        .iter()
        .find(|e| e.pid == worker.pid)
        .expect("the worker is registered live");
    assert_eq!(row.pid_start.as_deref(), Some(worker.start.as_str()));
    assert_eq!(row.name.as_deref(), Some(AGENT));
    assert_eq!(row.current_spec.as_deref(), Some("TASK-2254"));
    // A successor's row under the same name, live while the old worker ends.
    let mut successor = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let successor_start = aida_core::liveness::process_start_identity(successor.id()).unwrap();
    agent_registry::register_detached_worker(
        h.root(),
        "codex",
        successor.id(),
        &successor_start,
        agent_registry::SpawnedRow {
            delegate_seats: Vec::new(),
            role: Some("implementer".into()),
            current_spec: Some("TASK-2254".into()),
            worktree_path: h.worktree.clone(),
            binary: None,
            name: Some(AGENT.into()),
            native_session_id: None,
            resumed_from: None,
            description: None,
        },
    )
    .unwrap();
    run.join().unwrap();
    let r = h.receipt();
    assert_eq!(r.phase, Phase::Exited);
    assert_eq!(r.owner, Owner::Released);
    let terminal = r.terminal.unwrap();
    assert_eq!((terminal.code, terminal.unknown), (Some(0), None));
    assert!(h.grant().revoked_at.is_some(), "own child grant revoked");
    let entries = agent_registry::load_entries(h.root());
    let ended = |pid: u32| {
        entries
            .iter()
            .find(|(_, e)| e.pid == pid)
            .map(|(_, e)| e.ended_at.is_some())
    };
    assert_eq!(ended(worker.pid), Some(true), "own row ended");
    assert_eq!(ended(successor.id()), Some(false), "successor untouched");
    assert!(receipt::scope_entry_blocker(h.root(), "implementer", "TASK-2254").is_none());
    let _ = successor.kill();
    let _ = successor.wait();
}

/// The launcher disappears before it commits the transfer: the supervisor
/// stops its gated worker unrun; nothing is bound and the attempt stays the
/// (dead) launcher's for explicit recovery.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_parent_exit_before_transfer_stops_worker_unrun() {
    let mut h = harness("TASK-2255");
    let init = h.init(h.marker_worker());
    let mut run = start(&mut h, init, no_faults);
    let (_, worker) = run.spawned();
    let err = run.join().unwrap_err();
    assert!(format!("{err:#}").contains("not transferred"), "{err:#}");
    assert!(wait_gone(&worker));
    assert!(!h.marker.exists(), "the worker never executed");
    assert!(!bound_to(&h, &worker));
    let r = h.receipt();
    assert_eq!(
        (r.phase, r.owner),
        (Phase::Prepared, Owner::Dispatcher(h.me.clone()))
    );
}

/// The launcher commits the transfer, then disappears before acknowledging:
/// the supervisor continues from the durable owner record (a lost ack is not
/// guessed from EOF) and runs the attempt to its terminal state.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_parent_exit_after_transfer_lost_ack_supervisor_continues() {
    let mut h = harness("TASK-2256");
    let init = h.init(h.sleep_worker("0.3"));
    let mut run = start(&mut h, init, no_faults);
    let (sup, worker) = run.spawned();
    h.transfer(&sup, &worker);
    // No acknowledgement, and the launcher's end goes away.
    run.join().unwrap();
    let r = h.receipt();
    assert_eq!(r.phase, Phase::Exited);
    assert_eq!(r.terminal.unwrap().code, Some(0));
    assert!(h.grant().revoked_at.is_some());
}

/// Rollback is gated on the durable owner, not on local disarm timing: a
/// launcher guard dropped after the transfer committed (but before its
/// disarm) destroys nothing; one dropped while still the owner rolls back
/// exactly its own grant and reservation.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_transfer_before_drop_never_rolls_back() {
    let h = harness("TASK-2257");
    let stranger = ProcId {
        pid: 1,
        start: "linux-starttime:1".into(),
    };
    h.transfer(&stranger, &stranger);
    drop(DispatcherGuard {
        dir: h.dir.clone(),
        project_root: h.root().to_path_buf(),
        attempt: h.attempt.clone(),
        me: h.me.clone(),
        grant_id: Some(h.grant_id.clone()),
        armed: true,
    });
    assert!(
        h.grant().revoked_at.is_none(),
        "supervisor-owned grant kept"
    );
    assert_eq!(h.receipt().phase, Phase::SupervisorOwned);

    // Still the owner: the guard rolls back its own grant and reservation.
    let attempt = uuid::Uuid::new_v4().to_string();
    receipt::admit(
        &h.dir,
        &LaunchReceipt::new(
            h.root(),
            &attempt,
            ScopeKey::new("implementer", "TASK-2258"),
            h.me.clone(),
            AGENT,
            &h.worktree,
            &h.dir.join("x.log"),
        ),
        || Ok(()),
    )
    .unwrap();
    let (grant, _cap) = seat::issue_detached_launch_child(
        h.root(),
        "implementer",
        AGENT,
        false,
        &attempt,
        "implementer:TASK-2258",
        &[],
    )
    .unwrap();
    drop(DispatcherGuard {
        dir: h.dir.clone(),
        project_root: h.root().to_path_buf(),
        attempt: attempt.clone(),
        me: h.me.clone(),
        grant_id: Some(grant.id.clone()),
        armed: true,
    });
    assert!(seat::read_grant(&grant.id).unwrap().revoked_at.is_some());
    match receipt::read_receipt(&h.dir, h.root(), &attempt) {
        ReceiptRead::Known(r) => assert_eq!(r.phase, Phase::RefusedBeforeExecution),
        ReceiptRead::Unknown(why) => panic!("{why}"),
    }
}

/// A receipt lost or corrupted mid-attempt is never read as acceptance: the
/// supervisor refuses before release (the worker never executes), revokes
/// its own grant, and the scope the corrupt receipt names stays held until
/// it is explicitly retired as provably dead.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_lost_or_corrupt_receipt_is_unknown_not_acceptance() {
    let mut h = harness("TASK-2259");
    let init = h.init(h.marker_worker());
    let path = h.dir.join("attempts").join(format!("{}.json", h.attempt));
    let corrupt = path.clone();
    let mut run = start(&mut h, init, move |step, _| {
        if step == Step::BeforeRegistered {
            std::fs::write(
                &corrupt,
                r#"{"version": 99, "scope": {"role": "implementer", "spec": "TASK-2259"}}"#,
            )?;
        }
        Ok(())
    });
    let (sup, worker) = run.spawned();
    h.transfer(&sup, &worker);
    run.ack();
    match run.recv() {
        Some(Control::Failed { executed, reason }) => {
            assert!(!executed, "{reason}");
            assert!(reason.contains("registered phase"), "{reason}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(run.join().is_err());
    assert!(!h.marker.exists(), "the worker never executed");
    assert!(h.grant().revoked_at.is_some());
    assert!(receipt::scope_entry_blocker(h.root(), "implementer", "TASK-2259").is_some());
    let status = launch_status_at(&h.dir, h.root(), &h.attempt);
    assert!(status.contains("UNKNOWN"), "{status}");
    let err = recover_launch_at(&h.dir, h.root(), &h.attempt, &own_process_chain()).unwrap_err();
    assert!(format!("{err:#}").contains("UNKNOWN"), "{err:#}");
    assert!(wait_gone(&worker));
    receipt::retire_unknown(&h.dir, h.root(), &h.attempt).unwrap();
    assert!(!path.exists());
    assert!(receipt::scope_entry_blocker(h.root(), "implementer", "TASK-2259").is_none());
}

/// A bootstrap killed before release is a failed launch, never acceptance.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_killed_bootstrap_is_not_acceptance() {
    let mut h = harness("TASK-2260");
    let init = h.init(h.sleep_worker("5"));
    let mut run = start(&mut h, init, |step, worker| {
        if step == Step::BeforeRelease {
            // SAFETY: SIGKILL to the gated barrier this test's supervisor owns.
            unsafe {
                libc::kill(worker.pid as libc::pid_t, libc::SIGKILL);
            }
        }
        Ok(())
    });
    let (sup, worker) = run.spawned();
    h.transfer(&sup, &worker);
    run.ack();
    assert!(
        matches!(run.recv(), Some(Control::Failed { .. })),
        "a killed bootstrap is reported as a failure"
    );
    let _ = run.join();
    let r = h.receipt();
    assert_ne!(r.phase, Phase::AcceptedRunning);
    assert!(h.grant().revoked_at.is_some());
    assert!(wait_gone(&worker));
}

/// Strict authorship: a ledger append that cannot persist refuses the launch
/// before release, rolling back exactly the attempt's own grant.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_strict_authorship_write_failure_refuses_before_exec() {
    let mut h = harness("TASK-2261");
    // The ledger path is a directory: the append fails for real.
    std::fs::create_dir_all(h.root().join(".aida/review-authorship.jsonl/blocked")).unwrap();
    let init = h.init(h.marker_worker());
    let mut run = start(&mut h, init, no_faults);
    let (sup, worker) = run.spawned();
    h.transfer(&sup, &worker);
    run.ack();
    match run.recv() {
        Some(Control::Failed { executed, reason }) => {
            assert!(!executed);
            assert!(reason.contains("authorship"), "{reason}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(run.join().is_err());
    assert!(!h.marker.exists());
    assert!(h.grant().revoked_at.is_some());
    let r = h.receipt();
    assert_eq!(
        (r.phase, r.owner),
        (Phase::RefusedBeforeExecution, Owner::Released)
    );
    assert!(
        agent_registry::load_entries(h.root()).is_empty(),
        "never registered"
    );
}

/// A registry (handle/row) write failure after the ledger landed rolls back
/// exactly: the worker never runs, its own grant is revoked, the receipt is
/// released, and the lease is kept for re-entry.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_registry_failure_rolls_back_exactly() {
    let mut h = harness("TASK-2262");
    std::fs::write(h.root().join(".aida/agents"), "not a directory").unwrap();
    let init = h.init(h.marker_worker());
    let mut run = start(&mut h, init, no_faults);
    let (sup, worker) = run.spawned();
    h.transfer(&sup, &worker);
    run.ack();
    match run.recv() {
        Some(Control::Failed { executed, reason }) => {
            assert!(!executed);
            assert!(reason.contains("registry"), "{reason}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(run.join().is_err());
    assert!(!h.marker.exists());
    assert!(h.grant().revoked_at.is_some());
    assert_eq!(h.receipt().phase, Phase::RefusedBeforeExecution);
    assert!(h
        .root()
        .join(".aida/sessions")
        .join(format!("{}.toml", h.lease_id))
        .exists());
}

/// Terminal cleanup is a generation compare-update: once another owner
/// (an explicit recovery) holds the attempt, the old supervisor's final
/// write is refused and never clobbers the newer record.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_generation_cas_cleanup_never_clobbers() {
    let mut h = harness("TASK-2263");
    let init = h.init(h.sleep_worker("1"));
    let mut run = start(&mut h, init, no_faults);
    let (sup, worker) = run.spawned();
    h.transfer(&sup, &worker);
    run.ack();
    assert!(matches!(run.recv(), Some(Control::Accepted { .. })));
    receipt::compare_update(
        &h.dir,
        h.root(),
        &h.attempt,
        |_| Ok(()),
        |r| {
            r.owner = Owner::Released;
            r.phase = Phase::RecoveryRequired;
            r.reason = Some("taken over by a newer owner".into());
        },
    )
    .unwrap();
    let err = run.join().unwrap_err();
    assert!(format!("{err:#}").contains("owned elsewhere"), "{err:#}");
    let r = h.receipt();
    assert_eq!(r.reason.as_deref(), Some("taken over by a newer owner"));
    assert_eq!(r.phase, Phase::RecoveryRequired);
}

/// Before acceptance, no hook can arm a mail wake on the registered row,
/// even one that names a native thread: a detached exec lane's wake is
/// `unsupported`, and arming needs an accepted, pending row.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_pre_acceptance_hook_cannot_arm_wake() {
    let mut h = harness("TASK-2264");
    let init = h.init(h.sleep_worker("0.3"));
    let root = h.root().to_path_buf();
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let record = seen.clone();
    let mut run = start(&mut h, init, move |step, worker| {
        if step == Step::BeforeRelease {
            let row = agent_registry::load_entries(&root)
                .into_iter()
                .map(|(_, e)| e)
                .find(|e| e.pid == worker.pid)
                .expect("registered before release");
            let mut hooked = row.clone();
            hooked.native_session_id = Some("019a0000-0000-7000-8000-000000000001".into());
            let armed = crate::mail_wake::arm_session(
                &root,
                &hooked,
                Some("codex"),
                || Some(true),
                |_| anyhow::bail!("the wake service must never be installed here"),
            );
            record.lock().unwrap().push(format!(
                "{:?} {:?}",
                row.mail_wake,
                armed.map(|a| a.state)
            ));
        }
        Ok(())
    });
    let (sup, worker) = run.spawned();
    h.transfer(&sup, &worker);
    run.ack();
    assert!(matches!(run.recv(), Some(Control::Accepted { .. })));
    run.join().unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.as_slice(),
        [format!(
            "{:?} None",
            Some(agent_registry::MailWake::Unsupported)
        )]
    );
}

/// `recover-launch` fails CLOSED on a FRESH scan: a process still in the
/// worktree, or one left in the dead worker's process group, blocks it; an
/// unrecorded start identity is unknown; nothing is revoked on refusal; once
/// everything is provably gone, the very next call reconciles.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn story_2254_bg_recover_launch_fresh_scan_fails_closed() {
    let h = harness("TASK-2265");
    let own = own_process_chain();
    // A dead worker that left a descendant in its process group.
    let mut leader = std::process::Command::new("sh")
        .args(["-c", "sleep 30 & echo $!"])
        .stdout(std::process::Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let leader_id = ProcId {
        pid: leader.id(),
        start: aida_core::liveness::process_start_identity(leader.id()).unwrap(),
    };
    let mut out = String::new();
    BufReader::new(leader.stdout.take().unwrap())
        .read_line(&mut out)
        .unwrap();
    let escaped: u32 = out.trim().parse().unwrap();
    leader.wait().unwrap();
    assert_eq!(leader_id.state(), receipt::ProcState::Gone);
    let set_worker = |w: ProcId| {
        receipt::compare_update(
            &h.dir,
            h.root(),
            &h.attempt,
            |_| Ok(()),
            move |r| {
                r.phase = Phase::RecoveryRequired;
                r.owner = Owner::Released;
                r.worker = Some(w);
            },
        )
        .unwrap();
    };
    set_worker(leader_id.clone());
    let err = recover_launch_at(&h.dir, h.root(), &h.attempt, &own).unwrap_err();
    assert!(format!("{err:#}").contains("process group"), "{err:#}");
    // SAFETY: SIGKILL to the escaped descendant this test spawned.
    unsafe {
        libc::kill(escaped as libc::pid_t, libc::SIGKILL);
    }
    let escaped_id = ProcId {
        pid: escaped,
        start: "unused".into(),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::fs::symlink_metadata(format!("/proc/{}", escaped_id.pid)).is_ok()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // A missing start identity is unknown, never gone.
    set_worker(ProcId {
        pid: leader_id.pid,
        start: String::new(),
    });
    let err = recover_launch_at(&h.dir, h.root(), &h.attempt, &own).unwrap_err();
    assert!(format!("{err:#}").contains("UNKNOWN"), "{err:#}");
    set_worker(leader_id);
    // A process still running in the worktree.
    let mut occupant = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&h.worktree)
        .spawn()
        .unwrap();
    let err = recover_launch_at(&h.dir, h.root(), &h.attempt, &own).unwrap_err();
    assert!(format!("{err:#}").contains("running in"), "{err:#}");
    assert!(h.grant().revoked_at.is_none(), "nothing revoked on refusal");
    let _ = occupant.kill();
    let _ = occupant.wait();
    // Fresh scan: the very next call sees it gone and reconciles.
    let done = recover_launch_at(&h.dir, h.root(), &h.attempt, &own).unwrap();
    assert!(done.contains("recovered"), "{done}");
    assert!(h.grant().revoked_at.is_some());
    assert_eq!(h.receipt().phase, Phase::Exited);
}

/// The occupancy scan itself fails closed on an unreadable process table or
/// an unreadable process of this user.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_occupancy_scan_unreadable_is_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let none = std::collections::HashSet::new();
    let probe = receipt::OccupancyProbe {
        worktree: Some(tmp.path()),
        ..Default::default()
    };
    assert!(matches!(
        receipt::scan_occupancy_in(&tmp.path().join("missing"), &probe, &none),
        receipt::Occupancy::Unknown(_)
    ));
    assert!(matches!(
        receipt::scan_occupancy_in(tmp.path(), &probe, &none),
        receipt::Occupancy::Unknown(_)
    ));
    // A process of this user whose stat cannot be read.
    std::fs::create_dir_all(tmp.path().join("4242/stat")).unwrap();
    assert!(matches!(
        receipt::scan_occupancy_in(tmp.path(), &probe, &none),
        receipt::Occupancy::Unknown(_)
    ));
    // The real table, with nothing in an empty tree, is clear once bounded
    // by the attempt's first process (this test's own start): an unreadable
    // older process of this user (a PAM helper) cannot be the attempt's.
    let empty = tempfile::tempdir().unwrap();
    let probe = receipt::OccupancyProbe {
        worktree: Some(empty.path()),
        not_before: ProcId::current().unwrap().start_tick(),
        ..Default::default()
    };
    assert_eq!(
        receipt::scan_occupancy(&probe, &own_process_chain()),
        receipt::Occupancy::Clear
    );
}

/// `aida agent launch-status` and `aida agent recover-launch` through the
/// CLI dispatcher, from inside the project.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_launch_status_and_recover_launch_cli() {
    if !crate::test_env::run_path_isolated_test() {
        return;
    }
    let h = harness("TASK-2266");
    std::env::set_current_dir(h.root()).unwrap();
    let dispatch = |args: &[&str]| {
        let cli = crate::TEST_CLI.try_parse_from(args).unwrap();
        let AgentArgs { inner } = cli.command.registered::<AgentArgs>();
        handle_agent_command(&inner)
    };
    dispatch(&["aida", "agent", "launch-status", &h.attempt]).unwrap();
    let status = launch_status_at(&h.dir, h.root(), &h.attempt);
    assert!(
        status.contains("prepared") && status.contains("TASK-2266"),
        "{status}"
    );
    // The launcher (this live test process) still owns it: refused.
    let err = dispatch(&["aida", "agent", "recover-launch", &h.attempt]).unwrap_err();
    assert!(format!("{err:#}").contains("live launcher"), "{err:#}");
    // A readable receipt is recovered, never retired.
    assert!(dispatch(&[
        "aida",
        "agent",
        "recover-launch",
        &h.attempt,
        "--retire-unknown"
    ])
    .is_err());
    assert!(h.grant().revoked_at.is_none());
}

/// Exec corroboration: an exit before the vendor is observed, or a process
/// that never becomes the expected executable, is a failure; the vendor
/// executable at the worker PID is acceptance.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_corroborate_exec_outcomes() {
    let sleep = canonical_program("sleep");
    let expected = ExpectedExec::Native {
        path: sleep.clone(),
    };
    let within = std::time::Duration::from_millis(400);
    let mut quick = std::process::Command::new("true").spawn().unwrap();
    let _ = wait_exit_unreaped(quick.id());
    let err = corroborate_exec(&mut quick, &expected, within).unwrap_err();
    assert!(err.contains("exited"), "{err}");
    let mut sleeper = std::process::Command::new(&sleep).arg("5").spawn().unwrap();
    assert_eq!(
        corroborate_exec(&mut sleeper, &expected, within).unwrap(),
        sleep
    );
    let other = ExpectedExec::Native {
        path: canonical_program("sh"),
    };
    let err = corroborate_exec(&mut sleeper, &other, within).unwrap_err();
    assert!(err.contains("cannot be proven"), "{err}");
    let _ = sleeper.kill();
    let _ = sleeper.wait();
}

/// The dispatcher's abort stops the gated worker only when it is provably
/// the supervisor's own child leading its own group, then kills and reaps
/// the supervisor; a foreign process is never signalled.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn story_2254_bg_abort_kills_owned_barrier_and_reaps_supervisor() {
    let spawn_fake_supervisor = || {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "set -m; sleep 30 & echo $!; wait"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let pid: u32 = line.trim().parse().unwrap();
        let start = aida_core::liveness::process_start_identity(pid).unwrap();
        (child, ProcId { pid, start })
    };
    let (mut sup, barrier) = spawn_fake_supervisor();
    // SAFETY: getpgid only reads the group of a process this test spawned.
    let leads = unsafe { libc::getpgid(barrier.pid as libc::pid_t) } == barrier.pid as libc::pid_t;
    let err = abort_supervisor(&mut sup, Some(&barrier), "stopped".into());
    assert!(format!("{err}").starts_with("stopped"));
    assert!(sup.try_wait().unwrap().is_some(), "supervisor reaped");
    if leads {
        assert!(wait_gone(&barrier), "the owned barrier was stopped");
    } else {
        // The shell gave no job control: never signal a non-leader.
        let _ = std::process::Command::new("kill")
            .arg(barrier.pid.to_string())
            .status();
    }
    // A foreign process (not the supervisor's child) is never signalled.
    let mut foreign = std::process::Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let foreign_id = ProcId {
        pid: foreign.id(),
        start: aida_core::liveness::process_start_identity(foreign.id()).unwrap(),
    };
    let (mut sup, barrier) = spawn_fake_supervisor();
    let _ = abort_supervisor(&mut sup, Some(&foreign_id), "stopped".into());
    assert_eq!(foreign_id.state(), receipt::ProcState::Live);
    let _ = foreign.kill();
    let _ = foreign.wait();
    let _ = std::process::Command::new("kill")
        .arg(barrier.pid.to_string())
        .status();
}

/// Terminal cleanup signals the worker's group while the exited leader is
/// still an unreaped zombie (its pid, and so the group id, cannot be reused
/// by an unrelated process), then reaps it.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn story_2254_bg_until_exit_signals_group_before_reaping_leader() {
    let h = harness("TASK-2267");
    let mut leader = std::process::Command::new("sh")
        .args(["-c", "sleep 30 & echo $!"])
        .stdout(std::process::Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let worker = ProcId {
        pid: leader.id(),
        start: aida_core::liveness::process_start_identity(leader.id()).unwrap(),
    };
    let mut line = String::new();
    BufReader::new(leader.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let member: u32 = line.trim().parse().unwrap();
    wait_exit_unreaped(worker.pid).unwrap();
    assert!(
        std::fs::symlink_metadata(format!("/proc/{}", worker.pid)).is_ok(),
        "observed without reaping: the leader is still a zombie"
    );
    h.transfer(&h.me, &worker);
    let init = h.init(h.sleep_worker("1"));
    supervise_until_exit(&h.dir, &h.attempt, &init, &h.me, &worker, leader, None).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::fs::symlink_metadata(format!("/proc/{member}")).is_ok()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        std::fs::symlink_metadata(format!("/proc/{member}")).is_err(),
        "the group member was stopped"
    );
    let r = h.receipt();
    assert_eq!(
        (r.phase, r.terminal.unwrap().code),
        (Phase::Exited, Some(0))
    );
    drop(h.env);
}

/// The normal exit runs the same occupancy scan as `recover-launch` before
/// it releases the scope: a process left running in the worktree (outside
/// the worker's group, so the exit signal never reaches it) keeps the scope
/// held as recovery-required instead of a clean exit.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_until_exit_holds_scope_while_worktree_occupied() {
    let h = harness("TASK-2268");
    let mut lingerer = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&h.worktree)
        .process_group(0)
        .spawn()
        .unwrap();
    let leader = std::process::Command::new("true")
        .process_group(0)
        .spawn()
        .unwrap();
    let worker = ProcId {
        pid: leader.id(),
        start: aida_core::liveness::process_start_identity(leader.id()).unwrap(),
    };
    h.transfer(&h.me, &worker);
    let init = h.init(h.sleep_worker("1"));
    supervise_until_exit(&h.dir, &h.attempt, &init, &h.me, &worker, leader, None).unwrap();
    let r = h.receipt();
    let _ = lingerer.kill();
    let _ = lingerer.wait();
    assert_eq!(r.phase, Phase::RecoveryRequired);
    assert!(r.phase.holds_scope(), "the scope stays held");
    let reason = r.reason.unwrap_or_default();
    assert!(reason.contains("not provably over"), "{reason}");
    drop(h.env);
}

/// Plain git in `dir`, hooks off, a fixed host identity.
// trace:TASK-2712 | ai:claude
#[cfg(target_os = "linux")]
fn host_git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(["-c", "user.name=host", "-c", "user.email=host@localhost"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// What one detached lane's post-exit step saw: the step's calls
/// `(tree, spec, clean_exit)`, the worktree's last commit subject, whether
/// the child's work is still uncommitted, and the final receipt.
// trace:TASK-2712 | ai:claude
#[cfg(target_os = "linux")]
struct PostExitCase {
    calls: Vec<(std::path::PathBuf, String, bool)>,
    subject: String,
    pending: bool,
    receipt: LaunchReceipt,
}

/// Drive a real detached lane end to end through `supervise_until_exit`:
/// the worktree is a git checkout, the child writes `pending.txt` there and
/// exits with `code`, and the lane's own post-exit step runs on this thread
/// with a stand-in commit (plain git, hooks off, the step's own message) in
/// place of the bwrap'd host commit. `linger` leaves a process in the
/// worktree, so the exit scan cannot prove the lane over.
// trace:TASK-2712 | ai:claude
#[cfg(target_os = "linux")]
fn post_exit_case(role: &str, code: i32, linger: bool) -> PostExitCase {
    let h = harness("TASK-2712");
    crate::test_git::commit_fixture(&h.worktree);
    let mut lingerer = linger.then(|| {
        std::process::Command::new("sleep")
            .arg("30")
            .current_dir(&h.worktree)
            .process_group(0)
            .spawn()
            .unwrap()
    });
    let leader = std::process::Command::new("sh")
        .args(["-c", &format!("echo work > pending.txt; exit {code}")])
        .current_dir(&h.worktree)
        .process_group(0)
        .spawn()
        .unwrap();
    let worker = ProcId {
        pid: leader.id(),
        start: aida_core::liveness::process_start_identity(leader.id()).unwrap(),
    };
    h.transfer(&h.me, &worker);
    let mut init = h.init(h.sleep_worker("1"));
    init.role = role.to_string();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&calls);
    let step: Box<crate::ship_from_seat::PostExitStep> = Box::new(
        move |_main: &std::path::Path,
              tree: &std::path::Path,
              spec: &str,
              lane: &str,
              clean: bool| {
            seen.lock()
                .unwrap()
                .push((tree.to_path_buf(), spec.to_string(), clean));
            host_git(tree, &["add", "-A"]);
            let message = crate::ship_from_seat::post_exit_message(
                Some(&aida_core::Requirement::new(
                    "Native lane test".into(),
                    String::new(),
                )),
                spec,
                lane,
                clean,
            )
            .unwrap();
            host_git(tree, &["commit", "-q", "--no-verify", "-m", &message]);
            crate::ship_from_seat::PostExitCommit::Committed(host_git(tree, &["rev-parse", "HEAD"]))
        },
    );
    crate::ship_from_seat::with_post_exit_step(step, || {
        supervise_until_exit(&h.dir, &h.attempt, &init, &h.me, &worker, leader, None).unwrap()
    });
    if let Some(l) = lingerer.as_mut() {
        let _ = l.kill();
        let _ = l.wait();
    }
    let case = PostExitCase {
        calls: calls.lock().unwrap().clone(),
        subject: host_git(&h.worktree, &["log", "-1", "--format=%s"]),
        pending: !host_git(&h.worktree, &["status", "--porcelain"]).is_empty(),
        receipt: h.receipt(),
    };
    drop(h.env);
    case
}

/// Advisor F3: a detached implementer lane whose child left pending work and
/// exited 0 ends with the host's post-exit commit, run by the lane itself
/// (`supervise_until_exit` -> `post_exit_seat_commit`), with a completing
/// trailer; the receipt still records a clean exit.
// trace:TASK-2712 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn task_2712_bg_lane_commits_pending_work_after_clean_exit() {
    let case = post_exit_case("implementer", 0, false);
    assert_eq!(case.calls.len(), 1, "the post-exit step ran once");
    let (tree, spec, clean) = &case.calls[0];
    assert!(tree.ends_with("wt"), "{tree:?}");
    assert_eq!((spec.as_str(), *clean), ("TASK-2712", true));
    assert!(!case.pending, "the child's work is committed");
    assert!(case.subject.ends_with("(TASK-2712)"), "{}", case.subject);
    assert_eq!(case.receipt.phase, Phase::Exited);
}

/// Advisor F3/F4: a failed child exit still has its leftovers committed by
/// the lane, but as salvage with `(Refs SPEC)`, never a completing trailer.
// trace:TASK-2712 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn task_2712_bg_lane_failed_exit_commits_salvage_with_refs() {
    let case = post_exit_case("implementer", 3, false);
    assert_eq!(case.calls.len(), 1);
    assert!(!case.calls[0].2, "a non-zero exit is not clean");
    assert!(!case.pending);
    assert!(
        case.subject.ends_with("(Refs TASK-2712)"),
        "{}",
        case.subject
    );
}

/// Advisor F3: the role gate: a non-implementer lane's post-exit step never
/// commits, so its tree keeps the work uncommitted.
// trace:TASK-2712 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn task_2712_bg_lane_non_implementer_commits_nothing() {
    let case = post_exit_case("reviewer", 0, false);
    assert!(case.calls.is_empty(), "{:?}", case.calls);
    assert!(case.pending, "the reviewer's tree was committed");
    assert_eq!(case.subject, "fixture");
}

/// Advisor F3: the `exit_clear = false` branch: while the exit scan cannot
/// prove the lane over (a process still runs in the tree), nothing commits.
// trace:TASK-2712 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn task_2712_bg_lane_unproven_exit_commits_nothing() {
    let case = post_exit_case("implementer", 0, true);
    assert!(case.calls.is_empty(), "{:?}", case.calls);
    assert!(case.pending);
    assert_eq!(case.receipt.phase, Phase::RecoveryRequired);
}

// trace:BUG-2467 | ai:codex
#[test]
#[cfg(target_os = "linux")]
fn bug_2467_detached_registry_retains_requested_selection_for_resume() {
    for selection in [Vec::new(), vec!["reviewer".to_string()]] {
        let mut h = harness_delegating("BUG-2467", "advisor", &selection);
        let effective = h.grant().delegable_seats;
        assert_eq!(
            effective,
            if selection.is_empty() {
                vec![
                    "advisor".to_string(),
                    "implementer".into(),
                    "reviewer".into(),
                ]
            } else {
                selection.clone()
            }
        );
        let mut init = h.init(h.sleep_worker("1"));
        init.role = "advisor".into();
        init.delegate_seats = selection.clone();
        // Exercise the actual supervisor wire format as well as registration.
        let init = serde_json::from_slice(&serde_json::to_vec(&init).unwrap()).unwrap();
        let mut run = start(&mut h, init, no_faults);
        let (sup, worker) = run.spawned();
        h.transfer(&sup, &worker);
        run.ack();
        assert!(matches!(run.recv(), Some(Control::Accepted { .. })));
        run.join().unwrap();
        let (_, row) = agent_registry::load_entries(h.root())
            .into_iter()
            .find(|(_, row)| row.pid == worker.pid)
            .expect("supervisor retains the ended worker row");
        assert_eq!(row.delegate_seats, selection);
        assert!(row.ended_at.is_some());
        // This is resume's delegation input: no selection inherits the NEW
        // parent, while explicit narrowing still refuses a removed seat.
        h.env.set_key("AIDA_USER", SUBJECT);
        let less = seat::test_support::mint_grant_for(
            h.root(),
            SUBJECT,
            "advisor",
            &["advisor", "implementer"],
        );
        h.env.set_key(GRANT_ENV, &less);
        let resumed = seat::issue_launch_child_with_delegation(
            h.root(),
            "advisor",
            AGENT,
            false,
            &row.delegate_seats,
        );
        if selection.is_empty() {
            assert_eq!(resumed.unwrap().delegable_seats, ["advisor", "implementer"]);
        } else {
            assert!(format!("{:#}", resumed.unwrap_err()).contains("does not delegate `reviewer`"));
        }
    }
}

// trace:BUG-2511 | ai:codex
#[test]
fn bug_2511_occupancy_unreadable_foreign_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let process = tmp.path().join("4242");
    std::fs::create_dir(&process).unwrap();
    std::fs::write(process.join("stat"), "4242 (foreign process) S 1 42 43").unwrap();
    std::fs::write(process.join("cmdline"), b"unrelated\0").unwrap();
    // A regular file makes read_link fail deterministically, including when
    // tests run as root; unlike a missing cwd, this is not a vanished process.
    std::fs::write(process.join("cwd"), b"unreadable link").unwrap();
    let none = std::collections::HashSet::new();
    let mut probe = receipt::OccupancyProbe {
        worktree: Some(tmp.path()),
        attempt: Some("our-attempt"),
        ..Default::default()
    };
    let scan = |probe: &receipt::OccupancyProbe<'_>, ours| {
        receipt::scan_occupancy_in_with_owner(tmp.path(), probe, &none, |_| ours)
    };
    assert_eq!(scan(&probe, false), receipt::Occupancy::Clear);
    assert!(matches!(scan(&probe, true), receipt::Occupancy::Unknown(why) if why.contains("cwd")));
    // Readable evidence still blocks even for a foreign process.
    probe.group = Some(42);
    assert!(
        matches!(scan(&probe, false), receipt::Occupancy::Occupied(why) if why.contains("process group"))
    );
    probe.group = None;
    probe.session = Some(43);
    assert!(
        matches!(scan(&probe, false), receipt::Occupancy::Occupied(why) if why.contains("session"))
    );
    probe.session = None;
    std::fs::write(process.join("cmdline"), b"worker\0our-attempt\0").unwrap();
    assert!(
        matches!(scan(&probe, false), receipt::Occupancy::Occupied(why) if why.contains("attempt"))
    );
    std::fs::write(process.join("cmdline"), b"unrelated\0").unwrap();
    std::fs::remove_file(process.join("cwd")).unwrap();
    std::os::unix::fs::symlink(tmp.path(), process.join("cwd")).unwrap();
    assert!(
        matches!(scan(&probe, false), receipt::Occupancy::Occupied(why) if why.contains("running in"))
    );
}

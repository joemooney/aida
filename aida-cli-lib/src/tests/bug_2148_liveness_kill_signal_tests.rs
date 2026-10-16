// trace:BUG-2148 trace:TASK-1782 | ai:antigravity
//! BUG-2148: `aida agent stop` and `session end` leaked-process kill signals
//! checked only `pid_is_alive` before signalling, with no start-time identity check.
//! If an agent or leaked session exited and its PID was recycled onto an unrelated
//! process, `terminate_pids_with_grace` would SIGTERM/SIGKILL the unrelated process.
//! These tests verify that signals are strictly gated on
//! `aida_core::liveness::process_identity_matches_strict`: an unknown, mismatched,
//! or recycled start-time identity is NEVER signalled.

use super::*;
use std::path::Path;

/// RAII guard to prevent leaked processes in tests on failure/panic.
struct ProcessGuard(Option<std::process::Child>);

impl ProcessGuard {
    fn new(child: std::process::Child) -> Self {
        Self(Some(child))
    }
    fn id(&self) -> u32 {
        self.0.as_ref().expect("child alive").id()
    }
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.0.as_mut().expect("child alive").try_wait()
    }
    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.0.as_mut().expect("child alive").wait()
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn bug_2148_terminate_pids_with_grace_never_signals_recycled_or_unknown_identity() {
    let mut child = ProcessGuard::new(
        std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn sleep"),
    );
    let pid = child.id();

    // Verify child is alive in OS
    assert!(aida_core::liveness::pid_is_alive(pid));

    // 1. Recycled PID fixture: same PID, different / historical start time
    let recycled_target = TerminateTarget::new(pid, Some("2000-01-01T00:00:00+00:00".to_string()));
    let survivors = terminate_pids_with_grace(&[recycled_target], 0);
    assert!(
        survivors.is_empty(),
        "unconfirmed target must not be reported as a surviving target"
    );
    assert!(
        child.try_wait().unwrap().is_none(),
        "recycled PID fixture must NOT be signalled"
    );

    // 2. Unknown start time: None
    let unknown_target = TerminateTarget::new(pid, None);
    let survivors = terminate_pids_with_grace(&[unknown_target], 0);
    assert!(survivors.is_empty());
    assert!(
        child.try_wait().unwrap().is_none(),
        "unknown identity (None) must NOT be signalled"
    );

    // 3. Garbage / unparsable start time
    let garbage_target = TerminateTarget::new(pid, Some("garbage".to_string()));
    let survivors = terminate_pids_with_grace(&[garbage_target], 0);
    assert!(survivors.is_empty());
    assert!(
        child.try_wait().unwrap().is_none(),
        "garbage identity must NOT be signalled"
    );

    // 4. Non-canonical tick format
    let non_canonical = TerminateTarget::new(pid, Some("linux-starttime:007".to_string()));
    let survivors = terminate_pids_with_grace(&[non_canonical], 0);
    assert!(survivors.is_empty());
    assert!(
        child.try_wait().unwrap().is_none(),
        "non-canonical tick identity must NOT be signalled"
    );

    // 5. Positive control: genuinely matching start time identity IS signalled
    let real_identity =
        aida_core::liveness::process_start_identity(pid).expect("live start identity");
    let matching_target = TerminateTarget::new(pid, Some(real_identity));
    let survivors = terminate_pids_with_grace(&[matching_target], 0);
    assert!(
        survivors.is_empty(),
        "plain sleep must not survive SIGTERM+SIGKILL"
    );

    let status = child.wait().expect("wait on child");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        assert_eq!(
            status.signal(),
            Some(15),
            "positively confirmed process must be terminated with SIGTERM"
        );
    }
}

#[test]
fn bug_2148_agent_stop_never_signals_unconfirmed_identity_and_keeps_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida/agents")).unwrap();
    std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();

    let mut unrelated = ProcessGuard::new(
        std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn unrelated child"),
    );
    let pid = unrelated.id();

    // Case 1: Agent registered with recycled PID (mismatched start time).
    // Must NOT signal the live process, MUST keep the entry, and MUST exit nonzero.
    let entry_path = root.join(".aida/agents").join(format!("claude-{pid}.toml"));
    let root_quoted = aida_core::toml_quote::toml_string(root.to_str().unwrap());
    let recycled_toml = format!(
        r#"id = "claude-{pid}"
agent_type = "claude"
pid = {pid}
pid_start = "2000-01-01T00:00:00+00:00"
name = "victim-agent"
worktree_path = {root_quoted}
source = "agent-launcher"
started_at = "2026-10-15T00:00:00Z"
last_active_at = "2026-10-15T00:00:00Z"
"#
    );
    std::fs::write(&entry_path, &recycled_toml).unwrap();

    // Call agent_stop_in_project: must fail (exit nonzero) because unconfirmed live process exists
    let res = crate::agent_registry_root::agent_stop_in_project(root, "victim-agent");
    assert!(
        res.is_err(),
        "agent_stop must fail when an agent has unconfirmed live processes: {res:?}"
    );

    // The unrelated process with the recycled PID must NOT have been signalled!
    assert!(
        unrelated.try_wait().unwrap().is_none(),
        "unrelated process with recycled PID must NOT be killed"
    );

    // The registry entry MUST be kept in place (never reaped for a live process with unconfirmed identity)
    assert!(
        entry_path.exists(),
        "registry entry for unconfirmed live process must be kept in place"
    );
    let err_msg = res.unwrap_err().to_string();
    assert!(
        err_msg.contains("recycled PID") && err_msg.contains("aida agent gc"),
        "error must suggest aida agent gc for recycled PID: {err_msg}"
    );

    // Case 1b (TASK-1782): Agent registered with unknown start identity (pid_start missing/None).
    // Must NOT signal the live process, MUST keep the entry, and MUST exit nonzero with ps -p hint.
    let unknown_entry_path = root
        .join(".aida/agents")
        .join(format!("claude-{pid}-unknown.toml"));
    let unknown_toml = format!(
        r#"id = "claude-{pid}-unknown"
agent_type = "claude"
pid = {pid}
name = "unknown-agent"
worktree_path = {root_quoted}
source = "agent-launcher"
started_at = "2026-10-15T00:00:00Z"
last_active_at = "2026-10-15T00:00:00Z"
"#
    );
    std::fs::write(&unknown_entry_path, &unknown_toml).unwrap();

    let res = crate::agent_registry_root::agent_stop_in_project(root, "unknown-agent");
    assert!(
        res.is_err(),
        "agent_stop must fail when an agent has unknown start identity: {res:?}"
    );
    let err_msg = res.unwrap_err().to_string();
    assert!(
        err_msg.contains("unknown identity") && err_msg.contains("ps -p"),
        "error must suggest ps -p inspection for unknown identity: {err_msg}"
    );
    assert!(
        unrelated.try_wait().unwrap().is_none(),
        "unrelated process with unknown identity must NOT be killed"
    );
    assert!(
        unknown_entry_path.exists(),
        "registry entry for unknown live process must be kept in place"
    );
    let _ = std::fs::remove_file(&unknown_entry_path);

    // Case 1c (TASK-1782 round 1 finding): Agent registered with unparseable/garbage start identity.
    // Must NOT be classified as recycled (which requires provable start difference);
    // must be treated as unknown identity, NOT signalled, entry kept, and exit nonzero with ps -p hint.
    let garbage_entry_path = root
        .join(".aida/agents")
        .join(format!("claude-{pid}-garbage.toml"));
    let garbage_toml = format!(
        r#"id = "claude-{pid}-garbage"
agent_type = "claude"
pid = {pid}
pid_start = "garbage"
name = "garbage-agent"
worktree_path = {root_quoted}
source = "agent-launcher"
started_at = "2026-10-15T00:00:00Z"
last_active_at = "2026-10-15T00:00:00Z"
"#
    );
    std::fs::write(&garbage_entry_path, &garbage_toml).unwrap();

    let res = crate::agent_registry_root::agent_stop_in_project(root, "garbage-agent");
    assert!(
        res.is_err(),
        "agent_stop must fail when an agent has unparseable/garbage start identity: {res:?}"
    );
    let err_msg = res.unwrap_err().to_string();
    assert!(
        err_msg.contains("unknown identity") && err_msg.contains("ps -p"),
        "error must suggest ps -p inspection for garbage start identity: {err_msg}"
    );
    assert!(
        !err_msg.contains("recycled PID"),
        "garbage identity must not be classified as recycled: {err_msg}"
    );
    assert!(
        unrelated.try_wait().unwrap().is_none(),
        "unrelated process with garbage identity must NOT be killed"
    );
    assert!(
        garbage_entry_path.exists(),
        "registry entry for garbage live process must be kept in place"
    );
    let _ = std::fs::remove_file(&garbage_entry_path);

    // Case 1d (TASK-1782 round 1 finding): Agent entry with invalid PID (<= 1).
    // Must NOT signal PID 1 / 0, MUST keep the entry, and MUST report as corrupt entry without ps -p kill hint.
    let corrupt_entry_path = root.join(".aida/agents").join("claude-1-corrupt.toml");
    let corrupt_toml = format!(
        r#"id = "claude-1-corrupt"
agent_type = "claude"
pid = 1
name = "corrupt-agent"
worktree_path = {root_quoted}
source = "agent-launcher"
started_at = "2026-10-15T00:00:00Z"
last_active_at = "2026-10-15T00:00:00Z"
"#
    );
    std::fs::write(&corrupt_entry_path, &corrupt_toml).unwrap();

    let res = crate::agent_registry_root::agent_stop_in_project(root, "corrupt-agent");
    assert!(
        res.is_err(),
        "agent_stop must fail when an agent has invalid PID <= 1: {res:?}"
    );
    let err_msg = res.unwrap_err().to_string();
    assert!(
        err_msg.contains("corrupt registry entry") && !err_msg.contains("ps -p 1"),
        "error must report corrupt entry and must NOT suggest killing PID 1: {err_msg}"
    );
    assert!(
        corrupt_entry_path.exists(),
        "corrupt registry entry must be kept in place"
    );
    let _ = std::fs::remove_file(&corrupt_entry_path);

    // Case 2: Truly dead process (PID gone from OS) is reaped with exit 0
    let dead_entry_path = root.join(".aida/agents").join("claude-99999999.toml");
    let dead_toml = format!(
        r#"id = "claude-99999999"
agent_type = "claude"
pid = 99999999
pid_start = "2000-01-01T00:00:00+00:00"
name = "dead-agent"
worktree_path = {root_quoted}
source = "agent-launcher"
started_at = "2026-10-15T00:00:00Z"
last_active_at = "2026-10-15T00:00:00Z"
"#
    );
    std::fs::write(&dead_entry_path, &dead_toml).unwrap();

    let res = crate::agent_registry_root::agent_stop_in_project(root, "dead-agent");
    assert!(
        res.is_ok(),
        "agent_stop for dead process should succeed: {res:?}"
    );
    assert!(
        !dead_entry_path.exists(),
        "stale registry entry for dead process must be reaped"
    );

    // Case 3: Agent registered with matching start identity IS stopped
    let real_identity = aida_core::liveness::process_start_identity(pid).expect("identity");
    let live_toml = format!(
        r#"id = "claude-{pid}"
agent_type = "claude"
pid = {pid}
pid_start = "{real_identity}"
name = "live-agent"
worktree_path = {root_quoted}
source = "agent-launcher"
started_at = "2026-10-15T00:00:00Z"
last_active_at = "2026-10-15T00:00:00Z"
"#
    );
    std::fs::write(&entry_path, &live_toml).unwrap();

    let res = crate::agent_registry_root::agent_stop_in_project(root, "live-agent");
    assert!(res.is_ok(), "agent_stop should succeed: {res:?}");

    // The live process MUST have been terminated
    let status = unrelated.wait().expect("child should exit");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        assert_eq!(status.signal(), Some(15));
    }

    // Registry entry reaped
    assert!(!entry_path.exists());
}

#[cfg(unix)]
#[test]
fn bug_2148_terminate_pids_sigkill_recheck_aborts_kill_if_identity_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let ready_file = tmp.path().join("trap-ready");

    // Process traps SIGTERM so it stays alive after Phase 1 (SIGTERM).
    // TASK-1782: write ready marker file after 'trap' is active to eliminate spawn/trap race.
    let child = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "trap '' TERM; touch '{}'; while true; do sleep 1; done",
            ready_file.display()
        ))
        .spawn()
        .expect("spawn trapping process");
    let mut guard = ProcessGuard::new(child);
    let pid = guard.id();

    // Wait until trap handler is installed in child (wait up to ~5s)
    for _ in 0..500 {
        if ready_file.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        ready_file.exists(),
        "trap handler in child process must be installed"
    );

    let target = TerminateTarget::new(pid, Some("initial-start".to_string()));
    let mut check_count = 0;

    let survivors = terminate_pids_with_grace_with(
        &[target],
        0, // 0s grace in test
        |_target_pid, _start| {
            check_count += 1;
            if check_count == 1 {
                // Phase 1 (SIGTERM): confirm identity so SIGTERM is sent
                true
            } else {
                // Phase 2 (SIGKILL): identity mismatch (e.g. recycled PID) -> abort SIGKILL
                false
            }
        },
        |pid, start| aida_core::liveness::process_identity_is_alive(pid, start),
    );

    assert_eq!(
        check_count, 2,
        "identity must be checked in both SIGTERM and SIGKILL phases"
    );
    assert_eq!(
        survivors,
        vec![pid],
        "process must be reported as survivor because SIGKILL was aborted"
    );
    assert!(
        guard.try_wait().unwrap().is_none(),
        "process must still be running because SIGKILL was NOT sent"
    );
}

#[test]
fn bug_2148_session_end_leaked_process_identity_gating() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let mut child = ProcessGuard::new(
        std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn sleep"),
    );
    let pid = child.id();

    let mock_session = aida_core::liveness::LiveSession {
        pid,
        cwd: root.to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    };

    // 1. collect_leaked_claude_targets captures start identity
    let leaked = crate::session_end::collect_leaked_claude_targets(vec![mock_session.clone()]);
    assert_eq!(leaked.len(), 1);
    assert_eq!(leaked[0].0.pid, pid);
    assert!(
        leaked[0].1.is_some(),
        "start identity must be recorded for live leaked session"
    );

    // 2. leaked_to_terminate_targets propagates identity
    let targets = crate::session_end::leaked_to_terminate_targets(&leaked);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].pid, pid);
    assert_eq!(targets[0].recorded_start_time, leaked[0].1);

    // 3. Recycled identity on leaked session: terminate_pids_with_grace refuses to signal
    let recycled_leaked = vec![(
        mock_session.clone(),
        Some("2000-01-01T00:00:00+00:00".to_string()),
    )];
    let recycled_targets = crate::session_end::leaked_to_terminate_targets(&recycled_leaked);
    let survivors = terminate_pids_with_grace(&recycled_targets, 0);
    assert!(survivors.is_empty());
    assert!(
        child.try_wait().unwrap().is_none(),
        "recycled leaked session must NOT be signalled"
    );

    // 4. Positive control: genuine identity terminates process
    let real_targets = crate::session_end::leaked_to_terminate_targets(&leaked);
    let survivors = terminate_pids_with_grace(&real_targets, 0);
    assert!(survivors.is_empty());
    let status = child.wait().expect("child should be terminated");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        assert_eq!(status.signal(), Some(15));
    }
}

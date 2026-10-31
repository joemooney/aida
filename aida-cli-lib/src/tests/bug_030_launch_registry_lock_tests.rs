// trace:BUG-b6b1278890774ef993d2ebc327862470-030 | ai:claude
//
// `aida agent new` spawned claude, then failed to register it because the
// handle namespace lock was busy for more than the 250 ms SessionStart
// budget. The `?` returned from the launcher while claude was still running,
// so the shell took the terminal back and claude died.
use super::*;
use std::time::Duration;

/// Hold the namespace lock on another thread for `hold`, returning once it
/// is held.
fn hold_namespace(root: &std::path::Path, hold: Duration) -> std::thread::JoinHandle<()> {
    let root = root.to_path_buf();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let lock = crate::agent_handle::lock_namespace(&root).unwrap();
        held_tx.send(()).unwrap();
        std::thread::sleep(hold);
        drop(lock);
    });
    held_rx.recv().unwrap();
    holder
}

fn register(root: &std::path::Path) -> Result<agent_registry::AgentRegistryEntry> {
    agent_registry::register_spawned_agent(
        root,
        "claude",
        std::process::id(),
        Some("advisor".into()),
        None,
        root.to_path_buf(),
        None,
        Some("claude-advisor-07".into()),
        None,
        None,
        None,
    )
}

/// Records whether `marker` existed when the first byte was written, so a
/// test can prove output came after the child finished.
#[cfg(unix)]
struct AfterExitWriter {
    marker: std::path::PathBuf,
    child_done_at_first_write: Option<bool>,
    out: Vec<u8>,
}

#[cfg(unix)]
impl std::io::Write for AfterExitWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.child_done_at_first_write
            .get_or_insert_with(|| self.marker.exists());
        self.out.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Deterministic reproduction of the message the operator saw flash: a
// SessionStart-budget acquire fails with "handle allocation lock busy"
// while another writer holds the lock. The launcher's registration waits.
// trace:BUG-b6b1278890774ef993d2ebc327862470-030.Reproduce | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-030.ac181de7 | ai:claude
#[test]
fn launcher_registration_waits_out_lock_held_past_sessionstart_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let holder = hold_namespace(tmp.path(), Duration::from_millis(800));
    // The SessionStart budget gives up while the lock is held: the error the
    // operator saw flash before the crash.
    let busy = crate::agent_handle::lock_namespace(tmp.path())
        .err()
        .unwrap();
    assert!(format!("{busy:#}").contains("handle allocation lock busy"));
    // The launcher's registration waits for the holder instead.
    let entry = register(tmp.path()).expect("registration waits for the lock");
    holder.join().unwrap();
    assert_eq!(entry.name.as_deref(), Some("claude-advisor-07"));
    assert!(agent_registry::registry_path(tmp.path(), &entry.id).is_file());
}

// Regression for the early return: with the lock held past the budget, the
// launcher still waits for a real child, and the warning prints only after
// the child exits.
// trace:BUG-b6b1278890774ef993d2ebc327862470-030.ac1d0e79 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-030.ac181de7 | ai:claude
#[test]
#[cfg(unix)]
fn launcher_waits_for_child_when_registration_lock_stays_busy() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("child-exited");
    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("sleep 0.3; : > \"$1\"")
        .arg("sh")
        .arg(&marker)
        .spawn()
        .unwrap();
    let child_pid = child.id();
    let holder = hold_namespace(tmp.path(), Duration::from_secs(2));
    let mut writer = AfterExitWriter {
        marker: marker.clone(),
        child_done_at_first_write: None,
        out: Vec::new(),
    };
    let status = supervise_spawned_child(
        "claude",
        child_pid,
        Vec::new(),
        || {
            crate::agent_handle::lock_namespace_within(tmp.path(), Duration::from_millis(20))
                .map(|_| "never-registered".to_string())
        },
        || child.wait(),
        false,
        &mut writer,
    )
    .expect("a failed registration must not end the launch");
    holder.join().unwrap();
    assert!(status.success());
    assert!(
        marker.exists(),
        "the launcher returned before the child exited"
    );
    assert_eq!(writer.child_done_at_first_write, Some(true));
    let printed = String::from_utf8(writer.out).unwrap();
    assert!(
        printed.contains("could not record this agent in the registry"),
        "{printed}"
    );
    assert!(printed.contains("handle allocation lock busy"), "{printed}");
}

// A failed wait still prints the deferred warnings before the error returns.
// trace:BUG-b6b1278890774ef993d2ebc327862470-030.ac3e8354 | ai:claude
#[test]
fn deferred_warnings_print_when_wait_fails() {
    let mut out = Vec::new();
    let err = supervise_spawned_child(
        "claude",
        1,
        vec!["could not forward Ctrl+C/SIGTERM to the agent: denied".into()],
        || Err(anyhow::anyhow!("handle allocation lock busy")),
        || Err(std::io::Error::other("wait interrupted")),
        false,
        &mut out,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("failed to wait for claude"));
    let printed = String::from_utf8(out).unwrap();
    assert!(
        printed.contains("could not forward Ctrl+C/SIGTERM"),
        "{printed}"
    );
    assert!(printed.contains("handle allocation lock busy"), "{printed}");
}

// A launcher error before spawn returns to the caller (which prints it and
// exits) instead of handing the terminal to a child that paints over it.
// trace:BUG-b6b1278890774ef993d2ebc327862470-030.ac3e8354 | ai:claude
#[test]
fn pre_spawn_error_returns_without_starting_a_child() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(project.join(".aida/config.toml"), "").unwrap();
    let binary = tmp.path().join("missing-vendor");
    let config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: vec![],
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    let plan = AgentLaunchPlan {
        project_root: project.clone(),
        launch_cwd: project.clone(),
        role: None,
        role_instance: RoleInstanceKind::Driver,
        current_spec: None,
        name: "claude-advisor-07".into(),
        lease_id: None,
        native_session_id: None,
        resumed_from: None,
    };
    let mut spawned = false;
    let err = run_tracked_agent_after_spawn(
        &binary,
        &config,
        &plan,
        None,
        &[],
        None,
        false,
        false,
        None,
        false,
        || spawned = true,
    )
    .unwrap_err();
    assert!(!spawned, "no child may start after a launcher error");
    let message = format!("{err:#}");
    assert!(message.contains("failed to spawn"), "{message}");
    assert!(message.contains("missing-vendor"), "{message}");
}

// The post-exit mark uses the launcher budget too, so a dead pid is not left
// listed as running.
// trace:BUG-b6b1278890774ef993d2ebc327862470-030.ac1d0e79 | ai:claude
#[test]
fn mark_ended_waits_out_lock_held_past_sessionstart_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let entry = register(tmp.path()).unwrap();
    let holder = hold_namespace(tmp.path(), Duration::from_millis(800));
    let ended = agent_registry::mark_agent_ended(tmp.path(), "claude", std::process::id())
        .expect("mark ended waits for the lock")
        .expect("the registered row exists");
    holder.join().unwrap();
    assert_eq!(ended.id, entry.id);
    assert!(ended.ended_at.is_some());
}

#[test]
fn post_spawn_failure_is_deferred_not_returned() {
    let tmp = tempfile::tempdir().unwrap();
    let holder = hold_namespace(tmp.path(), Duration::from_millis(400));
    let mut warnings = Vec::new();
    let lock = post_spawn_step(
        crate::agent_handle::lock_namespace_within(tmp.path(), Duration::from_millis(20)),
        "could not record this agent in the registry",
        &mut warnings,
    );
    holder.join().unwrap();
    assert!(lock.is_none(), "a busy lock yields no value");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].starts_with("could not record this agent in the registry: "));
    assert!(warnings[0].contains("handle allocation lock busy"));

    let value = post_spawn_step(Ok(7), "unused", &mut warnings);
    assert_eq!(value, Some(7));
    assert_eq!(warnings.len(), 1, "a successful step adds no warning");
}

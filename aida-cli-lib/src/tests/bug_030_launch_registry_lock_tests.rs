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

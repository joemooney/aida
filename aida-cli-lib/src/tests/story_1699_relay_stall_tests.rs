//! Tests for STORY-1699: the `relay-stall` doctor category over fixture
//! `relay.status` files.
//
// trace:STORY-1699 | ai:claude

use super::*;

fn write_run(root: &Path, run: &str, state: &str, handoff: Option<&str>) -> PathBuf {
    let dir = root.join(".aida").join("relay").join(run);
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    let handoff_line = handoff
        .map(|h| dir.join("sessions").join(h).display().to_string())
        .unwrap_or_default();
    let updated = Utc::now().format("%Y-%m-%dT%H:%M:%S%z").to_string();
    std::fs::write(
        dir.join("relay.status"),
        format!(
            "state={state}\npid=123\nrun={run}\nsession=11\nhandoff={handoff_line}\nexit=3\n\
             updated={updated}\nticket=\nreason=\nslot=0\n"
        ),
    )
    .unwrap();
    dir
}

#[test]
fn no_relay_dir_is_silent() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(scan(tmp.path()).is_empty());
}

#[test]
fn blocked_and_exited_runs_without_done_marker_are_reported() {
    let tmp = tempfile::tempdir().unwrap();
    write_run(tmp.path(), "run-blocked", "blocked", Some("h_blocked.md"));
    write_run(tmp.path(), "run-exited", "exited", None);

    let findings = scan(tmp.path());
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["relay-stall:run-blocked", "relay-stall:run-exited"]
    );
    assert!(findings.iter().all(|f| f.category == CATEGORY));
    assert!(findings[0]
        .summary
        .contains("is blocked (exit 3, session 11"));
    assert!(findings[0].action.contains("h_blocked.md.done"));
}

#[test]
fn handoff_done_marker_acknowledges_a_blocked_run() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_run(tmp.path(), "run-a", "blocked", Some("h.md"));
    std::fs::write(dir.join("sessions").join("h.md.done"), "").unwrap();
    assert!(scan(tmp.path()).is_empty());
}

#[test]
fn run_dir_live_sentinel_does_not_acknowledge_an_exited_run() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_run(tmp.path(), "run-b", "exited", None);
    std::fs::write(dir.join("handoff.md.done"), "").unwrap();
    assert_eq!(scan(tmp.path()).len(), 1);
}

#[test]
fn dirs_without_status_file_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida/relay/prompts")).unwrap();
    assert!(scan(tmp.path()).is_empty());
}

// trace:TASK-1792 | ai:codex
fn set_status(dir: &Path, key: &str, value: &str) {
    let path = dir.join("relay.status");
    let mut status = parse_status(&std::fs::read_to_string(&path).unwrap());
    status.insert(key.to_string(), value.to_string());
    std::fs::write(
        path,
        status
            .iter()
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect::<String>(),
    )
    .unwrap();
}

fn hold_lock(dir: &Path) -> std::fs::File {
    let lock = std::fs::File::create(dir.join("relay.lock")).unwrap();
    lock.lock_exclusive().unwrap();
    lock
}

#[test]
fn dead_running_and_queued_owners_are_reported_even_with_live_sentinel() {
    let tmp = tempfile::tempdir().unwrap();
    for state in ["running", "queued"] {
        let dir = write_run(tmp.path(), state, state, None);
        set_status(
            &dir,
            "handoff",
            &dir.join("handoff.md").display().to_string(),
        );
        std::fs::write(dir.join("handoff.md.done"), "").unwrap();
        // A recycled live PID cannot hide an unlocked stale run.
        set_status(&dir, "pid", &std::process::id().to_string());
        set_status(&dir, "updated", "2000-01-01T00:00:00Z");
        std::fs::File::create(dir.join("relay.lock")).unwrap();
    }
    let findings = scan(tmp.path());
    assert_eq!(findings.len(), 2);
    assert!(findings.iter().all(|f| f.summary.contains("owner dead")));
    assert!(findings.iter().all(|f| !f.action.contains("touch")));
}

#[test]
fn live_locked_owner_is_silent_but_recycled_identity_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_run(tmp.path(), "live", "running", None);
    set_status(&dir, "pid", &std::process::id().to_string());
    let _lock = hold_lock(&dir);
    assert!(scan(tmp.path()).is_empty());
    // Simulate a PID whose current process started after the status writer.
    set_status(&dir, "updated", "2000-01-01T00:00:00Z");
    assert_eq!(scan(tmp.path()).len(), 1);
}

#[test]
fn killed_child_with_stale_running_status_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_run(tmp.path(), "killed", "running", None);
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    set_status(&dir, "pid", &child.id().to_string());
    set_status(&dir, "updated", &Utc::now().to_rfc3339());
    let _lock = hold_lock(&dir);
    assert!(scan(tmp.path()).is_empty());
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(scan(tmp.path()).len(), 1);
}

#[test]
fn explicit_exits_age_out_at_twenty_four_hours_but_blocked_runs_do_not() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_run(tmp.path(), "exit", "exited", None);
    let updated = updated_at(&parse_status(
        &std::fs::read_to_string(dir.join("relay.status")).unwrap(),
    ))
    .unwrap();
    assert_eq!(scan_at(tmp.path(), updated + Duration::hours(24)).len(), 1);
    assert!(scan_at(
        tmp.path(),
        updated + Duration::hours(24) + Duration::seconds(1)
    )
    .is_empty());
    set_status(&dir, "state", "blocked");
    assert_eq!(scan_at(tmp.path(), updated + Duration::days(100)).len(), 1);
}

#[test]
fn relative_seat_acknowledgement_counts_but_relative_live_sentinel_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_run(tmp.path(), "relative", "blocked", None);
    std::fs::write(dir.join("handoff.md.done"), "").unwrap();
    set_status(&dir, "handoff", "./handoff.md");
    assert_eq!(scan(tmp.path()).len(), 1);
    std::fs::remove_file(dir.join("handoff.md.done")).unwrap();
    set_status(&dir, "handoff", "sessions/../handoff.md");
    assert!(!scan(tmp.path())[0].action.contains("touch"));
    set_status(&dir, "handoff", "sessions/seat.md");
    std::fs::write(dir.join("sessions/seat.md.done"), "").unwrap();
    assert!(scan(tmp.path()).is_empty());
}

#[test]
fn live_lockless_relay_is_not_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_run(tmp.path(), "no-flock", "running", None);
    set_status(&dir, "pid", &std::process::id().to_string());
    std::fs::File::create(dir.join("relay.lock")).unwrap();
    assert!(scan(tmp.path()).is_empty());
}

#[test]
fn relay_shell_timestamp_and_rfc3339_timestamp_parse_to_the_same_instant() {
    let mut status = BTreeMap::new();
    status.insert("updated".into(), "2026-10-15T16:13:32-0700".into());
    let shell = updated_at(&status).unwrap();
    status.insert("updated".into(), "2026-10-15T16:13:32-07:00".into());
    assert_eq!(updated_at(&status), Some(shell));
}

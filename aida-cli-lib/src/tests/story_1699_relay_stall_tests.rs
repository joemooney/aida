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
    std::fs::write(
        dir.join("relay.status"),
        format!(
            "state={state}\npid=123\nrun={run}\nsession=11\nhandoff={handoff_line}\nexit=3\n\
             updated=2026-10-15T16:13:32-0700\nticket=\nreason=\nslot=0\n"
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
    write_run(tmp.path(), "run-running", "running", Some("h.md"));

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
fn run_dir_handoff_md_done_acknowledges_an_exited_run() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_run(tmp.path(), "run-b", "exited", None);
    std::fs::write(dir.join("handoff.md.done"), "").unwrap();
    assert!(scan(tmp.path()).is_empty());
}

#[test]
fn dirs_without_status_file_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida/relay/prompts")).unwrap();
    assert!(scan(tmp.path()).is_empty());
}

//! Tests for STORY-1700 (F1 Option A): `fleet_lanes::classify_lanes` over
//! fixture `relay.status` files, the classification table of R7.
//
// trace:STORY-1700 | ai:claude

use super::*;

fn write_run(root: &Path, run: &str, body: &str) -> PathBuf {
    let dir = root.join(".aida").join("relay").join(run);
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    std::fs::write(dir.join("relay.status"), body).unwrap();
    dir
}

fn status(state: &str, exit: &str, handoff: &str) -> String {
    format!(
        "state={state}\npid=999999999\nrun=r\nsession=2\nhandoff={handoff}\nexit={exit}\n\
         updated=2026-10-15T16:13:32-0700\n"
    )
}

const IMPL_HANDOFF: &str = "/repo/.aida/relay/r/sessions/h.md";

fn class_of(state: &str, exit: &str) -> LaneClass {
    let tmp = tempfile::tempdir().unwrap();
    write_run(tmp.path(), "r", &status(state, exit, IMPL_HANDOFF));
    let lanes = classify_lanes(tmp.path(), Utc::now());
    assert_eq!(lanes.len(), 1);
    lanes[0].class
}

// T7.1
// trace:STORY-1700.acb260c7 | ai:claude
#[test]
fn classify_table_covers_every_exit() {
    let table: &[(&str, &str, LaneClass)] = &[
        ("exited", "130", LaneClass::SignalStopped),
        ("exited", "143", LaneClass::SignalStopped),
        ("exited", "0", LaneClass::SessionCap),
        ("complete", "0", LaneClass::Complete),
        ("exited", "1", LaneClass::SetupError),
        ("exited", "3", LaneClass::Blocked),
        ("blocked", "3", LaneClass::Blocked),
        ("exited", "4", LaneClass::UsageBin),
        ("exited", "5", LaneClass::UsageBin),
        ("exited", "6", LaneClass::RunLocked),
        ("exited", "7", LaneClass::TerminalApi),
        ("blocked", "7", LaneClass::TerminalApi),
        ("exited", "8", LaneClass::NoSlot),
        ("exited", "42", LaneClass::Crashed),
        ("running", "0", LaneClass::Crashed),
        ("queued", "0", LaneClass::Crashed),
    ];
    for (state, exit, want) in table {
        assert_eq!(class_of(state, exit), *want, "state={state} exit={exit}");
    }
}

// T7.2
// trace:STORY-1700.acb260c7 | ai:claude
#[test]
fn running_state_with_free_lock_is_crashed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_run(tmp.path(), "r", &status("running", "0", IMPL_HANDOFF));
    // An unlocked lock file and a dead pid: the EXIT trap never ran.
    std::fs::File::create(dir.join("relay.lock")).unwrap();
    let lane = &classify_lanes(tmp.path(), Utc::now())[0];
    assert_eq!(lane.evidence.lock_held, Some(false));
    assert_eq!(lane.evidence.owner_alive, Some(false));
    assert_eq!(lane.class, LaneClass::Crashed);
}

// T7.3
// trace:STORY-1700.acb260c7 | ai:claude
#[test]
fn held_lock_is_running_whatever_status_says() {
    let tmp = tempfile::tempdir().unwrap();
    let mut locks = Vec::new();
    for (run, state, exit) in [
        ("a", "exited", "0"),
        ("b", "blocked", "3"),
        ("c", "complete", "0"),
    ] {
        let dir = write_run(tmp.path(), run, &status(state, exit, IMPL_HANDOFF));
        let lock = std::fs::File::create(dir.join("relay.lock")).unwrap();
        lock.lock_exclusive().unwrap();
        locks.push(lock);
    }
    let lanes = classify_lanes(tmp.path(), Utc::now());
    assert_eq!(lanes.len(), 3);
    assert!(
        lanes.iter().all(|l| l.class == LaneClass::Running),
        "{lanes:?}"
    );
}

#[test]
fn handoff_done_marker_is_acknowledged() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".aida/relay/r");
    let handoff = dir.join("sessions/h.md");
    write_run(
        tmp.path(),
        "r",
        &status("blocked", "3", &handoff.display().to_string()),
    );
    std::fs::write(dir.join("sessions/h.md.done"), "").unwrap();
    assert_eq!(
        classify_lanes(tmp.path(), Utc::now())[0].class,
        LaneClass::Acknowledged
    );
}

#[test]
fn advisor_seat_comes_from_the_handoff_path_and_fails_closed() {
    let mut s = BTreeMap::new();
    s.insert(
        "handoff".to_string(),
        "/repo/.aida/handoff/advisor-2026.md".to_string(),
    );
    assert_eq!(seat_kind_of(&s), SeatKind::Advisor);
    s.insert("handoff".to_string(), IMPL_HANDOFF.to_string());
    assert_eq!(seat_kind_of(&s), SeatKind::Implementer);
    s.insert("handoff".to_string(), String::new());
    assert_eq!(seat_kind_of(&s), SeatKind::Advisor);
    s.remove("handoff");
    assert_eq!(seat_kind_of(&s), SeatKind::Advisor);
}

#[test]
fn run_id_falls_back_to_the_directory_name() {
    let tmp = tempfile::tempdir().unwrap();
    write_run(tmp.path(), "dir-name", "state=exited\nexit=0\n");
    let lane = &classify_lanes(tmp.path(), Utc::now())[0];
    assert_eq!(lane.run_id, "dir-name");
}

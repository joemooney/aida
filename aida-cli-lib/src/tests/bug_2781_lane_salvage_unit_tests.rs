//! BUG-2781: which failed exits salvage, where to, and how the end is named.
// trace:BUG-2781 | ai:claude

use super::*;
use crate::headless_background::OriginHead;

fn seat(head: &str, origin: OriginHead, uncommitted: usize, unpushed: Option<bool>) -> SeatState {
    SeatState {
        head: Some(head.into()),
        origin,
        uncommitted: Some(uncommitted),
        head_has_unpushed: unpushed,
    }
}

#[test]
fn bug_2781_only_a_failed_exit_with_committed_unpushed_work_salvages() {
    let committed = seat("b", OriginHead::Absent, 0, Some(true));
    assert!(wanted(&Outcome::NoPr, false, &committed, Some("a")));
    // Committed work beside uncommitted edits: the commits salvage, the edits stay.
    let dirty = seat("b", OriginHead::At("a".into()), 2, Some(true));
    assert!(wanted(&Outcome::NoPr, false, &dirty, Some("a")));
    // A clean exit publishes the normal way.
    assert!(!wanted(&Outcome::NoPr, true, &committed, Some("a")));
    // Uncommitted only: HEAD never moved.
    let edits = seat("a", OriginHead::At("a".into()), 3, Some(false));
    assert!(!wanted(&Outcome::NoPr, false, &edits, Some("a")));
    // Already pushed, unreadable reachability, a punt or hold: no.
    let pushed = seat("b", OriginHead::At("b".into()), 0, Some(false));
    assert!(!wanted(&Outcome::NoPr, false, &pushed, Some("a")));
    let unknown = seat("b", OriginHead::Absent, 0, None);
    assert!(!wanted(&Outcome::NoPr, false, &unknown, Some("a")));
    let unknown_origin = seat("b", OriginHead::Unknown, 0, Some(true));
    assert!(!wanted(&Outcome::NoPr, false, &unknown_origin, Some("a")));
    let punted = Outcome::Punted { reason: "x".into() };
    assert!(!wanted(&punted, false, &committed, Some("a")));
}

#[test]
fn bug_2781_salvage_targets_the_pr_branch_only_when_a_pr_is_open() {
    assert_eq!(target("feat", Some(7), "lease-1"), "feat");
    assert_eq!(target("feat", None, "lease-1"), "salvage/lease-1");
}

#[cfg(unix)]
#[test]
fn bug_2781_exit_cause_names_the_signal_and_its_source() {
    use std::os::unix::process::ExitStatusExt as _;
    let term = exit_cause(std::process::ExitStatus::from_raw(15));
    assert!(term.contains("SIGTERM (15)"), "{term}");
    assert!(term.contains("not recorded by the launcher"), "{term}");
    let kill = exit_cause(std::process::ExitStatus::from_raw(9));
    assert!(
        kill.contains("SIGKILL (9)") && kill.contains("OOM"),
        "{kill}"
    );
    // A wrapper's exit 143 is 128 + SIGTERM.
    let wrapped = exit_cause(std::process::ExitStatus::from_raw(143 << 8));
    assert!(wrapped.contains("SIGTERM (15)"), "{wrapped}");
    let code = exit_cause(std::process::ExitStatus::from_raw(1 << 8));
    assert_eq!(code, "exited with code 1");
    let text = announcement("abc", "salvage/l", &term);
    assert!(text.starts_with(UNVERIFIED), "{text}");
    assert!(
        text.contains("SIGTERM") && text.contains("independent review"),
        "{text}"
    );
}

#[test]
fn bug_2781_gate_end_line_records_wall_time_and_overlay_state() {
    let line = crate::lane_job::end_line("j1", "stopped at lane end", 137, 1288, Some(true));
    assert_eq!(
        line,
        "j1 stopped at lane end rc=137 after 1288s; target overlay-stale"
    );
    let fresh = crate::lane_job::end_line("j1", "exited", 0, 90, Some(false));
    assert!(
        fresh.ends_with("after 90s; target overlay fresh"),
        "{fresh}"
    );
    assert_eq!(
        crate::lane_job::end_line("j1", "exited", 0, 5, None),
        "j1 exited rc=0 after 5s"
    );
}

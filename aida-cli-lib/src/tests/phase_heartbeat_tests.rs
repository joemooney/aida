use super::*;
use std::time::Duration;

/// STORY-726: the heartbeat line carries the actor, the spec, the elapsed
/// wall-clock (m + s), and the seconds-since-last-activity the watchdog
/// already tracks — the reassurance signal an unattended drive needs.
// trace:STORY-726 | ai:claude
#[test]
fn formats_elapsed_minutes_seconds_and_since_progress() {
    let line = phase_heartbeat_line(
        "implementer",
        "STORY-726",
        Duration::from_secs(4 * 60 + 18),
        Duration::from_secs(12),
    );
    assert_eq!(
        line,
        "implementer working on STORY-726 (4m 18s elapsed, last activity 12s ago)"
    );
}

/// Sub-minute elapsed reads `0m Ns`, and a zero since-progress (a just-reset
/// timer) reads `0s ago` rather than panicking or rolling over.
// trace:STORY-726 | ai:claude
#[test]
fn handles_sub_minute_and_zero_since_progress() {
    let line = phase_heartbeat_line(
        "implementer",
        "BUG-1",
        Duration::from_secs(42),
        Duration::from_secs(0),
    );
    assert_eq!(
        line,
        "implementer working on BUG-1 (0m 42s elapsed, last activity 0s ago)"
    );
}

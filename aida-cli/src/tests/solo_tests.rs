use super::*;
use crate::cli::SoloAction;

// ── resolve_solo_effect: verb → action, with flags as silent aliases ──

#[test]
fn verb_run_maps_to_run() {
    assert_eq!(
        resolve_solo_effect(Some(SoloAction::Run), false, false, false),
        SoloEffect::Run
    );
}

#[test]
fn verb_stop_maps_to_stop() {
    assert_eq!(
        resolve_solo_effect(Some(SoloAction::Stop), false, false, false),
        SoloEffect::Stop
    );
}

#[test]
fn verb_status_maps_to_status() {
    assert_eq!(
        resolve_solo_effect(Some(SoloAction::Status), false, false, false),
        SoloEffect::Status
    );
}

#[test]
fn watch_flag_aliases_to_run() {
    assert_eq!(
        resolve_solo_effect(None, false, false, true),
        SoloEffect::Run
    );
}

#[test]
fn off_flag_aliases_to_stop() {
    assert_eq!(
        resolve_solo_effect(None, true, false, false),
        SoloEffect::Stop
    );
}

#[test]
fn status_flag_aliases_to_status() {
    assert_eq!(
        resolve_solo_effect(None, false, true, false),
        SoloEffect::Status
    );
}

#[test]
fn no_verb_no_flag_enters_mode() {
    assert_eq!(
        resolve_solo_effect(None, false, false, false),
        SoloEffect::EnterMode
    );
}

#[test]
fn verb_wins_over_a_conflicting_flag() {
    // `aida solo stop --watch` → the verb is canonical, so Stop wins.
    assert_eq!(
        resolve_solo_effect(Some(SoloAction::Stop), false, false, true),
        SoloEffect::Stop
    );
}

// ── solo_sleep_until_stop: responsive-poll exits when the flag clears ──

#[test]
fn sleep_runs_full_interval_when_flag_stays_set() {
    let mut slept = Vec::new();
    let cleared = solo_sleep_until_stop(
        10,
        2,
        &mut |s| slept.push(s),
        &mut || true, // flag never clears
    );
    assert!(!cleared, "should report not-cleared after a full interval");
    // 10s in 2s chunks → five 2s sleeps.
    assert_eq!(slept, vec![2, 2, 2, 2, 2]);
}

#[test]
fn sleep_breaks_early_when_flag_clears() {
    let mut calls = 0;
    let mut slept = Vec::new();
    let cleared = solo_sleep_until_stop(300, 2, &mut |s| slept.push(s), &mut || {
        calls += 1;
        calls < 2 // clears on the second poll
    });
    assert!(cleared, "should report cleared mid-sleep");
    // Only polled twice → only two 2s chunks slept, NOT the full 300s.
    assert_eq!(slept, vec![2, 2]);
}

#[test]
fn sleep_chunks_the_tail_remainder() {
    // 5s in 2s chunks → 2, 2, 1.
    let mut slept = Vec::new();
    let cleared = solo_sleep_until_stop(5, 2, &mut |s| slept.push(s), &mut || true);
    assert!(!cleared);
    assert_eq!(slept, vec![2, 2, 1]);
}

#[test]
fn sleep_clamps_zero_poll_to_one() {
    // poll_secs 0 must not divide-by-zero / spin — clamps to 1.
    let mut slept = Vec::new();
    let _ = solo_sleep_until_stop(3, 0, &mut |s| slept.push(s), &mut || true);
    assert_eq!(slept, vec![1, 1, 1]);
}

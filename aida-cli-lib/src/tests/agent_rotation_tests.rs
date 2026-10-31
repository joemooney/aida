// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
use super::*;
use std::sync::atomic::AtomicBool;

fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000 + secs, 0).unwrap()
}

fn record(ts: i64, seat: &str, handle: &str, sid: Option<&str>, final_: bool) -> HandoffRecord {
    HandoffRecord {
        ts: at(ts),
        seat: seat.to_string(),
        event: "handoff".to_string(),
        path: format!(".aida/handoff/{seat}/{ts}.md"),
        handle: Some(handle.to_string()),
        native_session_id: sid.map(str::to_string),
        final_,
    }
}

fn run(sid: Option<&'static str>) -> GenerationRun<'static> {
    GenerationRun {
        seat: "advisor",
        handle: "claude-advisor-08",
        native_session_id: sid,
        spawned_at: at(0),
    }
}

fn decide(sid: Option<&'static str>, exit: i64, records: &[HandoffRecord]) -> RotationDecision {
    rotation_decision(
        &run(sid),
        at(exit),
        records,
        &[],
        &RotationPolicy::default(),
    )
}

#[test]
fn a_handoff_from_this_session_relaunches() {
    let r = [record(
        500,
        "advisor",
        "claude-advisor-08",
        Some("s1"),
        false,
    )];
    assert_eq!(
        decide(Some("s1"), 600, &r),
        RotationDecision::Relaunch {
            handoff: PathBuf::from(".aida/handoff/advisor/500.md")
        }
    );
}

#[test]
fn a_stale_or_foreign_handoff_never_relaunches() {
    // Another session of the same handle (a stale handoff), another handle,
    // and a sibling seat: none is this generation's.
    let r = [
        record(500, "advisor", "claude-advisor-08", Some("old"), false),
        record(510, "advisor", "claude-advisor-06", Some("s1"), false),
        record(520, "reviewer", "claude-advisor-08", Some("s1"), false),
    ];
    assert_eq!(decide(Some("s1"), 600, &r), RotationDecision::Exit);
}

#[test]
fn without_a_seeded_session_id_freshness_falls_back_to_the_spawn_time() {
    let stale = [record(-5, "advisor", "claude-advisor-08", None, false)];
    assert_eq!(decide(None, 600, &stale), RotationDecision::Exit);
    let fresh = [record(5, "advisor", "claude-advisor-08", None, false)];
    assert!(matches!(
        decide(None, 600, &fresh),
        RotationDecision::Relaunch { .. }
    ));
}

#[test]
fn a_final_handoff_ends_the_run() {
    let r = [record(
        500,
        "advisor",
        "claude-advisor-08",
        Some("s1"),
        true,
    )];
    assert_eq!(decide(Some("s1"), 600, &r), RotationDecision::Final);
}

#[test]
fn a_fast_exit_without_a_handoff_is_a_crash_but_with_one_is_a_rotation() {
    assert_eq!(
        decide(Some("s1"), 12, &[]),
        RotationDecision::Crash { after_secs: 12 }
    );
    let r = [record(5, "advisor", "claude-advisor-08", Some("s1"), false)];
    assert!(matches!(
        decide(Some("s1"), 12, &r),
        RotationDecision::Relaunch { .. }
    ));
}

#[test]
fn the_window_caps_relaunches() {
    let r = [record(
        500,
        "advisor",
        "claude-advisor-08",
        Some("s1"),
        false,
    )];
    let policy = RotationPolicy::default();
    let three = [at(100), at(200), at(300)];
    assert_eq!(
        rotation_decision(&run(Some("s1")), at(600), &r, &three, &policy),
        RotationDecision::Limit {
            relaunches: 3,
            window_mins: 10
        }
    );
    // Relaunches older than the window no longer count.
    let old = [at(-1000), at(-900), at(300)];
    assert!(matches!(
        rotation_decision(&run(Some("s1")), at(600), &r, &old, &policy),
        RotationDecision::Relaunch { .. }
    ));
}

#[test]
fn stop_messages_say_why() {
    let crash = stop_message("h", 2, &RotationDecision::Crash { after_secs: 12 });
    assert!(crash.contains("h #2 exited after 12s without a handoff"));
    let limit = stop_message(
        "h",
        4,
        &RotationDecision::Limit {
            relaunches: 3,
            window_mins: 10,
        },
    );
    assert!(limit.contains("3 relaunches in 10 min (limit)"));
}

#[test]
fn countdown_runs_to_zero_or_stops_on_ctrl_c() {
    let stop = AtomicBool::new(false);
    let mut ticks = 0;
    let mut out = Vec::new();
    assert!(countdown("h", 2, 5, &stop, &mut || ticks += 1, &mut out));
    assert_eq!(ticks, 5);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("relaunching h #2 in 5s, Ctrl-C to stop"));

    let stop = AtomicBool::new(false);
    let mut n = 0;
    let mut out = Vec::new();
    let proceed = countdown(
        "h",
        2,
        5,
        &stop,
        &mut || {
            n += 1;
            if n == 2 {
                stop.store(true, Ordering::SeqCst);
            }
        },
        &mut out,
    );
    assert!(!proceed);
    assert_eq!(n, 2);
}

#[test]
fn a_stamped_handoff_round_trips_through_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let writer = crate::seat_rotation::HandoffWriter {
        handle: Some("claude-advisor-08".to_string()),
        native_session_id: Some("s1".to_string()),
        generation: Some(2),
        final_: false,
    };
    let w =
        crate::seat_rotation::write_handoff_by(dir.path(), "advisor", "next: x", at(5), &writer)
            .unwrap();
    let latest = std::fs::read_to_string(&w.latest).unwrap();
    assert!(latest.contains("handle=claude-advisor-08 generation=2"));
    let records = read_handoff_log(dir.path(), "advisor");
    assert_eq!(records.len(), 1);
    assert!(matches!(
        rotation_decision(
            &run(Some("s1")),
            at(600),
            &records,
            &[],
            &RotationPolicy::default()
        ),
        RotationDecision::Relaunch { .. }
    ));
    let final_writer = crate::seat_rotation::HandoffWriter {
        final_: true,
        ..writer
    };
    crate::seat_rotation::write_handoff_by(dir.path(), "advisor", "done", at(6), &final_writer)
        .unwrap();
    let records = read_handoff_log(dir.path(), "advisor");
    assert_eq!(
        rotation_decision(
            &run(Some("s1")),
            at(600),
            &records,
            &[],
            &RotationPolicy::default()
        ),
        RotationDecision::Final
    );
}

#[test]
fn reseeding_replaces_the_session_id() {
    let mut args = vec![
        "--model".to_string(),
        "opus".to_string(),
        "--session-id".to_string(),
        "old".to_string(),
    ];
    crate::agent_launch::reseed_session_args("claude", &mut args, Some("new"));
    assert_eq!(args, ["--model", "opus", "--session-id", "new"]);
}

#[test]
fn the_supervised_ceiling_notice_promises_a_relaunch_under_the_handle() {
    let line =
        crate::seat_rotation::rotation_notice_line(400_000, Some(300_000), "advisor").unwrap();
    let supervised =
        crate::seat_rotation::supervised_rotation_notice(&line, "advisor", "claude-advisor-08");
    assert!(supervised.starts_with("Seat rotation: context at 400k tokens (ceiling 300k)."));
    assert!(supervised.contains("relaunches you as claude-advisor-08"));
    assert!(!supervised.contains("start a fresh session"));
}

// The launch's own reservation becomes a relay run: every generation binds
// the same recipient, the label is held in the gap between generations, and
// ending the run lets it go.
#[test]
fn the_rotation_run_keeps_one_recipient_across_generations() {
    use crate::agent_handle::{load_reservation, reserve_handle, HandleRequest};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let handle = "claude-advisor-08";
    reserve_handle(
        &root,
        &HandleRequest {
            agent_type: "claude",
            role: Some("advisor"),
            explicit: Some(handle),
            launcher: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut run = RotationRun::bind(&root, handle).unwrap();
    let first = load_reservation(&root, handle).unwrap();
    assert_eq!(first.recipient_id, run.run_id);
    run.bind_child(Some("gen-1")).unwrap();
    let gen1 = load_reservation(&root, handle).unwrap();
    assert_eq!(gen1.native_session_id.as_deref(), Some("gen-1"));
    // The gap: no child is bound, and another launch cannot take the label.
    assert_eq!(
        crate::relay_mail::run_holding(&root, &gen1).as_deref(),
        Some(run.run_id.as_str())
    );
    let other = reserve_handle(
        &root,
        &HandleRequest {
            agent_type: "claude",
            explicit: Some(handle),
            native_session_id: Some("intruder"),
            ..Default::default()
        },
    );
    assert!(other.is_err(), "the label must be held between generations");
    run.bind_child(Some("gen-2")).unwrap();
    let gen2 = load_reservation(&root, handle).unwrap();
    assert_eq!(gen2.recipient_id, gen1.recipient_id);
    assert_eq!(gen2.native_session_id.as_deref(), Some("gen-2"));
    run.end(crate::relay_mail::TerminalReason::Exit);
    assert!(crate::relay_mail::run_holding(&root, &gen2).is_none());
}

#[test]
fn ls_shows_the_generation_and_the_last_rotation() {
    use crate::relay_mail::ChildBinding;
    let child = |generation, secs| ChildBinding {
        generation,
        native_session_id: None,
        bound_at: at(secs),
    };
    // Run generation 2 is the first child: no rotation yet.
    let first = rotation_status(&[child(2, 0)]).unwrap();
    assert_eq!(first.generation, 1);
    assert_eq!(
        ls_handle_label("claude-advisor-08", Some(&first)),
        "claude-advisor-08"
    );
    assert_eq!(ls_rotation_note(Some(&first), at(100)), "");
    let third = rotation_status(&[child(2, 0), child(3, 100), child(4, 200)]).unwrap();
    assert_eq!(third.generation, 3);
    assert_eq!(
        ls_handle_label("claude-advisor-08", Some(&third)),
        "claude-advisor-08 #3"
    );
    assert!(ls_rotation_note(Some(&third), at(320)).starts_with("  rotated 2m"));
    assert_eq!(rotation_status(&[]), None);
}

#[test]
fn a_rotated_hello_names_the_generation_and_the_previous_session() {
    use crate::advisor_handshake::{handle_label, hello_message, HelloContext, Rotation};
    let rotation = Rotation {
        generation: 3,
        previous_native_session_id: Some("s2".into()),
    };
    assert_eq!(
        handle_label("claude-implementer-12", Some(&rotation)),
        "claude-implementer-12 #3 (rotated)"
    );
    let ctx = HelloContext {
        handle: "claude-implementer-12".into(),
        from_source: aida_core::mailbox::SenderSource::Explicit,
        role: "implementer".into(),
        spec: None,
        path: PathBuf::from("/w"),
        rotation: Some(rotation),
        session_started: None,
    };
    let msg = hello_message("id-1", &ctx, "advisor", Path::new("/m"), at(0));
    assert!(msg
        .body
        .contains("generation: 3 (rotated; previous session s2)"));
    assert_eq!(msg.from, "claude-implementer-12");
    assert_eq!(msg.thread_id, "aida-hello-id-1");
}

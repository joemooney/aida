//! STORY-HOME-100: the advisor handshake's pure rules — who says hello, what
//! a hello is, and when its sender reports the advisor unreachable. The
//! seat-clone round trip lives with the seat-clone fixtures
//! (`story_b6b_100_seat_clone_handshake_tests.rs`).
// trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude

use super::*;

fn t(mins_ago: i64) -> DateTime<Utc> {
    now() - Duration::minutes(mins_ago)
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-30T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn record(home: HomeKind, sent_mins_ago: i64) -> HelloRecord {
    HelloRecord {
        handle: "claude-implementer-229".into(),
        role: "implementer".into(),
        spec: Some("STORY-HOME-100".into()),
        path: "/work/aida-seat-x".into(),
        hello_id: "0190f000-0000-7000-8000-000000000001".into(),
        sent_at: t(sent_mins_ago),
        mailbox: "/work/aida/.aida/mailbox".into(),
        home,
        advisor_live: true,
        acked_at: None,
        acked_by: None,
        ack_reported: false,
    }
}

fn message(thread: &str, to: &str, in_reply_to: Option<&str>) -> Message {
    Message {
        id: "m1".into(),
        thread_id: thread.into(),
        from: "claude-implementer-229".into(),
        to: Recipient::Agent(to.into()),
        timestamp: 1,
        in_reply_to: in_reply_to.map(str::to_string),
        body: String::new(),
        subject: None,
        urgent: false,
        intent: Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    }
}

#[test]
fn only_launched_non_advisor_agents_say_hello() {
    assert!(eligible(Some("claude-implementer"), Some("implementer")));
    assert!(eligible(Some("codex-reviewer"), Some("Reviewer")));
    assert!(!eligible(Some("claude-advisor"), Some("advisor")));
    assert!(
        !eligible(None, Some("implementer")),
        "not launched by agent new"
    );
    assert!(!eligible(Some(" "), Some("implementer")));
    assert!(!eligible(Some("claude-implementer"), None), "no role");
}

#[test]
fn a_hello_is_typed_by_its_thread_and_addressed_to_the_advisor() {
    let hello_thread = format!("{THREAD_PREFIX}m1");
    assert!(is_hello(&message(&hello_thread, "advisor", None)));
    assert!(!is_hello(&message("m1", "advisor", None)), "plain mail");
    assert!(
        !is_hello(&message(
            &hello_thread,
            "claude-implementer-229",
            Some("m0")
        )),
        "the ack is a reply, not a hello"
    );
    assert!(!is_hello(&message(&hello_thread, "reviewer", None)));
}

#[test]
fn the_ack_id_is_stable_per_hello_and_a_valid_uuid() {
    assert_eq!(ack_id("a"), ack_id("a"));
    assert_ne!(ack_id("a"), ack_id("b"));
    assert!(uuid::Uuid::parse_str(&ack_id("a")).is_ok());
}

#[test]
fn a_hello_inside_the_window_is_waiting() {
    let policy = Policy::default();
    assert_eq!(
        state(&record(HomeKind::SeatPrimary, 9), true, &policy, now()),
        State::Waiting
    );
}

#[test]
fn an_acked_hello_reports_who_acked_it() {
    let mut rec = record(HomeKind::SeatPrimary, 30);
    rec.acked_at = Some(t(25));
    rec.acked_by = Some("claude-advisor-1".into());
    assert_eq!(
        state(&rec, false, &Policy::default(), now()),
        State::Acked {
            by: Some("claude-advisor-1".into())
        }
    );
}

#[test]
fn past_the_window_the_reason_names_the_mail_path() {
    let policy = Policy::default();
    let State::Unreachable(no_advisor) =
        state(&record(HomeKind::SeatPrimary, 11), false, &policy, now())
    else {
        panic!("expected unreachable");
    };
    assert!(no_advisor.starts_with("no live advisor"), "{no_advisor}");
    assert!(
        no_advisor.contains("/work/aida/.aida/mailbox"),
        "{no_advisor}"
    );

    let State::Unreachable(silent) = state(&record(HomeKind::Checkout, 11), true, &policy, now())
    else {
        panic!("expected unreachable");
    };
    assert!(silent.contains("has not acked within 10m"), "{silent}");

    let mut stranded = record(HomeKind::UnverifiedClone, 11);
    stranded.mailbox = "/work/aida-seat-x/.aida/mailbox".into();
    let State::Unreachable(clone) = state(&stranded, true, &policy, now()) else {
        panic!("expected unreachable");
    };
    assert!(clone.contains("seat clone's local mailbox"), "{clone}");
    assert!(clone.contains("/work/aida-seat-x/.aida/mailbox"), "{clone}");
    assert!(clone.contains("primary inbox does not read"), "{clone}");
}

#[test]
fn the_window_and_switch_come_from_the_mailbox_config() {
    let cfg: toml::Value =
        toml::from_str("[mailbox]\nhandshake_timeout_mins = 3\nadvisor_handshake = false\n")
            .unwrap();
    let policy = Policy::from_config(Some(&cfg));
    assert_eq!(policy.window, Duration::minutes(3));
    assert!(!policy.enabled);
    assert_eq!(Policy::from_config(None), Policy::default());
    assert_eq!(Policy::default().window, Duration::minutes(10));
}

#[test]
fn the_advisor_notice_lists_unacked_recent_hellos_oldest_first() {
    let mut newer = record(HomeKind::SeatPrimary, 2);
    newer.handle = "codex-reviewer-7".into();
    newer.role = "reviewer".into();
    newer.spec = None;
    let older = record(HomeKind::SeatPrimary, 5);
    let mut acked = record(HomeKind::SeatPrimary, 4);
    acked.handle = "claude-triage-3".into();
    acked.acked_at = Some(t(3));
    let mut stale = record(HomeKind::SeatPrimary, 60 * 25);
    stale.handle = "claude-old-1".into();

    let lines = advisor_lines(&[newer, older, acked, stale], now());
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains("hello from claude-implementer-229"),
        "{lines:?}"
    );
    assert!(lines[0].contains("implementer · STORY-HOME-100 · /work/aida-seat-x"));
    assert!(lines[0].contains("`aida mailbox inbox` acks it"));
    assert!(lines[1].contains("codex-reviewer-7 (reviewer · no spec"));
}

#[test]
fn the_agent_announces_an_ack_once_and_an_overdue_hello_every_turn() {
    let mut rec = record(HomeKind::SeatPrimary, 30);
    rec.acked_at = Some(t(1));
    rec.acked_by = Some("claude-advisor-1".into());
    let acked = State::Acked {
        by: rec.acked_by.clone(),
    };
    let line = agent_line(&rec, &acked).unwrap();
    assert!(line.contains("ack from claude-advisor-1"), "{line}");
    rec.ack_reported = true;
    assert_eq!(agent_line(&rec, &acked), None);
    assert_eq!(agent_line(&rec, &State::Waiting), None);
    let overdue = agent_line(&rec, &State::Unreachable("no live advisor".into())).unwrap();
    assert!(
        overdue.starts_with("advisor unreachable: no live advisor"),
        "{overdue}"
    );
}

#[test]
fn the_doctor_finding_names_the_failed_mail_path() {
    let mut rec = record(HomeKind::UnverifiedClone, 12);
    rec.mailbox = "/work/aida-seat-x/.aida/mailbox".into();
    let reason = unreachable_reason(&rec, true, &Policy::default());
    let f = finding(&rec, &reason, now());
    assert_eq!(f.category, DOCTOR_CATEGORY);
    assert_eq!(f.id, "advisor-handshake:claude-implementer-229");
    assert!(
        f.summary.contains("advisor unreachable for 12m"),
        "{}",
        f.summary
    );
    assert!(
        f.action
            .contains("/work/aida-seat-x/.aida/mailbox is a seat-clone local layer the primary inbox does not read"),
        "{}",
        f.action
    );
}

#[test]
fn a_record_round_trips_and_ls_flags_only_an_overdue_hello() {
    let dir = tempfile::tempdir().unwrap();
    let roots = vec![dir.path().to_path_buf()];
    let policy = Policy::default();
    assert_eq!(
        ls_note(&roots, Some("claude-implementer-229"), true, &policy, now()),
        ""
    );

    save_record(dir.path(), &record(HomeKind::SeatPrimary, 5)).unwrap();
    assert_eq!(
        load_record(dir.path(), "claude-implementer-229"),
        Some(record(HomeKind::SeatPrimary, 5))
    );
    assert_eq!(
        ls_note(&roots, Some("claude-implementer-229"), true, &policy, now()),
        ""
    );

    save_record(dir.path(), &record(HomeKind::SeatPrimary, 15)).unwrap();
    let note = ls_note(
        &roots,
        Some("claude-implementer-229"),
        false,
        &policy,
        now(),
    );
    assert!(
        note.contains("⚠ advisor unreachable: no live advisor"),
        "{note}"
    );
    assert_eq!(ls_note(&roots, None, false, &policy, now()), "");
}

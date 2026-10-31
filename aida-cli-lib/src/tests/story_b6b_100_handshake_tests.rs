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

// trace:STORY-B6B1278890774EF993D2EBC327862470-100.acf0ac12 | ai:claude
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

// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ac05f0ba | ai:claude
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ace90225 | ai:claude
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

// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ac5a43a5 | ai:claude
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

// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ac05f0ba | ai:claude
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

const HANDLE: &str = "claude-implementer-229";

/// A census over `root` with no live agents (so no live advisor).
fn census(root: &Path) -> Census {
    Census::new(
        root,
        root.join(".aida-store"),
        Vec::new(),
        Policy::default(),
        now(),
    )
}

// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ac05f0ba | ai:claude
#[test]
fn a_record_round_trips_and_ls_flags_only_an_overdue_hello() {
    let dir = tempfile::tempdir().unwrap();
    let census = census(dir.path());
    let started = t(60);
    assert_eq!(ls_note(&census, Some(HANDLE), started), "");

    save_record(dir.path(), &record(HomeKind::SeatPrimary, 5)).unwrap();
    assert_eq!(
        load_record(dir.path(), HANDLE),
        Some(record(HomeKind::SeatPrimary, 5))
    );
    assert_eq!(ls_note(&census, Some(HANDLE), started), "");

    save_record(dir.path(), &record(HomeKind::SeatPrimary, 15)).unwrap();
    let note = ls_note(&census, Some(HANDLE), started);
    assert!(
        note.contains("⚠ advisor unreachable: no live advisor"),
        "{note}"
    );
    assert_eq!(ls_note(&census, None, started), "");
    // A session that started after the hello reused the handle; the old
    // record says nothing about it.
    assert_eq!(ls_note(&census, Some(HANDLE), t(14)), "");
}

/// `agent ls` and doctor share one check: an ack already in a mailbox
/// clears the agent before its own hook marks the record acked.
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ac05f0ba | ai:claude
#[test]
fn an_ack_in_the_mailbox_clears_ls_before_the_record_is_marked() {
    let dir = tempfile::tempdir().unwrap();
    let rec = record(HomeKind::SeatPrimary, 15);
    save_record(dir.path(), &rec).unwrap();
    let census = census(dir.path());
    assert!(ls_note(&census, Some(HANDLE), t(60)).contains("advisor unreachable"));

    let mut hello = message(&format!("{THREAD_PREFIX}{}", rec.hello_id), "advisor", None);
    hello.id = rec.hello_id.clone();
    let ack = ack_message(
        &hello,
        HANDLE,
        "claude-advisor-1",
        SenderSource::Explicit,
        now(),
    );
    mailbox_store::write_message(dir.path(), &ack).unwrap();
    assert_eq!(ls_note(&census, Some(HANDLE), t(60)), "");
    assert!(
        load_record(dir.path(), HANDLE).unwrap().acked_at.is_none(),
        "the record is not marked yet"
    );
}

#[test]
fn a_record_belongs_to_the_session_that_started_before_it() {
    let rec = record(HomeKind::SeatPrimary, 5);
    assert!(sent_this_session(&rec, None), "unknown session");
    assert!(sent_this_session(&rec, Some(t(6))));
    assert!(!sent_this_session(&rec, Some(t(4))), "a later session");
}

/// The relay script itself: Claude/Codex and both Antigravity events run
/// `aida mailbox hello --hook` and pass its lines on (AC1's wiring; the
/// advisor's pending-hello lines ride the same call, AC2).
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.acf0ac12 | ai:claude
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ac5a43a5 | ai:claude
#[cfg(unix)]
#[test]
fn the_mail_notice_hook_relays_the_hello_for_every_client() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let log = dir.path().join("calls.log");
    let fake = bin.join("aida");
    crate::test_exec::write_executable(
        &fake,
        "#!/bin/sh\n\
         echo \"$*\" >> \"$AIDA_FAKE_LOG\"\n\
         case \"$*\" in\n\
           'awaiting --notice') echo 'Awaiting you: 1 mail' ;;\n\
           'mailbox notice') echo 'Mail: 1 unread' ;;\n\
           'mailbox hello --hook') echo 'Advisor handshake: hello sent to \"claude-advisor-1\"' ;;\n\
         esac\n",
    );
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aida-core/templates/hooks/aida-mail-notice.sh");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = |args: &[&str]| {
        std::fs::write(&log, "").unwrap();
        let out = std::process::Command::new("sh")
            .arg(&script)
            .args(args)
            .env("PATH", &path)
            .env("AIDA_FAKE_LOG", &log)
            .env("AIDA_SESSION_PROJECT", dir.path())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("mailbox hello --hook"), "{args:?}: {calls}");
        String::from_utf8(out.stdout).unwrap()
    };

    let claude = run(&[]);
    assert!(claude.contains("Awaiting you: 1 mail"), "{claude}");
    assert!(
        claude.contains("Advisor handshake: hello sent to \"claude-advisor-1\""),
        "{claude}"
    );

    for (event, notice) in [
        ("SessionStart", "Awaiting you: 1 mail"),
        ("PreInvocation", "Mail: 1 unread"),
    ] {
        let out = run(&["--client", "agy", "--event", event]);
        let json: serde_json::Value =
            serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{event}: {e}: {out}"));
        let text = json["injectSteps"][0]["systemMessage"]["systemMessage"]
            .as_str()
            .unwrap_or_else(|| panic!("{event}: {out}"));
        assert_eq!(
            text,
            format!("{notice}\nAdvisor handshake: hello sent to \"claude-advisor-1\""),
            "{event}"
        );
    }
}

/// `aida agent ls` and both doctor routes (`doctor check advisor-handshake`
/// and the scan's category path) over a live agent's overdue hello. Runs
/// in its own process: it changes the working directory and captures
/// stdout.
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ac05f0ba | ai:claude
#[test]
fn agent_ls_and_doctor_report_an_overdue_hello_and_its_mailbox() {
    if let Some(out) = crate::test_env::run_path_isolated_test_output() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for section in ["ls", "check", "scan"] {
            let body = stdout
                .split_once(&format!("=== {section}\n"))
                .unwrap_or_else(|| panic!("no {section} section: {stdout}"))
                .1
                .split("=== ")
                .next()
                .unwrap();
            assert!(
                body.contains("advisor unreachable") && body.contains("no live advisor"),
                "{section}: {body}"
            );
            assert!(body.contains(".aida/mailbox"), "{section}: {body}");
        }
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let init = crate::test_git::git_command()
        .current_dir(&root)
        .args(["init", "-q", "-b", "main"])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    std::fs::create_dir(root.join(".aida")).unwrap();
    std::fs::create_dir(root.join(".aida-store")).unwrap();
    let started = Utc::now() - Duration::minutes(30);
    let pid = std::process::id();
    crate::agent_registry::write_entry(
        &root,
        &AgentRegistryEntry {
            id: format!("claude-{pid}"),
            agent_type: "claude".into(),
            pid,
            pid_start: None,
            pid_namespace: crate::agent_registry::current_pid_namespace(),
            name: Some(HANDLE.into()),
            description: None,
            tty: None,
            terminal: None,
            started_at: started,
            last_active_at: Utc::now(),
            role: Some("implementer".into()),
            current_spec: None,
            worktree_path: root.clone(),
            source: "agent-launcher".into(),
            binary_version: None,
            build_sha: None,
            availability: Default::default(),
            paused_since: None,
            paused_reason: None,
            expected_back: None,
            native_session_id: None,
            claude_session_id: None,
            ended_at: None,
            resumed_from: None,
            spec_status_at_end: None,
            mail_wake: None,
        },
    )
    .unwrap();
    let mut rec = record(HomeKind::Checkout, 0);
    rec.sent_at = Utc::now() - Duration::minutes(15);
    rec.mailbox = mailbox_store::mailbox_dir(&root).display().to_string();
    save_record(&root, &rec).unwrap();
    std::env::set_current_dir(&root).unwrap();

    // `--all`, so the row does not depend on the busy/stale classifier.
    println!("=== ls");
    crate::agent_ls(true, false, false).unwrap();
    println!("=== check");
    let check = crate::cli::DoctorCommand::Check {
        category: DOCTOR_CATEGORY.into(),
        all: false,
        json: false,
        fail_on_findings: false,
    };
    crate::doctor_cmd::handle_doctor_command(
        false,
        false,
        None,
        false,
        false,
        false,
        None,
        Some(&check),
    )
    .unwrap();
    println!("=== scan");
    crate::doctor_cmd::handle_doctor_command(
        false,
        false,
        Some(DOCTOR_CATEGORY),
        true,
        false,
        false,
        None,
        None,
    )
    .unwrap();
}

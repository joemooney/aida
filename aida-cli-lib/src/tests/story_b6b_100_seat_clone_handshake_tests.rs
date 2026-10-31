//! STORY-HOME-100: the advisor handshake round trip from a contained seat's
//! private clone. The agent in the clone says hello; the advisor reading
//! from the primary sees it and acks it; the agent sees the ack. Before
//! BUG-HOME-031 the hello stayed in the clone and this failed.
// trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude

use super::s4_host_git_tests::leased_clone;
use super::tests::fixture;
use super::*;
use crate::advisor_handshake::{
    ack_hellos, hello_message, is_hello, load_record, save_record, send_hello, HelloContext,
    HelloRecord, HomeKind, SendOutcome,
};
use crate::mailbox_store::{mailbox_home, read_local_messages, MailboxHome};

const HANDLE: &str = "claude-implementer-229";

/// The primary's store (an `aida-store` worktree, so a mailbox read sees
/// the whole view), aliased into the clone as a seat's `.aida-store` is.
fn share_store(main: &Path, clone: &Path) -> PathBuf {
    let store = main.join(".aida-store");
    let add = crate::test_git::git_command()
        .current_dir(main)
        .args(["worktree", "add", "-q", "-b", "aida-store"])
        .arg(&store)
        .output()
        .unwrap();
    assert!(add.status.success(), "{add:?}");
    std::os::unix::fs::symlink(&store, clone.join(".aida-store")).unwrap();
    clone.join(".aida-store")
}

fn ctx(clone: &Path) -> HelloContext {
    HelloContext {
        handle: HANDLE.into(),
        from_source: aida_core::mailbox::SenderSource::Explicit,
        role: "implementer".into(),
        spec: Some("STORY-HOME-100".into()),
        path: clone.to_path_buf(),
        session_started: None,
        rotation: None,
    }
}

fn inbox_cmd() -> crate::cli::MailboxCommand {
    crate::cli::MailboxCommand::Inbox {
        agent: None,
        all: false,
        archived: false,
        peek: false,
        unread: false,
        recent_read_tail: 5,
    }
}

fn advisor_env() -> crate::test_env::AgentSessionIsolation {
    crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("operator")),
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_AGENT_TYPE", None),
    ])
}

// trace:STORY-B6B1278890774EF993D2EBC327862470-100.acf0ac12 | ai:claude
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ac5a43a5 | ai:claude
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ace90225 | ai:claude
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.Test | ai:claude
#[test]
fn a_seat_clone_hello_reaches_the_primary_advisor_and_the_ack_comes_back() {
    let _env = advisor_env();
    let fx = fixture();
    let clone = leased_clone(&fx, "story-100", "b100c10e0001");
    let clone_store = share_store(&fx.main, &clone);
    let home = mailbox_home(&clone);
    assert_eq!(home, MailboxHome::SeatPrimary(fx.main.clone()));
    let now = chrono::Utc::now();

    // AC1 + AC4: one hello naming handle, role, spec and path; with no live
    // advisor it is queued, not a silent success.
    let outcome = send_hello(&home, &clone_store, &ctx(&clone), now).unwrap();
    let SendOutcome::Queued(rec) = outcome else {
        panic!("no advisor is live in the fixture: {outcome:?}");
    };
    assert_eq!(rec.home, HomeKind::SeatPrimary);
    assert!(!rec.advisor_live);

    // AC5: the advisor, reading from the primary, sees it.
    let primary = read_local_messages(mailbox_home(&fx.main).root()).unwrap();
    let inbox = aida_core::mailbox::inbox_for("advisor", &primary);
    let hello = *inbox
        .iter()
        .find(|m| is_hello(m))
        .expect("the hello is in the primary advisor inbox");
    for field in [
        format!("handle: {HANDLE}"),
        "role: implementer".to_string(),
        "spec: STORY-HOME-100".to_string(),
        format!("path: {}", clone.display()),
    ] {
        assert!(hello.body.contains(&field), "{field} in {}", hello.body);
    }
    assert!(
        read_local_messages(&clone).unwrap().is_empty(),
        "nothing stranded in the clone"
    );

    // A resumed session does not say hello twice.
    assert!(matches!(
        send_hello(&home, &clone_store, &ctx(&clone), now).unwrap(),
        SendOutcome::AlreadySent(_)
    ));

    // AC2: the advisor's per-turn hook lists the hello...
    let main_store = fx.main.join(".aida-store");
    let notice =
        crate::advisor_handshake::run(&mailbox_home(&fx.main), &fx.main, &main_store, true)
            .unwrap();
    assert_eq!(notice.len(), 1, "{notice:?}");
    assert!(
        notice[0].contains(&format!(
            "hello from {HANDLE} (implementer · STORY-HOME-100"
        )),
        "{notice:?}"
    );
    // ...and reading it acks it, once.
    assert_eq!(ack_hellos(&fx.main, &main_store, &[hello], now), [HANDLE]);
    assert!(
        ack_hellos(&fx.main, &main_store, &[hello], now).is_empty(),
        "a re-read reports no new ack"
    );
    let notice =
        crate::advisor_handshake::run(&mailbox_home(&fx.main), &fx.main, &main_store, true)
            .unwrap();
    assert!(notice.is_empty(), "nothing left waiting: {notice:?}");
    let after = read_local_messages(home.root()).unwrap();
    let acks: Vec<_> = aida_core::mailbox::inbox_for(HANDLE, &after)
        .into_iter()
        .filter(|m| m.in_reply_to.as_deref() == Some(hello.id.as_str()))
        .collect();
    assert_eq!(acks.len(), 1, "one ack in the agent's inbox");
    assert_eq!(acks[0].thread_id, hello.thread_id);

    // The agent sees the ack.
    let rec = load_record(home.root(), HANDLE).unwrap();
    assert!(rec.acked_at.is_some(), "the record is marked acked");
    assert!(crate::advisor_handshake::find_ack(
        &rec,
        &[crate::mailbox_store::mailbox_dir(home.root())]
    )
    .is_some());
}

/// `aida mailbox inbox` itself acks a hello it shows: the ack lands in the
/// agent's inbox and the agent's record is marked.
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ac5a43a5 | ai:claude
#[test]
fn the_advisors_inbox_read_acks_the_hello_into_the_agents_inbox() {
    let _env = advisor_env();
    let fx = fixture();
    let clone = leased_clone(&fx, "story-100-inbox", "b100c10e0005");
    let main_store = share_store(&fx.main, &clone).canonicalize().unwrap();
    let home = mailbox_home(&clone);
    let now = chrono::Utc::now();
    let id = uuid::Uuid::new_v4().to_string();
    let mailbox = crate::mailbox_store::mailbox_dir(&fx.main);
    let hello = hello_message(&id, &ctx(&clone), "advisor", &mailbox, now);
    crate::mailbox_store::write_message(&fx.main, &hello).unwrap();
    save_record(
        home.root(),
        &HelloRecord {
            handle: HANDLE.into(),
            role: "implementer".into(),
            spec: None,
            path: clone.display().to_string(),
            hello_id: id.clone(),
            sent_at: now,
            mailbox: mailbox.display().to_string(),
            home: HomeKind::of(&home),
            advisor_live: true,
            acked_at: None,
            acked_by: None,
            ack_reported: false,
            rotation: None,
        },
    )
    .unwrap();

    crate::mailbox_cmd::handle_mailbox_command(&inbox_cmd(), &main_store).unwrap();
    let after = read_local_messages(home.root()).unwrap();
    let acks: Vec<_> = aida_core::mailbox::inbox_for(HANDLE, &after)
        .into_iter()
        .filter(|m| m.in_reply_to.as_deref() == Some(id.as_str()))
        .collect();
    assert_eq!(acks.len(), 1, "one ack in the agent's inbox: {after:?}");
    assert_eq!(acks[0].thread_id, hello.thread_id);
    let rec = load_record(home.root(), HANDLE).unwrap();
    assert_eq!(rec.acked_by.as_deref(), Some("operator"));
    assert!(
        ack_hellos(&fx.main, &main_store, &[&hello], now).is_empty(),
        "the inbox read already wrote the ack"
    );
}

/// A later `agent new` that reuses the handle is a new session: it says
/// hello again instead of inheriting the old record.
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.acf0ac12 | ai:claude
#[test]
fn a_reused_handle_says_hello_again_in_a_new_session() {
    let _env = advisor_env();
    let fx = fixture();
    let clone = leased_clone(&fx, "story-100-reuse", "b100c10e0004");
    let clone_store = share_store(&fx.main, &clone);
    let home = mailbox_home(&clone);
    let first_at = chrono::Utc::now() - chrono::Duration::hours(2);
    let first = send_hello(&home, &clone_store, &ctx(&clone), first_at)
        .unwrap()
        .record()
        .clone();
    let mut old = first.clone();
    old.acked_at = Some(first_at);
    save_record(home.root(), &old).unwrap();

    let mut same = ctx(&clone);
    same.session_started = Some(first_at - chrono::Duration::minutes(1));
    assert!(matches!(
        send_hello(&home, &clone_store, &same, chrono::Utc::now()).unwrap(),
        SendOutcome::AlreadySent(_)
    ));

    let mut next = ctx(&clone);
    next.session_started = Some(chrono::Utc::now() - chrono::Duration::minutes(1));
    let SendOutcome::Queued(rec) =
        send_hello(&home, &clone_store, &next, chrono::Utc::now()).unwrap()
    else {
        panic!("a new session sends a new hello");
    };
    assert_ne!(rec.hello_id, first.hello_id);
    assert_eq!(rec.acked_at, None, "the old ack is not inherited");
    assert_eq!(load_record(home.root(), HANDLE), Some(rec));
}

// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ac05f0ba | ai:claude
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ace90225 | ai:claude
#[test]
fn the_hook_reports_queued_then_unreachable_then_the_ack() {
    let fx = fixture();
    let clone = leased_clone(&fx, "story-100-hook", "b100c10e0002");
    let clone_store = share_store(&fx.main, &clone);
    let home = mailbox_home(&clone);
    // The env `aida agent new` gives a seat: its own handle as
    // `AIDA_AGENT_NAME` and `AIDA_USER`, no inherited `AIDA_HANDLE`.
    let run_hook = |event: Option<&str>| {
        let _env = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_USER", Some(HANDLE)),
            ("AIDA_AGENT_NAME", Some(HANDLE)),
            ("AIDA_HANDLE", None),
            ("AIDA_SESSION_ROLE", Some("implementer")),
            ("AIDA_SESSION_SCOPE", Some("STORY-HOME-100")),
            ("AIDA_AGENT_TYPE", Some("claude")),
            ("AIDA_HOOK_EVENT", event),
            ("AIDA_ADVISOR_HANDSHAKE", None),
        ]);
        crate::advisor_handshake::run(&home, &clone, &clone_store, true).unwrap()
    };

    // Before SessionStart there is nothing to say.
    assert!(run_hook(None).is_empty());
    let start = run_hook(Some("SessionStart"));
    assert!(
        start
            .iter()
            .any(|l| l.contains("no registered advisor; hello queued")),
        "{start:?}"
    );
    // Inside the window the hook stays quiet.
    assert!(run_hook(None).is_empty());

    // AC3: past the window the agent reports the advisor unreachable.
    let mut rec = load_record(home.root(), HANDLE).unwrap();
    rec.sent_at -= chrono::Duration::minutes(11);
    save_record(home.root(), &rec).unwrap();
    let overdue = run_hook(None);
    assert_eq!(overdue.len(), 1, "{overdue:?}");
    assert!(
        overdue[0].starts_with("advisor unreachable: no registered advisor"),
        "{overdue:?}"
    );
    let census = crate::advisor_handshake::Census::new(
        &fx.main,
        clone_store.clone(),
        Vec::new(),
        crate::advisor_handshake::Policy::default(),
        chrono::Utc::now(),
    );
    let note = crate::advisor_handshake::ls_note(
        &census,
        Some(HANDLE),
        rec.sent_at - chrono::Duration::minutes(1),
    );
    assert!(note.contains("advisor unreachable"), "{note}");

    // The advisor acks; the agent announces it once.
    {
        let _env = advisor_env();
        let primary = read_local_messages(&fx.main).unwrap();
        let hello = primary.iter().find(|m| is_hello(m)).unwrap();
        ack_hellos(
            &fx.main,
            &fx.main.join(".aida-store"),
            &[hello],
            chrono::Utc::now(),
        );
    }
    let acked = run_hook(None);
    assert_eq!(acked.len(), 1, "{acked:?}");
    assert!(acked[0].contains("ack from operator"), "{acked:?}");
    assert!(run_hook(None).is_empty(), "announced once");
}

/// A startup without its own handle (a headless queue child: seat label in
/// `AIDA_AGENT_NAME`, its own `AIDA_HANDLE`; or a child holding its parent's
/// names) sends no hello and prints nothing.
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.acf0ac12 | ai:claude
#[test]
fn a_startup_without_its_own_handle_sends_no_hello() {
    let fx = fixture();
    let clone = leased_clone(&fx, "story-100-nohello", "b100c10e0006");
    let clone_store = share_store(&fx.main, &clone);
    let home = mailbox_home(&clone);
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("claude-implementer")),
        ("AIDA_AGENT_NAME", Some("claude-implementer")),
        ("AIDA_HANDLE", Some(HANDLE)),
        ("AIDA_SESSION_ROLE", Some("implementer")),
        ("AIDA_AGENT_TYPE", Some("claude")),
        ("AIDA_HOOK_EVENT", Some("SessionStart")),
        ("AIDA_ADVISOR_HANDSHAKE", None),
    ]);
    let lines = crate::advisor_handshake::run(&home, &clone, &clone_store, true).unwrap();
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(load_record(home.root(), HANDLE), None);
    assert!(
        !read_local_messages(home.root())
            .unwrap()
            .iter()
            .any(is_hello),
        "no hello was written"
    );
}

#[test]
fn an_unverified_clone_hello_stays_local_and_names_that_path() {
    let fx = fixture();
    let clone = leased_clone(&fx, "story-100-unverified", "b100c10e0003");
    std::fs::remove_file(crate::lease_path(&fx.main, "b100c10e0003")).unwrap();
    let clone_store = share_store(&fx.main, &clone);
    let home = mailbox_home(&clone);
    assert_eq!(home, MailboxHome::UnverifiedClone(clone.clone()));

    let rec = send_hello(&home, &clone_store, &ctx(&clone), chrono::Utc::now())
        .unwrap()
        .record()
        .clone();
    assert_eq!(rec.home, HomeKind::UnverifiedClone);
    assert!(
        !read_local_messages(&fx.main).unwrap().iter().any(is_hello),
        "the primary inbox never sees it"
    );
    let reason = crate::advisor_handshake::unreachable_reason(
        &rec,
        &crate::advisor_handshake::AdvisorPresence::Registered,
        &crate::advisor_handshake::Policy::default(),
    );
    assert!(reason.contains("seat clone's local mailbox"), "{reason}");
    assert!(reason.contains(&clone.display().to_string()), "{reason}");
}

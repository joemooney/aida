//! STORY-HOME-100: the advisor handshake round trip from a contained seat's
//! private clone. The agent in the clone says hello; the advisor reading
//! from the primary sees it and acks it; the agent sees the ack. Before
//! BUG-HOME-031 the hello stayed in the clone and this failed.
// trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude

use super::s4_host_git_tests::leased_clone;
use super::tests::fixture;
use super::*;
use crate::advisor_handshake::{
    ack_hellos, is_hello, load_record, save_record, send_hello, HelloContext, HomeKind, SendOutcome,
};
use crate::mailbox_store::{mailbox_home, read_local_messages, MailboxHome};

const HANDLE: &str = "claude-implementer-229";

/// The primary's store, aliased into the clone as a seat's `.aida-store` is.
fn share_store(main: &Path, clone: &Path) -> PathBuf {
    let store = main.join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
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
    }
}

fn advisor_env() -> crate::test_env::AgentSessionIsolation {
    crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("operator")),
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_AGENT_TYPE", None),
    ])
}

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

    // AC2: reading the hello acks it, once.
    let main_store = fx.main.join(".aida-store");
    assert_eq!(ack_hellos(&fx.main, &main_store, &[hello], now), [HANDLE]);
    assert_eq!(ack_hellos(&fx.main, &main_store, &[hello], now), [HANDLE]);
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

#[test]
fn the_hook_reports_queued_then_unreachable_then_the_ack() {
    let fx = fixture();
    let clone = leased_clone(&fx, "story-100-hook", "b100c10e0002");
    let clone_store = share_store(&fx.main, &clone);
    let home = mailbox_home(&clone);
    let run_hook = |event: Option<&str>| {
        let _env = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_USER", Some("claude-implementer")),
            ("AIDA_AGENT_NAME", Some("claude-implementer")),
            ("AIDA_HANDLE", Some(HANDLE)),
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
            .any(|l| l.contains("no live advisor; hello queued")),
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
        overdue[0].starts_with("advisor unreachable: no live advisor"),
        "{overdue:?}"
    );
    let note = crate::advisor_handshake::ls_note(
        &[fx.main.clone()],
        Some(HANDLE),
        false,
        &crate::advisor_handshake::Policy::default(),
        chrono::Utc::now(),
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
        true,
        &crate::advisor_handshake::Policy::default(),
    );
    assert!(reason.contains("seat clone's local mailbox"), "{reason}");
    assert!(reason.contains(&clone.display().to_string()), "{reason}");
}

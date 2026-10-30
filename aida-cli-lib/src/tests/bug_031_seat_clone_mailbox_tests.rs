//! BUG-031: mail sent from a contained seat's private clone reaches the
//! primary checkout's inbox. A clone is its own repository, so the primary's
//! logical view never reads `<clone>/.aida/mailbox`; a verified clone writes
//! the primary's mailbox instead, and an unverified one says its mail is
//! local and names the sync command.
//! trace:BUG-B6B1278890774EF993D2EBC327862470-031 | ai:claude

use super::s4_host_git_tests::leased_clone;
use super::tests::fixture;
use super::*;
use crate::mailbox_store::{mailbox_home, read_local_messages, MailboxHome};

fn send_to_advisor(body: &str) -> crate::cli::MailboxCommand {
    crate::cli::MailboxCommand::Send {
        to: Some("advisor".into()),
        broadcast: false,
        body: Some(body.into()),
        subject: None,
        body_file: None,
        stdin: false,
        thread: None,
        in_reply_to: None,
        from: Some("claude-implementer-229".into()),
        urgent: false,
        intent: "fyi".into(),
        relayed_from: None,
        allow_second_person: false,
        queue_for_role: true,
    }
}

/// The primary's store, aliased into the clone as a seat's `.aida-store` is.
fn share_store(main: &Path, clone: &Path) -> PathBuf {
    let store = main.join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    std::os::unix::fs::symlink(&store, clone.join(".aida-store")).unwrap();
    clone.join(".aida-store")
}

fn bodies(root: &Path) -> Vec<String> {
    read_local_messages(root)
        .unwrap()
        .into_iter()
        .map(|m| m.body)
        .collect()
}

#[test]
fn a_seat_clone_send_to_advisor_lands_in_the_primary_inbox() {
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("operator")),
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_AGENT_TYPE", None),
    ]);
    let fx = fixture();
    let clone = leased_clone(&fx, "bug-031", "b031c10e0001");
    assert_eq!(
        mailbox_home(&clone),
        MailboxHome::SeatPrimary(fx.main.clone())
    );
    assert_eq!(
        mailbox_home(&fx.main),
        MailboxHome::Checkout(fx.main.clone())
    );
    let clone_store = share_store(&fx.main, &clone);

    crate::mailbox_cmd::handle_mailbox_command(&send_to_advisor("PR ready"), &clone_store).unwrap();

    assert_eq!(bodies(&fx.main), ["PR ready"], "written to the primary");
    assert!(bodies(&clone).is_empty(), "nothing stranded in the clone");
    // The advisor's inbox, read from the primary, lists it; so does the
    // seat's own read, which resolves to the same mailbox.
    for reader in [&fx.main, &clone] {
        let local = read_local_messages(mailbox_home(reader).root()).unwrap();
        let inbox = aida_core::mailbox::inbox_for("advisor", &local);
        assert!(
            inbox.iter().any(|m| m.body == "PR ready"),
            "advisor inbox read from {} lists the clone's message",
            reader.display()
        );
    }
    let note = mailbox_home(&clone).send_note().unwrap();
    assert!(note.contains("primary checkout's mailbox"), "{note}");
}

#[test]
fn an_unverified_clone_send_stays_local_and_says_so() {
    let fx = fixture();
    let clone = leased_clone(&fx, "bug-031-unverified", "b031c10e0002");
    std::fs::remove_file(crate::lease_path(&fx.main, "b031c10e0002")).unwrap();
    assert_eq!(
        mailbox_home(&clone),
        MailboxHome::UnverifiedClone(clone.clone())
    );

    let note = mailbox_home(&clone).send_note().unwrap();
    assert!(
        note.contains("only to this seat clone's local mailbox"),
        "{note}"
    );
    assert!(note.contains("aida mailbox sync"), "names the sync: {note}");
    assert_eq!(mailbox_home(&fx.main).send_note(), None);
}

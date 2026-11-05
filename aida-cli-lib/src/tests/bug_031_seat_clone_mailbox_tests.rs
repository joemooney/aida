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

/// ACK in a verified seat clone uses the primary's common-Git receipt
/// anchor. Moving to a linked worktree or another seat does not resurrect it.
// trace:BUG-2449 | ai:codex
#[test]
fn bug2449_receipt_survives_switching_between_seat_clones_and_worktrees() {
    use super::tests::git_ok;
    use crate::mailbox_view::{commit_ack, explicit_limits, own_audience, plan_ack, read_own_mail};
    use aida_core::mailbox_logical::ResolvedAudience;
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("operator")),
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_AGENT_TYPE", None),
    ]);
    let fx = fixture();
    let clone = leased_clone(&fx, "bug2449-first", "b2449c10e001");
    let other = super::s4_host_git_tests::leased_clone_on(
        &fx,
        "bug2449-second",
        "b2449c10e002",
        "other-seat",
    );
    let store = fx.main.join(".aida-store");
    git_ok(
        &fx.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "aida-store",
            store.to_str().unwrap(),
        ],
    );
    let sibling = fx.root.join("sibling");
    git_ok(
        &fx.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "sibling",
            sibling.to_str().unwrap(),
        ],
    );
    for root in [&clone, &other, &sibling] {
        std::os::unix::fs::symlink(&store, root.join(".aida-store")).unwrap();
    }
    let audience: ResolvedAudience = own_audience();
    let msg: aida_core::mailbox::Message = serde_json::from_value(serde_json::json!({
        "id": "seen-in-seat", "thread_id": "seen-in-seat", "from": "sender",
        "to": { "kind": "agent", "agent": "advisor" },
        "timestamp": 1, "body": "already read in the first seat"
    }))
    .unwrap();
    crate::mailbox_store::write_message(&fx.main, &msg).unwrap();
    let first = read_own_mail(
        &clone,
        &clone.join(".aida-store"),
        &audience,
        &explicit_limits(),
    );
    assert!(!first.is_partial(), "{:?}", first.partial_note());
    let rows = first.records().iter().collect::<Vec<_>>();
    assert_eq!(first.unread().len(), 1);
    let plan = plan_ack(&first, &rows).unwrap().unwrap();
    commit_ack(&clone, plan, &audience).unwrap();
    // Drop the clone-local mirror: only the shared durable receipt remains.
    std::fs::remove_dir_all(clone.join(".aida/mailbox/.read-receipts")).unwrap();
    for root in [&fx.main, &sibling, &other] {
        let after = read_own_mail(
            root,
            &root.join(".aida-store"),
            &audience,
            &explicit_limits(),
        );
        assert!(
            !after.is_partial(),
            "{}: {:?}",
            root.display(),
            after.partial_note()
        );
        assert_eq!(after.records().len(), 1);
        assert!(
            after.unread().is_empty(),
            "{} resurrected acknowledged mail",
            root.display()
        );
    }
}

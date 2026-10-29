//! STORY-1843 B2b adapter conformance on one real common-Git repository:
//! main, a sibling worktree and the canonical store worktree. Every
//! own-audience reader sees the same logical view, a partial view is never
//! an empty success, and the explicit read marks exactly what it displayed.
// trace:STORY-1843 | ai:claude
use super::*;
use aida_core::mailbox_logical::ResolvedAudience;
use std::path::PathBuf;
use std::process::Command;

pub(crate) fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(cwd)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
        ])
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t.t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t.t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// `main` (a Git checkout), a sibling worktree `sib`, and the store
/// worktree at `main/.aida-store` on the `aida-store` branch.
pub(crate) struct MailRepo {
    _dir: tempfile::TempDir,
    pub(crate) main: PathBuf,
    pub(crate) sib: PathBuf,
    pub(crate) store: PathBuf,
}

pub(crate) fn mail_repo() -> MailRepo {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let main = base.join("main");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("README.md"), "seed").unwrap();
    git(&main, &["add", "-A"]);
    git(&main, &["commit", "-qm", "seed"]);
    let sib = base.join("sib");
    git(
        &main,
        &["worktree", "add", "-q", "-b", "sib", sib.to_str().unwrap()],
    );
    let store = main.join(".aida-store");
    git(&main, &["branch", "aida-store"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            store.to_str().unwrap(),
            "aida-store",
        ],
    );
    MailRepo {
        _dir: dir,
        main,
        sib,
        store,
    }
}

pub(crate) fn mail(id: &str, to: &str, ts: i64, urgent: bool) -> Message {
    Message {
        id: id.to_string(),
        thread_id: id.to_string(),
        from: "sender".to_string(),
        to: aida_core::mailbox::Recipient::Agent(to.to_string()),
        timestamp: ts,
        in_reply_to: None,
        body: format!("body of {id}"),
        subject: None,
        urgent,
        intent: aida_core::mailbox::Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    }
}

pub(crate) fn put_canonical(store: &Path, m: &Message) {
    let dir = crate::mailbox_store::canonical_dir(store);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{}.json", m.id)),
        serde_json::to_vec(m).unwrap(),
    )
    .unwrap();
}

fn operator() -> ResolvedAudience {
    ResolvedAudience::from_facts(&unbound_facts("operator", Some("advisor"), None))
}

fn view_from(root: &Path, store: &Path, audience: &ResolvedAudience) -> OwnMail {
    read_own_mail(root, store, audience, &explicit_limits())
}

fn ids(messages: &[&Message]) -> Vec<String> {
    let mut out: Vec<String> = messages.iter().map(|m| m.id.clone()).collect();
    out.sort();
    out
}

/// The shared matrix: a sibling-only message and a canonical-only urgent
/// message are visible to every own-audience adapter from main; reading
/// them from main marks them so every adapter, from either checkout, sees
/// them read.
#[test]
fn story1843_adapters_share_one_logical_view_and_one_read_state() {
    let r = mail_repo();
    let audience = operator();
    crate::mailbox_store::write_message(&r.sib, &mail("sib-only", "operator", 10, false)).unwrap();
    put_canonical(&r.store, &mail("canon-urgent", "advisor", 20, true));
    crate::mailbox_store::write_message(&r.main, &mail("someone-else", "stranger", 30, true))
        .unwrap();

    let view = view_from(&r.main, &r.store, &audience);
    assert_eq!(view.coverage(), &Coverage::Complete);
    assert_eq!(ids(&view.unread()), vec!["canon-urgent", "sib-only"]);
    // Notice, statusline urgent count and the latency warning agree.
    let keys: Vec<String> = audience.keys().map(str::to_string).collect();
    let notice = crate::mailbox_cmd::notice_text(&view, &keys, 10);
    assert!(notice.contains("2 unread"), "{notice}");
    assert!(!notice.contains("partial"), "{notice}");
    assert_eq!(
        crate::read_urgent_unread_count_for(&r.main, &audience),
        Some(1)
    );
    assert!(crate::mailbox_latency_warning_for(&view, 1_000_000, 0)
        .is_some_and(|w| w.contains("oldest unread") && !w.contains("partial")));
    // The sibling sees the same view.
    let from_sib = view_from(&r.sib, &r.store, &audience);
    assert_eq!(ids(&from_sib.unread()), vec!["canon-urgent", "sib-only"]);

    // The explicit inbox shows both and marks exactly those, in main.
    let rows = crate::mailbox_cmd::select_inbox_rows(view.records(), false, true, 0);
    assert_eq!(rows.len(), 2);
    ack_displayed(&r.main, &view, &rows).unwrap();

    for root in [&r.main, &r.sib] {
        let after = view_from(root, &r.store, &audience);
        assert!(after.unread().is_empty(), "read from {root:?}");
        assert!(after
            .records()
            .iter()
            .all(|rec| ReadState::of(rec) == ReadState::Read));
        assert_eq!(crate::mailbox_cmd::notice_text(&after, &keys, 10), "");
    }
    assert_eq!(
        crate::read_urgent_unread_count_for(&r.main, &audience),
        Some(0)
    );
    // The receipts are per key the message was addressed to.
    assert!(crate::mailbox_store::read_receipts(&r.main, "operator").contains_key("sib-only"));
    assert!(crate::mailbox_store::read_receipts(&r.main, "advisor").contains_key("canon-urgent"));
    // Another recipient's mail never entered the view or got a receipt.
    assert!(crate::mailbox_store::read_receipts(&r.main, "stranger").is_empty());
}

/// A legacy watermark alone is not new mail (passive surfaces stay quiet)
/// and reads as `read (legacy)` in the explicit inbox. Displaying it
/// explicitly acknowledges it in the receipt anchor (B2c) with no read time
/// claimed: no local timed receipt is invented for it.
#[test]
fn story1843_legacy_watermark_is_not_new_and_is_labelled() {
    let r = mail_repo();
    let audience = operator();
    crate::mailbox_store::write_message(&r.main, &mail("old", "operator", 10, true)).unwrap();
    crate::mailbox_store::set_watermark(&r.main, "operator", 10).unwrap();
    crate::mailbox_store::write_message(&r.main, &mail("new", "operator", 20, false)).unwrap();

    let view = view_from(&r.main, &r.store, &audience);
    assert_eq!(ids(&view.unread()), vec!["new"]);
    assert_eq!(
        crate::read_urgent_unread_count_for(&r.main, &audience),
        Some(0),
        "a legacy-read urgent message does not light the statusline"
    );
    let unread_rows = crate::mailbox_cmd::select_inbox_rows(view.records(), false, true, 0);
    assert_eq!(unread_rows.len(), 1);
    let default_rows = crate::mailbox_cmd::select_inbox_rows(view.records(), false, false, 5);
    let states: Vec<_> = default_rows
        .iter()
        .map(|rec| (rec.message.id.as_str(), ReadState::of(rec).as_str()))
        .collect();
    assert_eq!(states, vec![("new", "unread"), ("old", "read (legacy)")]);

    ack_displayed(&r.main, &view, &default_rows).unwrap();
    let receipts = crate::mailbox_store::read_receipts(&r.main, "operator");
    assert!(receipts.contains_key("new"));
    assert!(
        !receipts.contains_key("old"),
        "a legacy read has no known read time; no receipt is invented"
    );
    let after = view_from(&r.main, &r.store, &audience);
    assert!(after
        .records()
        .iter()
        .all(|rec| ReadState::of(rec) == ReadState::Read));
    assert!(matches!(
        after
            .records()
            .iter()
            .find(|rec| rec.message.id == "old")
            .unwrap()
            .read,
        aida_core::mailbox_logical::ReadEvidence::Acknowledged {
            origin: aida_core::mailbox_logical::AckOrigin::Anchor { legacy: true },
            ..
        }
    ));
}

/// A worktree that vanished from disk makes every adapter say the view is
/// partial; none of them reports caught up or empty.
#[test]
fn story1843_partial_view_is_never_an_empty_success() {
    let r = mail_repo();
    let audience = operator();
    std::fs::remove_dir_all(&r.sib).unwrap();

    let view = view_from(&r.main, &r.store, &audience);
    assert!(view.is_partial());
    assert!(view.unread().is_empty());
    let keys: Vec<String> = audience.keys().map(str::to_string).collect();
    let notice = crate::mailbox_cmd::notice_text(&view, &keys, 10);
    assert!(notice.contains("mail view partial"), "{notice}");
    assert!(notice.contains("Worktree"), "{notice}");
    assert!(
        !notice.contains(r.sib.to_str().unwrap()),
        "no private path in the notice"
    );
    assert_eq!(
        crate::mailbox_latency_warning_for(&view, 1_000_000, 0).as_deref(),
        Some("⚠ mail: view partial — some mail may be missing")
    );
}

/// A non-Git project reads its own local mailbox, typed partial.
#[test]
fn story1843_local_only_project_reads_its_own_mail_as_partial() {
    let dir = tempfile::tempdir().unwrap();
    let audience = operator();
    crate::mailbox_store::write_message(dir.path(), &mail("local", "operator", 1, false)).unwrap();
    let view = view_from(dir.path(), &dir.path().join(".aida-store"), &audience);
    assert_eq!(ids(&view.unread()), vec!["local"]);
    assert!(view
        .partial_note()
        .is_some_and(|n| n.contains("NotAGitCheckout")));
}

/// `--agent` narrows the verified audience; it never widens it.
#[test]
fn story1843_selector_only_narrows() {
    let audience = operator();
    assert!(audience.narrowed_to("advisor").is_some());
    assert!(audience.narrowed_to("Operator").is_some());
    assert!(audience.narrowed_to("stranger").is_none());
}

/// The collector reads the env adapters it owns; with the suite runner's
/// own session hidden it projects no binding.
#[test]
fn story1843_collector_reads_env_and_projects_no_binding_when_isolated() {
    let _g = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("collector-user")),
        (crate::agent_handle::HANDLE_ENV, Some("child-handle")),
        ("AIDA_AGENT_NAME", Some("launch-name")),
        ("AIDA_SESSION_ROLE", Some("reviewer")),
        ("AIDA_AGENT_TYPE", Some("codex")),
    ]);
    let facts = crate::collect_mail_identity_facts();
    assert_eq!(facts.own_handle(), None);
    assert!(!facts.binding_unverified());
    assert_eq!(
        aida_core::mailbox_identity::inbox_identities(&facts),
        vec![
            "collector-user",
            "child-handle",
            "launch-name",
            "reviewer",
            "codex"
        ]
    );
}

/// Hold the env lock with no ambient session and an unbound `operator`
/// reader in the `advisor` role, as every own-audience command sees it.
fn operator_env() -> crate::test_env::AgentSessionIsolation {
    crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("operator")),
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_AGENT_TYPE", None),
    ])
}

fn run(store: &Path, cmd: crate::cli::MailboxCommand) -> Result<()> {
    crate::mailbox_cmd::handle_mailbox_command(&cmd, store)
}

/// F1 (PR #2952 review, round 1): every ID an own-audience listing displays
/// resolves in the follow-up verbs against the same logical snapshot. A
/// sibling-only message is listed by the inbox, then read, threaded and
/// replied to from main; a source-local write to it refuses and names the
/// worktree that holds it. The reader's own sent mail resolves too.
#[test]
fn story1843_every_listed_id_resolves_in_the_follow_up_verbs() {
    use crate::cli::MailboxCommand;
    let _env = operator_env();
    let r = mail_repo();
    crate::mailbox_store::write_message(&r.sib, &mail("sib-only", "operator", 10, false)).unwrap();
    let mut sent = mail("sent-from-sib", "someone", 15, false);
    sent.from = "operator".into();
    sent.thread_id = "sib-only".into();
    crate::mailbox_store::write_message(&r.sib, &sent).unwrap();
    crate::mailbox_store::write_message(&r.main, &mail("main-local", "operator", 20, false))
        .unwrap();

    // The inbox lists the sibling-only message...
    let view = view_from(&r.main, &r.store, &own_audience());
    assert!(view
        .records()
        .iter()
        .any(|rec| rec.message.id == "sib-only"));
    // ...and every follow-up verb resolves it, by ID and by prefix.
    run(
        &r.store,
        MailboxCommand::Read {
            message_id: "sib-only".into(),
        },
    )
    .unwrap();
    run(
        &r.store,
        MailboxCommand::Read {
            message_id: "sib-o".into(),
        },
    )
    .unwrap();
    run(
        &r.store,
        MailboxCommand::Thread {
            thread_id: "sib-only".into(),
        },
    )
    .unwrap();
    let book = crate::mailbox_view::addressable(&r.main, &r.store);
    assert_eq!(
        aida_core::mailbox::reply_target_thread("sib-only", book.messages()).as_deref(),
        Some("sib-only"),
        "a reply attaches to the listed message's thread"
    );
    let thread = aida_core::mailbox::thread("sib-only", book.messages());
    let ids: Vec<&str> = thread.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["sib-only", "sent-from-sib"],
        "own sent mail joins"
    );

    // Writes stay source-local: archiving a sibling-only message (read
    // first) and retracting one's own sibling-only send both refuse, naming
    // the worktree that holds it.
    let rows = crate::mailbox_cmd::select_inbox_rows(view.records(), false, true, 0);
    ack_displayed(&r.main, &view, &rows).unwrap();
    let archive = |id: &str| MailboxCommand::Archive {
        message_id: Some(id.into()),
        older_than: None,
        read_only: false,
        agent: None,
    };
    let err = run(&r.store, archive("sib-only")).unwrap_err().to_string();
    assert!(err.contains("another worktree"), "{err}");
    assert!(err.contains(r.sib.to_str().unwrap()), "{err}");
    let err = run(
        &r.store,
        MailboxCommand::Retract {
            message_id: "sent-from-sib".into(),
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("another worktree"), "{err}");
    // A message this checkout holds archives once read: the receipt the
    // inbox wrote is the read evidence (no legacy watermark is advanced).
    run(&r.store, archive("main-local")).unwrap();
    assert_eq!(
        crate::mailbox_store::read_watermark(&r.main, "operator"),
        None
    );
    // An unknown ID is still a typed not-found.
    let err = run(
        &r.store,
        MailboxCommand::Read {
            message_id: "nope".into(),
        },
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<crate::MailboxResolveFailure>(),
        Some(crate::MailboxResolveFailure::NotFound { .. })
    ));
}

/// F3 (ruling 76 Q3): an inbox read writes per-ID receipts only. A late,
/// old-timestamp arrival in the same local source after a read that
/// displayed newer sibling and canonical mail is still new to every passive
/// surface.
#[test]
fn story1843_a_late_old_arrival_after_a_read_is_still_new() {
    let r = mail_repo();
    let audience = operator();
    crate::mailbox_store::write_message(&r.sib, &mail("sib-new", "operator", 100, false)).unwrap();
    put_canonical(&r.store, &mail("canon-new", "advisor", 200, false));
    let view = view_from(&r.main, &r.store, &audience);
    let rows = crate::mailbox_cmd::select_inbox_rows(view.records(), false, true, 0);
    ack_displayed(&r.main, &view, &rows).unwrap();
    assert_eq!(
        crate::mailbox_store::read_watermark(&r.main, "operator"),
        None
    );
    assert_eq!(
        crate::mailbox_store::read_watermark(&r.main, "advisor"),
        None
    );

    crate::mailbox_store::write_message(&r.main, &mail("late-old", "operator", 50, true)).unwrap();
    let after = view_from(&r.main, &r.store, &audience);
    assert_eq!(ids(&after.unread()), vec!["late-old"]);
    let keys: Vec<String> = audience.keys().map(str::to_string).collect();
    let notice = crate::mailbox_cmd::notice_text(&after, &keys, 10);
    assert!(notice.contains("1 unread"), "{notice}");
    assert_eq!(
        crate::read_urgent_unread_count_for(&r.main, &audience),
        Some(1)
    );
}

/// F5: `aida mailbox latency --json` rows read the same view: unread and
/// oldest age before a read, the read latency from the receipt after, a
/// partial view flagged on every row, and another inbox an inspection.
#[test]
fn story1843_latency_rows_share_the_view() {
    let r = mail_repo();
    let audience = operator();
    crate::mailbox_store::write_message(&r.sib, &mail("sib-only", "operator", 1_000, false))
        .unwrap();
    let now = 5_000;
    let (rows, note) =
        crate::mailbox_cmd::latency_rows(&r.main, &r.store, &audience, None, now).unwrap();
    assert!(note.is_none());
    let op = rows
        .iter()
        .find(|row| row["recipient"] == "operator")
        .unwrap();
    assert_eq!(op["unread"], 1);
    assert_eq!(op["oldest_unread_age_ms"], 4_000);
    assert_eq!(op["partial"], false);
    assert_eq!(op["inspection"], false);

    let view = view_from(&r.main, &r.store, &audience);
    let shown = crate::mailbox_cmd::select_inbox_rows(view.records(), false, true, 0);
    ack_displayed(&r.main, &view, &shown).unwrap();
    let (rows, _) =
        crate::mailbox_cmd::latency_rows(&r.main, &r.store, &audience, Some("operator"), now)
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["unread"], 0);
    assert!(
        rows[0]["max_read_latency_ms"].as_i64().is_some(),
        "{rows:?}"
    );

    let (rows, _) =
        crate::mailbox_cmd::latency_rows(&r.main, &r.store, &audience, Some("stranger"), now)
            .unwrap();
    assert_eq!(rows[0]["inspection"], true);

    std::fs::remove_dir_all(&r.sib).unwrap();
    let (rows, note) =
        crate::mailbox_cmd::latency_rows(&r.main, &r.store, &audience, None, now).unwrap();
    assert!(note.is_some_and(|n| n.contains("partial")));
    assert!(rows.iter().all(|row| row["partial"] == true));
}

fn awaiting_mail(
    root: &Path,
    facts: impl FnOnce() -> crate::MailIdentityFacts,
) -> crate::awaiting_you::MailChannel {
    let backend_store = root.join("backend-store");
    std::fs::create_dir_all(&backend_store).unwrap();
    let backend = crate::test_env::home_fixture_cached_backend(
        &backend_store,
        &root.join(".aida").join("cache.db"),
    )
    .unwrap();
    let ctx = crate::UserStatusContext {
        session: None,
        role: None,
        branch: None,
        pr: None,
        queue_head: Vec::new(),
        queue_total: 0,
        agents: Vec::new(),
    };
    crate::collect_awaiting_report_with_mail_facts(root, &backend, &ctx, true, false, facts).mail
}

/// F5: the awaiting mail channel counts the same view and says when it is
/// partial instead of reading as caught up.
#[test]
fn story1843_awaiting_mail_channel_shares_the_view() {
    let r = mail_repo();
    crate::mailbox_store::write_message(&r.sib, &mail("sib-only", "operator", 10, false)).unwrap();
    put_canonical(&r.store, &mail("canon-urgent", "operator", 20, true));
    let facts = || unbound_facts("operator", None, None);
    let mail = awaiting_mail(&r.main, facts);
    assert_eq!(mail.unread, 2);
    assert_eq!(mail.urgent, 1);
    assert!(!mail.partial);

    std::fs::remove_dir_all(&r.sib).unwrap();
    let mail = awaiting_mail(&r.main, facts);
    assert_eq!(mail.unread, 1);
    assert!(mail.partial, "a missing worktree is never caught up");
}

/// F5: a bound child session that inherited its parent's `AIDA_USER` and
/// `AIDA_HANDLE` gets no notice, body, count or receipt of the parent's
/// private mail on any adapter; its own mail is read normally.
#[test]
fn story1843_bound_child_never_sees_inherited_parent_mail() {
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("parent-user")),
        (crate::agent_handle::HANDLE_ENV, Some("parent-handle")),
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_AGENT_TYPE", None),
    ]);
    let r = mail_repo();
    for (id, to) in [
        ("to-parent-user", "parent-user"),
        ("to-parent-handle", "parent-handle"),
        ("to-child", "child-seat"),
    ] {
        crate::mailbox_store::write_message(&r.sib, &mail(id, to, 10, true)).unwrap();
    }
    let facts = bound_facts("child-seat", "claude");
    let audience = ResolvedAudience::from_facts(&facts);
    assert_eq!(audience.private_keys(), ["child-seat"]);

    let view = view_from(&r.main, &r.store, &audience);
    let shown: Vec<&str> = view
        .records()
        .iter()
        .map(|rec| rec.message.id.as_str())
        .collect();
    assert_eq!(shown, vec!["to-child"], "no parent body is ever displayed");
    let keys: Vec<String> = audience.keys().map(str::to_string).collect();
    let notice = crate::mailbox_cmd::notice_text(&view, &keys, 10);
    assert!(notice.contains("1 unread"), "{notice}");
    assert!(!notice.contains("parent"), "{notice}");
    assert_eq!(
        crate::read_urgent_unread_count_for(&r.main, &audience),
        Some(1)
    );
    let (rows, _) =
        crate::mailbox_cmd::latency_rows(&r.main, &r.store, &audience, None, 1_000).unwrap();
    assert!(rows
        .iter()
        .all(|row| !row["recipient"].as_str().unwrap().starts_with("parent")));
    let mail = awaiting_mail(&r.main, || facts.clone());
    assert_eq!(mail.unread + mail.shared_unread, 1);

    let rows = crate::mailbox_cmd::select_inbox_rows(view.records(), false, false, 10);
    ack_displayed(&r.main, &view, &rows).unwrap();
    assert!(crate::mailbox_store::read_receipts(&r.main, "parent-user").is_empty());
    assert!(crate::mailbox_store::read_receipts(&r.main, "parent-handle").is_empty());
    assert!(crate::mailbox_store::read_receipts(&r.main, "child-seat").contains_key("to-child"));
}

/// R2-F1 (advisor ruling, round 3): an inbox read writes receipts only, so
/// every fleet-wide unread reader counts a receipt in any worktree as a
/// read. The operator reads from the sibling; from main the night-shift
/// probe, the maintenance snapshot, the launch section and the read
/// predicate behind `mailbox list`/inspection then all report 0 unread.
#[test]
fn story1843_inbox_read_clears_every_fleet_wide_unread_reader() {
    let r = mail_repo();
    let audience = operator();
    crate::mailbox_store::write_message(&r.main, &mail("main-mail", "operator", 10, true)).unwrap();
    put_canonical(&r.store, &mail("canon-mail", "operator", 20, false));
    let fields: std::collections::BTreeSet<_> =
        [crate::schedule_predicate::Field::MailUnread].into();
    let shift_unread = || {
        crate::mailbox_store::oldest_unread_by_recipient(&r.main, &r.store)
            .unwrap()
            .get("operator")
            .map_or(0, |u| u.count)
    };
    let maintenance_unread =
        || crate::maintenance_schedule::collect_snapshot(&r.main, &fields, None).mail_unread;
    let launch = || crate::agent_launch::render_launch_mailbox_section(&r.main, "operator");
    let inspected_unread = || {
        let evidence = crate::mailbox_store::ReadEvidence::load(&r.main).unwrap();
        ["main-mail", "canon-mail"]
            .iter()
            .filter(|id| !evidence.read_by("operator", &mail(id, "operator", 10, false)))
            .count()
    };
    assert_eq!(shift_unread(), 2);
    assert_eq!(maintenance_unread(), 2);
    assert!(
        !launch().contains("No unread mailbox messages"),
        "{}",
        launch()
    );
    assert_eq!(inspected_unread(), 2);

    let view = view_from(&r.sib, &r.store, &audience);
    let rows = crate::mailbox_cmd::select_inbox_rows(view.records(), false, true, 0);
    assert_eq!(rows.len(), 2);
    ack_displayed(&r.sib, &view, &rows).unwrap();
    assert_eq!(
        crate::mailbox_store::read_watermark(&r.sib, "operator"),
        None
    );

    assert_eq!(shift_unread(), 0);
    assert_eq!(maintenance_unread(), 0);
    assert!(
        launch().contains("No unread mailbox messages"),
        "{}",
        launch()
    );
    assert_eq!(inspected_unread(), 0);
    let local = crate::mailbox_store::read_local_messages(&r.main).unwrap();
    let canonical = crate::mailbox_store::read_canonical_messages(&r.store).unwrap();
    let merged = aida_core::mailbox::merge_dedup(&local, &canonical);
    let evidence = crate::mailbox_store::ReadEvidence::load(&r.main).unwrap();
    let summaries = aida_core::mailbox::agent_summaries_read_by(
        &merged,
        &evidence.watermarks,
        ["operator"],
        |agent, m| evidence.read_by(agent, m),
    );
    assert_eq!(summaries[0].unread, 0, "{summaries:?}");
    assert_eq!(summaries[0].total, 2, "{summaries:?}");
    let stranded = aida_core::mailbox::stranded_recipients_read_by(
        &merged,
        |_| false,
        |who, m| evidence.read_by(who, m),
    );
    assert!(stranded.is_empty(), "{stranded:?}");
}

/// R3-F2: an unreadable watermark directory fails closed in the unattended
/// counters: the mail still counts as unread (and the evidence says it is
/// partial), never as zero unread.
#[test]
fn story1843_unreadable_watermarks_fail_closed() {
    let r = mail_repo();
    crate::mailbox_store::write_message(&r.main, &mail("main-mail", "operator", 10, true)).unwrap();
    put_canonical(&r.store, &mail("canon-mail", "operator", 20, false));
    let marks = r.main.join(".aida/mailbox/.read");
    let _ = std::fs::remove_dir_all(&marks);
    std::fs::write(&marks, "not a directory").unwrap();

    assert!(crate::mailbox_store::ReadEvidence::load(&r.main).is_err());
    let evidence = crate::mailbox_store::ReadEvidence::load_fail_closed(&r.main);
    assert!(evidence.partial());
    let fields: std::collections::BTreeSet<_> =
        [crate::schedule_predicate::Field::MailUnread].into();
    assert_eq!(
        crate::maintenance_schedule::collect_snapshot(&r.main, &fields, None).mail_unread,
        2
    );
    let launch = crate::agent_launch::render_launch_mailbox_section(&r.main, "operator");
    assert!(!launch.contains("No unread mailbox messages"), "{launch}");
    assert!(
        launch.contains(crate::mailbox_store::PARTIAL_EVIDENCE_NOTE),
        "{launch}"
    );
}

/// R3-F1: a receipt that cannot be read counts its message unread and makes
/// the fleet-wide evidence a typed partial; a readable receipt elsewhere
/// still counts as a read.
#[test]
fn story1843_unanswerable_receipt_is_partial_not_absent() {
    let r = mail_repo();
    let read = mail("read-mail", "operator", 10, false);
    let unknown = mail("unknown-mail", "operator", 20, false);
    let dir = r.sib.join(".aida/mailbox/.read-receipts/operator");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("read-mail.txt"), "15").unwrap();
    std::fs::write(dir.join("unknown-mail.txt"), "garbage").unwrap();

    let evidence = crate::mailbox_store::ReadEvidence::load(&r.main).unwrap();
    assert!(evidence.read_by("operator", &read));
    assert!(!evidence.partial());
    assert!(!evidence.read_by("operator", &unknown));
    assert!(evidence.partial());
    assert_eq!(
        crate::mailbox_cmd::max_read_latency_ms(&evidence, "operator", &[read, unknown]),
        Some(5),
        "latency reads the same fleet-wide receipts as the unread count"
    );
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

/// B2c: `aida mailbox inbox` acknowledges, after its output is flushed,
/// exactly what it showed, in the common-Git receipt anchor; the ACK
/// survives the removal of the worktree that held the mail and of every
/// local receipt. A partial view shows the mail but acknowledges nothing.
#[test]
fn story1843_b2c_inbox_acknowledges_after_flush_into_the_anchor() {
    let audience = operator();
    let _env = operator_env();
    let r = mail_repo();
    crate::mailbox_store::write_message(&r.sib, &mail("sib-only", "operator", 10, false)).unwrap();
    put_canonical(&r.store, &mail("canon", "advisor", 20, false));

    run(&r.store, inbox_cmd()).unwrap();
    let after = view_from(&r.main, &r.store, &audience);
    assert!(after.unread().is_empty());
    assert!(after.records().iter().all(|rec| matches!(
        rec.read,
        aida_core::mailbox_logical::ReadEvidence::Acknowledged {
            origin: aida_core::mailbox_logical::AckOrigin::Anchor { legacy: false },
            ..
        }
    )));
    // The local compatibility copy is there too, for older readers.
    assert!(crate::mailbox_store::read_receipts(&r.main, "operator").contains_key("sib-only"));
    std::fs::remove_dir_all(r.main.join(".aida/mailbox/.read-receipts")).unwrap();
    let after = view_from(&r.main, &r.store, &audience);
    assert!(
        after.unread().is_empty(),
        "the anchor outlives local receipts"
    );

    // A new message with one sibling unreadable: shown, not acknowledged.
    crate::mailbox_store::write_message(&r.main, &mail("main-new", "operator", 30, false)).unwrap();
    let late = r.sib.join(".aida/mailbox/broken.json");
    std::fs::write(&late, "{not json").unwrap();
    run(&r.store, inbox_cmd()).unwrap();
    std::fs::remove_file(&late).unwrap();
    let after = view_from(&r.main, &r.store, &audience);
    assert_eq!(ids(&after.unread()), vec!["main-new"]);
}

/// Signoff 60 #4: a commit that fails after the output was flushed exits
/// non-zero with a message that says the mail was shown but not fully
/// recorded, the mail stays unread, and running the read again records it.
/// The failure here is a receipt lock another process holds past the
/// bounded wait.
#[test]
fn story1843_b2c_inbox_commit_failure_exits_non_zero_and_a_retry_records() {
    let audience = operator();
    let _env = operator_env();
    let r = mail_repo();
    // A first read creates the receipt anchor and its lock file.
    crate::mailbox_store::write_message(&r.main, &mail("first", "operator", 10, false)).unwrap();
    run(&r.store, inbox_cmd()).unwrap();
    crate::mailbox_store::write_message(&r.main, &mail("second", "operator", 20, false)).unwrap();

    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(r.main.join(".git/aida/mail-lifecycle/receipts.lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    let err = run(&r.store, inbox_cmd()).unwrap_err();
    fs2::FileExt::unlock(&lock).unwrap();
    let text = format!("{err:#}");
    assert!(
        text.contains("the mail above was shown, but recording it as read did not finish"),
        "{text}"
    );
    assert!(text.contains("run `aida mailbox inbox` again"), "{text}");
    assert_eq!(
        ids(&view_from(&r.main, &r.store, &audience).unread()),
        vec!["second"]
    );

    run(&r.store, inbox_cmd()).unwrap();
    assert!(view_from(&r.main, &r.store, &audience).unread().is_empty());
}

// STORY-1843 D1 / F1: the verified session binding carries the recipient
// UUID its reservation names, and a reader whose live identity does not own
// that UUID never takes queued role mail.
// trace:STORY-1843 | ai:claude
#[test]
fn story1843_d1_queued_role_mail_is_taken_only_by_its_live_owner() {
    let r = mail_repo();
    let uuid = "00000000-0000-4000-8000-0000000000aa";
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("reviewer")),
        ("AIDA_HANDLE", None),
        ("AIDA_AGENT_NAME", None),
        ("AIDA_HOOK_PAYLOAD", None),
    ]);
    let facts = bound_facts_with_recipient("rev-a", "codex", uuid);
    assert_eq!(facts.own_recipient().map(|u| u.as_str()), Some(uuid));
    let audience = ResolvedAudience::from_facts(&facts);
    assert!(audience.recipient().is_some());

    let sidecar = aida_core::mailbox_delivery::DeliverySidecar::queued("q1", "reviewer", 5);
    crate::mailbox_store::write_message_with_deliveries(
        &r.sib,
        &mail("q1", "reviewer", 5, false),
        Some(&sidecar),
    )
    .unwrap();
    // This test process has no registry row in the repository, so its live
    // identity owns no UUID: the explicit read re-checks and binds nothing.
    let view = read_own_mail_binding(
        &r.main,
        &r.store,
        &audience,
        &explicit_limits(),
        BindWait::Explicit,
    );
    assert!(view.records().is_empty());
    let (taken, _) = snapshot_view(
        &r.main,
        &r.store,
        &audience,
        &Query::Inbox,
        &explicit_limits(),
    );
    assert_eq!(taken.unwrap().1.pending_deliveries.len(), 1, "still queued");
}

pub(crate) const D1_SESSION: &str = "d1-test-thread-0001";

/// Register this test process's own session as a `reviewer` reader in
/// `root`'s registry so its live identity owns a recipient UUID, and return
/// that UUID's audience. Two rows are registered: one keyed to the nearest
/// vendor ancestor (a developer's or reviewer's client), one keyed to this
/// process and the Codex thread id [`d1_env`] exports (CI, with no vendor
/// ancestor). The reservation is then written for whichever row the
/// production session lookup actually selects, read back from disk, so it
/// names that row's id and handle exactly as `SessionBindingFacts::verify`
/// sees them.
// trace:STORY-1843 | ai:claude
pub(crate) fn register_self_as_reviewer(root: &Path) -> ResolvedAudience {
    let me = std::process::id();
    let nearest = ["claude", "codex", "antigravity"]
        .into_iter()
        .filter_map(|vendor| {
            let pid = crate::agent_registry::session_start_agent_pid(me, vendor)?;
            let depth = crate::agent_registry::ancestor_depth(me, pid)?;
            Some((depth, pid, vendor))
        })
        .min_by_key(|(depth, _, _)| *depth);
    let mut rows = vec![(me, "codex", "rev-me")];
    if let Some((_, pid, vendor)) = nearest {
        rows.push((pid, vendor, "rev-host"));
    }
    for (pid, vendor, handle) in rows {
        let mut entry = crate::agent_registry::register_existing_agent(
            root,
            vendor,
            pid,
            "reviewer".into(),
            None,
            root.to_path_buf(),
            Some(handle.into()),
            None,
        )
        .unwrap();
        if pid == me {
            entry.native_session_id = Some(D1_SESSION.into());
            crate::agent_registry::write_entry(root, &entry).unwrap();
        }
    }
    let row = crate::agent_handle::current_session_row_in(root)
        .expect("the session lookup selects one of the registered rows");
    let id = row.file_stem().unwrap().to_str().unwrap().to_string();
    let entry = crate::agent_registry::load_entries(root)
        .into_iter()
        .map(|(_, e)| e)
        .find(|e| e.id == id)
        .expect("the selected row is on disk");
    crate::mailbox_resolver::reserve_test_reader(root, &entry);
    let audience = own_audience_in(root);
    assert!(
        audience.recipient().is_some(),
        "row {id} (handle {:?}) must own its reservation's UUID",
        entry.name
    );
    audience
}

fn d1_env() -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("reviewer")),
        ("AIDA_HANDLE", None),
        ("AIDA_AGENT_NAME", None),
        ("AIDA_HOOK_PAYLOAD", None),
        ("CODEX_THREAD_ID", Some(D1_SESSION)),
        // The caller's own Claude session must not win the native-session
        // fallback where vendor ancestors are hidden.
        ("CLAUDE_CODE_SESSION_ID", None),
        ("CLAUDE_PID", None),
    ])
}

fn queue_role_mail(root: &Path, id: &str, ts: i64) {
    let sidecar = aida_core::mailbox_delivery::DeliverySidecar::queued(id, "reviewer", ts);
    crate::mailbox_store::write_message_with_deliveries(
        root,
        &mail(id, "reviewer", ts, false),
        Some(&sidecar),
    )
    .unwrap();
}

fn pending(r: &MailRepo, audience: &ResolvedAudience) -> usize {
    let (taken, _) = snapshot_view(
        &r.main,
        &r.store,
        audience,
        &Query::Inbox,
        &explicit_limits(),
    );
    taken.unwrap().1.pending_deliveries.len()
}

// F4 happy path through the adapter: an owning reader binds queued role mail
// from an explicit read and from the notice, and the re-taken view shows it
// as delivered to its UUID.
// trace:STORY-1843 | ai:claude
#[test]
fn story1843_d1_owning_reader_takes_queued_mail_on_explicit_read_and_notice() {
    let r = mail_repo();
    let _env = d1_env();
    let audience = register_self_as_reviewer(&r.main);
    let own = audience
        .recipient()
        .cloned()
        .expect("the registered session owns a recipient UUID");

    for (id, wait) in [
        ("q-explicit", BindWait::Explicit),
        ("q-notice", BindWait::Notice),
    ] {
        queue_role_mail(&r.sib, id, 10);
        assert_eq!(pending(&r, &audience), 1, "{id} waits for a reader");
        let limits = match wait {
            BindWait::Explicit => explicit_limits(),
            BindWait::Notice => passive_limits(),
        };
        let view = read_own_mail_binding(&r.main, &r.store, &audience, &limits, wait);
        let record = view
            .records()
            .iter()
            .find(|rec| rec.message.id == id)
            .unwrap_or_else(|| panic!("{id} is shown once taken"));
        assert_eq!(record.delivered_to.as_ref(), Some(&own), "{id}");
        assert_eq!(pending(&r, &audience), 0, "{id} is bound");
    }
}

// F4 refusal path: with the Lifecycle lock held, the notice returns well
// inside its deadline and leaves every queued delivery pending.
// trace:STORY-1843 | ai:claude
#[test]
fn story1843_d1_notice_never_waits_on_a_held_lifecycle_lock() {
    use std::os::unix::fs::DirBuilderExt;
    let r = mail_repo();
    let _env = d1_env();
    let audience = register_self_as_reviewer(&r.main);
    for (i, id) in ["q1", "q2", "q3"].into_iter().enumerate() {
        queue_role_mail(&r.sib, id, 10 + i as i64);
    }
    assert_eq!(pending(&r, &audience), 3);

    let anchor = r.main.join(".git/aida/mail-lifecycle");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&anchor)
        .unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(anchor.join("lifecycle.lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();

    let started = std::time::Instant::now();
    let view = read_own_mail_binding(
        &r.main,
        &r.store,
        &audience,
        &passive_limits(),
        BindWait::Notice,
    );
    let took = started.elapsed();
    // The pass itself is capped at 50 ms; the rest is two bounded snapshot
    // reads. One Explicit per-call wait alone would be 2 s.
    assert!(
        took < std::time::Duration::from_secs(1),
        "the notice waited {took:?} on a held lock"
    );
    assert!(view.records().is_empty());
    assert_eq!(pending(&r, &audience), 3, "nothing was bound");
    fs2::FileExt::unlock(&lock).unwrap();
}

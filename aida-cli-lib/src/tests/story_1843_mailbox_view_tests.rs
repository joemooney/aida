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
    mark_displayed(&r.main, view.audience(), &rows).unwrap();

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
/// and reads as `read (legacy)` in the explicit inbox; it gets no receipt
/// when displayed, and the view is not caught up on it.
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

    mark_displayed(&r.main, view.audience(), &default_rows).unwrap();
    let receipts = crate::mailbox_store::read_receipts(&r.main, "operator");
    assert!(receipts.contains_key("new"));
    assert!(
        !receipts.contains_key("old"),
        "a legacy read has no known read time; no receipt is invented"
    );
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
    mark_displayed(&r.main, view.audience(), &rows).unwrap();
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
    mark_displayed(&r.main, view.audience(), &rows).unwrap();
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
    mark_displayed(&r.main, view.audience(), &shown).unwrap();
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
    mark_displayed(&r.main, view.audience(), &rows).unwrap();
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
    mark_displayed(&r.sib, view.audience(), &rows).unwrap();
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

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

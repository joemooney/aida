//! STORY-1843 B1 conformance: one real common-Git repository with main,
//! sibling and pooled worktrees, a canonical store worktree and an unrelated
//! clone. No real mailboxes, recipients or vendors.
// trace:STORY-1843 | ai:claude
use super::*;
use std::process::Command;

fn git(cwd: &Path, args: &[&str]) {
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

fn init(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q", "-b", "main"]);
    std::fs::write(path.join("README.md"), "seed").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "seed"]);
}

struct Repo {
    _dir: tempfile::TempDir,
    main: PathBuf,
    sib_a: PathBuf,
    sib_b: PathBuf,
    pool0: PathBuf,
    store: PathBuf,
    clone: PathBuf,
}

/// main, two branch siblings, a detached pooled checkout with no
/// `.aida-store` alias, the store worktree at `main/.aida-store` on the
/// store branch, and an unrelated repository next to them.
fn repo() -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let main = base.join("main");
    init(&main);
    let sib_a = base.join("sib-a");
    let sib_b = base.join("sib-b");
    let pool0 = base.join("pool0");
    let store = main.join(".aida-store");
    git(
        &main,
        &["worktree", "add", "-q", "-b", "a", sib_a.to_str().unwrap()],
    );
    git(
        &main,
        &["worktree", "add", "-q", "-b", "b", sib_b.to_str().unwrap()],
    );
    git(
        &main,
        &["worktree", "add", "-q", "--detach", pool0.to_str().unwrap()],
    );
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
    // Sibling worktrees commonly alias the store; the canonical layer must
    // still be read once.
    std::os::unix::fs::symlink(&store, sib_a.join(".aida-store")).unwrap();
    let clone = base.join("unrelated");
    init(&clone);
    Repo {
        _dir: dir,
        main,
        sib_a,
        sib_b,
        pool0,
        store,
        clone,
    }
}

fn message(id: &str, thread: &str, to: &str, ts: i64) -> serde_json::Value {
    serde_json::json!({
        "id": id, "thread_id": thread, "from": "sender",
        "to": {"kind": "agent", "agent": to},
        "timestamp": ts, "body": format!("body of {id}"),
    })
}

fn put_in(dir: &Path, value: &serde_json::Value) {
    std::fs::create_dir_all(dir).unwrap();
    let id = value["id"].as_str().unwrap();
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_vec(value).unwrap(),
    )
    .unwrap();
}

fn put(root: &Path, value: &serde_json::Value) {
    put_in(&root.join(".aida/mailbox"), value);
}

fn put_canonical(store: &Path, value: &serde_json::Value) {
    put_in(&store.join("mailbox"), value);
}

fn advisor() -> ResolvedAudience {
    ResolvedAudience::from_identity_policy(vec![], vec![], vec!["advisor".into()]).unwrap()
}

fn snap(from: &Path, store: Option<&Path>, query: Query) -> LogicalSnapshot {
    let ctx = MailboxContext::resolve(from, store).unwrap();
    logical_snapshot(&ctx, &advisor(), &query, &ReadLimits::explicit_read())
}

fn ids(s: &LogicalSnapshot) -> Vec<&str> {
    s.records.iter().map(|r| r.message.id.as_str()).collect()
}

fn status_of<'a>(s: &'a LogicalSnapshot, root: &Path) -> &'a SourceStatus {
    &s.sources
        .iter()
        .find(|r| r.kind == SourceKind::Local(root.to_path_buf()))
        .unwrap_or_else(|| panic!("no source {root:?} in {:?}", s.sources))
        .status
}

fn tree_listing(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if path.is_dir() && !path.is_symlink() {
                stack.push(path.clone());
            }
            out.push(path);
        }
    }
    out.sort();
    out
}

#[test]
fn story1843_logical_inbox_conformance_sibling_only_mail_is_found_everywhere() {
    let r = repo();
    put(&r.sib_a, &message("sibling-only", "t1", "advisor", 10));
    put_canonical(&r.store, &message("canonical-1", "t2", "advisor", 20));
    put(&r.clone, &message("other-repo", "t3", "advisor", 30));

    for from in [&r.main, &r.pool0, &r.sib_b] {
        let s = snap(from, Some(&r.store), Query::Inbox);
        assert_eq!(
            ids(&s),
            vec!["sibling-only", "canonical-1"],
            "from {from:?}"
        );
        assert!(s.complete(), "{:?}", s.sources);
        assert_eq!(s.caught_up(), Some(false));
        assert_eq!(s.retention, RetentionCoverage::Unestablished);
        let canonical: Vec<_> = s
            .sources
            .iter()
            .filter(|x| matches!(x.kind, SourceKind::Canonical(_)))
            .collect();
        assert_eq!(canonical.len(), 1, "canonical read exactly once");
        assert!(s
            .sources
            .iter()
            .all(|x| x.kind != SourceKind::Local(r.store.clone())));
        assert!(s
            .sources
            .iter()
            .all(|x| x.kind != SourceKind::Local(r.clone.clone())));
        assert_eq!(s.sources[0].kind, SourceKind::Local(from.to_path_buf()));
    }
}

#[test]
fn story1843_context_keeps_requested_checkout_and_refuses_foreign_store() {
    let r = repo();
    put(&r.pool0, &message("pool-local", "t", "advisor", 1));
    let ctx = MailboxContext::resolve(&r.pool0, Some(&r.store)).unwrap();
    assert_eq!(ctx.requested_checkout().path(), r.pool0);
    assert_eq!(
        ctx.canonical(),
        &CanonicalCoverage::Store(StoreRoot(r.store.clone()))
    );

    let foreign = MailboxContext::resolve(&r.pool0, Some(&r.clone)).unwrap();
    assert_eq!(
        foreign.canonical(),
        &CanonicalCoverage::Refused(SourceErrorCode::CommonGitMismatch)
    );
    let s = logical_snapshot(
        &foreign,
        &advisor(),
        &Query::Inbox,
        &ReadLimits::explicit_read(),
    );
    assert_eq!(ids(&s), vec!["pool-local"]);
    assert!(!s.complete());
    assert_eq!(s.caught_up(), None);

    // No store at all is ordinary, not an error.
    let none = snap(&r.pool0, None, Query::Inbox);
    assert!(none.complete());
    assert_eq!(none.canonical, CanonicalCoverage::NotConfigured);
    assert_eq!(
        MailboxContext::resolve(&r.main.join("README.md"), None).unwrap_err(),
        SourceErrorCode::NotAGitCheckout
    );
}

#[test]
fn story1843_reconcile_by_id_rank_canonical_preference_and_conflicts() {
    let r = repo();
    // A delete marker in a sibling suppresses the main replica.
    put(&r.main, &message("deleted", "t", "advisor", 1));
    let mut marker = message("deleted", "t", "advisor", 1);
    marker["deleted"] = true.into();
    put(&r.sib_a, &marker);
    // Identical replicas collapse to one record with both provenances.
    put(&r.main, &message("same", "t", "advisor", 2));
    put_canonical(&r.store, &message("same", "t", "advisor", 2));
    // Equal-rank divergent payload is a conflict.
    put(&r.main, &message("diverged", "t", "advisor", 3));
    let mut other = message("diverged", "t", "advisor", 3);
    other["body"] = "different".into();
    put(&r.sib_b, &other);
    // Audience disagreement is a conflict whatever the rank.
    put(&r.main, &message("moved", "t", "advisor", 4));
    let mut moved = message("moved", "t", "someone", 4);
    moved["archived"] = true.into();
    put(&r.sib_b, &moved);

    let s = snap(&r.main, Some(&r.store), Query::Inbox);
    assert_eq!(ids(&s), vec!["same"]);
    let same = &s.records[0];
    assert_eq!(same.provenance.len(), 2);
    assert_eq!(
        same.provenance[0].fingerprint,
        same.provenance[1].fingerprint
    );
    assert_eq!(s.conflicts, vec!["diverged", "moved"]);
    assert!(!s.complete());
    assert_eq!(s.caught_up(), None);
    let exact = snap(&r.main, Some(&r.store), Query::ExactId("diverged".into()));
    assert!(exact.records.is_empty());
    assert_eq!(exact.exact_found, Some(true));
}

#[test]
fn story1843_read_evidence_is_id_scoped_and_watermarks_stay_legacy() {
    let r = repo();
    put(&r.main, &message("replicated", "t", "advisor", 10));
    put(&r.sib_a, &message("replicated", "t", "advisor", 10));
    put(&r.sib_a, &message("private-unseen", "t", "advisor", 5));
    put_canonical(&r.store, &message("canonical-late", "t", "advisor", 5));
    // Arrived in main after its watermark was set, with an older timestamp.
    put(&r.main, &message("late-local", "late", "advisor", 7));
    let read = r.main.join(".aida/mailbox/.read");
    std::fs::create_dir_all(&read).unwrap();
    std::fs::write(read.join("advisor.txt"), "100").unwrap();
    let before = tree_listing(r.main.parent().unwrap());

    let s = snap(&r.main, Some(&r.store), Query::Inbox);
    let evidence: BTreeMap<_, _> = s
        .records
        .iter()
        .map(|x| (x.message.id.as_str(), &x.read))
        .collect();
    let main_source = ReadEvidence::LegacyWatermark {
        source: 0,
        key: "advisor".into(),
    };
    assert_eq!(evidence["replicated"], &main_source);
    assert_eq!(
        evidence["late-local"], &main_source,
        "legacy, never Acknowledged"
    );
    assert_eq!(evidence["private-unseen"], &ReadEvidence::Unread);
    assert_eq!(evidence["canonical-late"], &ReadEvidence::Unread);
    // Existing inbox order: timestamp, then ID.
    assert_eq!(s.unread_ids(), vec!["canonical-late", "private-unseen"]);
    assert_eq!(s.caught_up(), Some(false));
    // A legacy watermark alone is unread-compatible: the late, old-timestamp
    // arrival keeps the view from being caught up even with nothing Unread.
    let late = snap(&r.main, Some(&r.store), Query::Thread("late".into()));
    assert_eq!(ids(&late), vec!["late-local"]);
    assert!(late.unread_ids().is_empty());
    assert!(late.complete());
    assert_eq!(late.caught_up(), Some(false));
    assert_eq!(
        before,
        tree_listing(r.main.parent().unwrap()),
        "snapshot wrote nothing"
    );

    // An explicit per-ID receipt in any local source acknowledges that ID.
    let receipts = r.sib_a.join(".aida/mailbox/.read-receipts/advisor");
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(receipts.join("private-unseen.txt"), "120").unwrap();
    let s = snap(&r.main, Some(&r.store), Query::Inbox);
    let unseen = s
        .records
        .iter()
        .find(|x| x.message.id == "private-unseen")
        .unwrap();
    let sib_a = s
        .sources
        .iter()
        .position(|x| x.kind == SourceKind::Local(r.sib_a.clone()))
        .unwrap();
    assert_eq!(
        unseen.read,
        ReadEvidence::Acknowledged {
            source: sib_a,
            key: "advisor".into(),
            at: 120
        }
    );
    // Explicit receipts for every visible ID are what make a view caught up.
    let receipts_main = r.main.join(".aida/mailbox/.read-receipts/advisor");
    std::fs::create_dir_all(&receipts_main).unwrap();
    std::fs::write(receipts_main.join("late-local.txt"), "130").unwrap();
    let late = snap(&r.main, Some(&r.store), Query::Thread("late".into()));
    assert_eq!(late.caught_up(), Some(true));

    // A corrupt receipt is an incomplete source, not "unread" or "read".
    std::fs::write(receipts.join("private-unseen.txt"), "garbage").unwrap();
    let s = snap(&r.main, Some(&r.store), Query::Inbox);
    assert_eq!(
        status_of(&s, &r.sib_a),
        &SourceStatus::Error(SourceErrorCode::CorruptReceipt)
    );
    assert_eq!(s.caught_up(), None);
}

#[test]
fn story1843_unsafe_or_partial_sources_are_typed_incomplete() {
    let cases: [(&str, SourceErrorCode); 5] = [
        ("symlink-dir", SourceErrorCode::UnsafeEntry),
        ("symlink-file", SourceErrorCode::UnsafeEntry),
        ("hardlink", SourceErrorCode::UnsafeEntry),
        ("fifo", SourceErrorCode::UnsafeEntry),
        ("corrupt", SourceErrorCode::CorruptMessage),
    ];
    for (case, code) in cases {
        let r = repo();
        put(&r.main, &message("main-ok", "t", "advisor", 1));
        let mail = r.sib_a.join(".aida/mailbox");
        let outside = r.sib_a.join("outside");
        std::fs::create_dir_all(r.sib_a.join(".aida")).unwrap();
        match case {
            "symlink-dir" => {
                put_in(&outside, &message("hidden", "t", "advisor", 2));
                std::os::unix::fs::symlink(&outside, &mail).unwrap();
            }
            "symlink-file" => {
                std::fs::create_dir_all(&mail).unwrap();
                std::fs::write(&outside, "{}").unwrap();
                std::os::unix::fs::symlink(&outside, mail.join("x.json")).unwrap();
            }
            "hardlink" => {
                std::fs::create_dir_all(&mail).unwrap();
                std::fs::write(&outside, "{}").unwrap();
                std::fs::hard_link(&outside, mail.join("x.json")).unwrap();
            }
            "fifo" => {
                std::fs::create_dir_all(&mail).unwrap();
                let c = CString::new(mail.join("x.json").as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            }
            _ => {
                std::fs::create_dir_all(&mail).unwrap();
                std::fs::write(mail.join("x.json"), "not json").unwrap();
            }
        }
        let started = Instant::now();
        let s = snap(&r.main, Some(&r.store), Query::Inbox);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{case} must not block"
        );
        assert_eq!(
            status_of(&s, &r.sib_a),
            &SourceStatus::Error(code),
            "{case}"
        );
        assert_eq!(ids(&s), vec!["main-ok"], "{case}");
        assert!(!s.complete(), "{case}");
        assert_eq!(s.caught_up(), None, "{case}");
        let missing = snap(&r.main, Some(&r.store), Query::ExactId("hidden".into()));
        assert_eq!(
            missing.exact_found, None,
            "{case}: not-found is not conclusive"
        );
    }
}

#[test]
fn story1843_budget_layout_and_missing_worktree_are_reported() {
    let r = repo();
    put(&r.sib_a, &message("big", "t", "advisor", 1));
    let ctx = MailboxContext::resolve(&r.main, Some(&r.store)).unwrap();
    // The inventory output draws from the same byte budget as the mail.
    let inventory_bytes = list_worktrees(&r.main, &mut Budget::new(&ReadLimits::explicit_read()))
        .unwrap()
        .len() as u64;
    let mut limits = ReadLimits::explicit_read();
    limits.max_total_bytes = inventory_bytes - 1;
    let s = logical_snapshot(&ctx, &advisor(), &Query::Inbox, &limits);
    assert_eq!(
        s.inventory_error,
        Some(SourceErrorCode::InventoryOverBudget)
    );
    limits.max_total_bytes = inventory_bytes + 8;
    let s = logical_snapshot(&ctx, &advisor(), &Query::Inbox, &limits);
    assert_eq!(s.inventory_error, None);
    assert_eq!(
        status_of(&s, &r.sib_a),
        &SourceStatus::Error(SourceErrorCode::OverBudget)
    );
    assert_eq!(s.caught_up(), None);

    // The entry budget is aggregate across sources, not per directory.
    put(&r.main, &message("main-one", "t", "advisor", 2));
    let mut limits = ReadLimits::explicit_read();
    limits.max_entries = 1;
    let s = logical_snapshot(&ctx, &advisor(), &Query::Inbox, &limits);
    assert_eq!(
        status_of(&s, &r.main),
        &SourceStatus::Observed { messages: 1 }
    );
    assert_eq!(
        status_of(&s, &r.sib_a),
        &SourceStatus::Error(SourceErrorCode::OverBudget)
    );
    assert_eq!(s.caught_up(), None);

    let expired = ReadLimits::with_deadline(Instant::now());
    let s = logical_snapshot(&ctx, &advisor(), &Query::Inbox, &expired);
    assert_eq!(
        s.inventory_error,
        Some(SourceErrorCode::InventoryOverBudget)
    );
    assert_eq!(s.caught_up(), None);

    // A layout manifest in place of the mailbox directory.
    std::fs::create_dir_all(r.sib_b.join(".aida")).unwrap();
    std::fs::write(
        r.sib_b.join(".aida/mailbox"),
        "# aida-mailbox-layout-manifest\nlayout = 2\n",
    )
    .unwrap();
    // A registered worktree deleted from disk.
    std::fs::remove_dir_all(&r.pool0).unwrap();
    let s = snap(&r.main, Some(&r.store), Query::Inbox);
    assert_eq!(
        status_of(&s, &r.sib_b),
        &SourceStatus::Error(SourceErrorCode::LayoutUnsupported)
    );
    let pool0 = s
        .sources
        .iter()
        .find(|x| x.kind == SourceKind::Local(r.pool0.clone()))
        .unwrap();
    assert!(matches!(
        pool0.status,
        SourceStatus::Error(SourceErrorCode::WorktreeMissing | SourceErrorCode::WorktreePrunable)
    ));
    assert_eq!(s.caught_up(), None);
}

#[test]
fn story1843_audience_filter_precedes_projection() {
    let r = repo();
    put(&r.sib_a, &message("mine", "shared-thread", "advisor", 1));
    put(
        &r.sib_a,
        &message("theirs", "shared-thread", "someone-else", 2),
    );
    let mut sent = message("own-sent", "shared-thread", "advisor", 3);
    sent["from"] = "advisor".into();
    put(&r.main, &sent);
    // A divergent private message for another recipient stays invisible,
    // even as a conflict ID.
    put(&r.main, &message("their-conflict", "x", "someone-else", 4));
    let mut div = message("their-conflict", "x", "someone-else", 4);
    div["body"] = "changed".into();
    put(&r.sib_b, &div);

    let thread = snap(
        &r.main,
        Some(&r.store),
        Query::Thread("shared-thread".into()),
    );
    assert_eq!(ids(&thread), vec!["mine"]);
    let inbox = snap(&r.main, Some(&r.store), Query::Inbox);
    assert_eq!(ids(&inbox), vec!["mine"]);
    assert!(inbox.conflicts.is_empty());
    assert!(inbox.complete());
    let exact = snap(&r.main, Some(&r.store), Query::ExactId("theirs".into()));
    assert!(exact.records.is_empty());
    assert_eq!(exact.exact_found, Some(false));

    assert!(ResolvedAudience::from_identity_policy(vec!["../x".into()], vec![], vec![]).is_err());
    assert!(ResolvedAudience::from_identity_policy(vec![String::new()], vec![], vec![]).is_err());
}

#[test]
fn story1843_worktree_porcelain_z_parser() {
    let raw = b"worktree /a\0HEAD 1\0branch refs/heads/main\0\0worktree /s\0HEAD 2\0branch refs/heads/aida-store\0\0worktree /p\0HEAD 3\0detached\0prunable gitdir file points to non-existent location\0\0";
    let recs = parse_worktree_porcelain_z(raw).unwrap();
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0].path, PathBuf::from("/a"));
    assert_eq!(recs[1].branch.as_deref(), Some(STORE_BRANCH));
    assert!(recs[2].prunable);
    assert_eq!(
        parse_worktree_porcelain_z(b"HEAD 1\0\0"),
        Err(SourceErrorCode::InventoryFailed)
    );
}

#[test]
fn story1843_older_writer_replica_merges_optional_metadata() {
    let r = repo();
    // The canonical copy came from an older writer that dropped the
    // optional, writer-enriched fields; the local copy carries them.
    let lean = message("enriched", "t", "advisor", 1);
    let mut rich = lean.clone();
    rich["subject"] = "Subject line".into();
    rich["from_source"] = "agent_name".into();
    rich["from_role"] = "implementer".into();
    rich["urgent"] = true.into();
    put(&r.main, &rich);
    put_canonical(&r.store, &lean);
    // Two writers that recorded DIFFERENT values contradict each other.
    let mut a = message("two-subjects", "t", "advisor", 2);
    a["subject"] = "one".into();
    let mut b = a.clone();
    b["subject"] = "two".into();
    put(&r.main, &a);
    put_canonical(&r.store, &b);

    let s = snap(&r.main, Some(&r.store), Query::Inbox);
    assert_eq!(ids(&s), vec!["enriched"]);
    let m = &s.records[0].message;
    assert_eq!(s.records[0].provenance.len(), 2);
    assert_eq!(m.subject.as_deref(), Some("Subject line"));
    assert_eq!(m.from_role.as_deref(), Some("implementer"));
    assert_eq!(m.from_source, crate::mailbox::SenderSource::AgentName);
    assert!(m.urgent);
    assert_eq!(s.conflicts, vec!["two-subjects"]);
}

#[test]
fn story1843_conflicts_are_scoped_to_the_query() {
    let r = repo();
    put(&r.main, &message("healthy", "good", "advisor", 1));
    put(&r.main, &message("split", "bad", "advisor", 2));
    let mut other = message("split", "bad", "advisor", 2);
    other["body"] = "different".into();
    put(&r.sib_b, &other);

    let thread = snap(&r.main, Some(&r.store), Query::Thread("good".into()));
    assert_eq!(ids(&thread), vec!["healthy"]);
    assert!(thread.conflicts.is_empty());
    assert!(thread.complete());
    let missing = snap(&r.main, Some(&r.store), Query::ExactId("nope".into()));
    assert!(missing.conflicts.is_empty());
    assert_eq!(missing.exact_found, Some(false));
    let bad = snap(&r.main, Some(&r.store), Query::Thread("bad".into()));
    assert_eq!(bad.conflicts, vec!["split"]);
    assert_eq!(bad.caught_up(), None);
}

#[test]
fn story1843_retracted_records_are_redacted_and_sources_carry_identity() {
    let r = repo();
    let mut withdrawn = message("withdrawn", "t", "advisor", 1);
    withdrawn["retracted"] = true.into();
    withdrawn["subject"] = "secret subject".into();
    put(&r.sib_a, &withdrawn);

    let s = snap(&r.main, Some(&r.store), Query::Inbox);
    assert_eq!(ids(&s), vec!["withdrawn"]);
    assert!(s.records[0].message.retracted);
    assert!(s.records[0].message.body.is_empty());
    assert_eq!(s.records[0].message.subject, None);

    assert!(s.observed_at_ms > 0);
    let sib_a = s
        .sources
        .iter()
        .find(|x| x.kind == SourceKind::Local(r.sib_a.clone()))
        .unwrap();
    let meta = std::fs::metadata(r.sib_a.join(".aida/mailbox")).unwrap();
    let identity = sib_a.identity.expect("observed source is pinned");
    assert_eq!((identity.dev, identity.ino), (meta.dev(), meta.ino()));
    let main = s
        .sources
        .iter()
        .find(|x| x.kind == SourceKind::Local(r.main.clone()))
        .unwrap();
    assert_eq!(main.status, SourceStatus::Absent);
    assert_eq!(main.identity, None);
}

#[test]
fn story1843_readdir_error_is_never_a_complete_listing() {
    assert_eq!(readdir_end(Some(0)), Ok(()));
    assert_eq!(readdir_end(None), Ok(()));
    assert_eq!(
        readdir_end(Some(libc::EIO)),
        Err(SourceErrorCode::Unreadable)
    );
    assert_eq!(
        readdir_end(Some(libc::EBADF)),
        Err(SourceErrorCode::Unreadable)
    );
}
